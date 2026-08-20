use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use portex_common::{Accept, Frame, Hello, Reject, PROTOCOL_VERSION};
use portex_common::frame::RejectReason;
use quinn::{Endpoint, TransportConfig};

use crate::auth::{AuthError, Authenticator};
use crate::metrics::Metrics;
use crate::registry::{Claim, Registry, Tunnel};
use crate::tls::Reloadable;

pub async fn serve(
    addr: SocketAddr,
    registry: Arc<Registry>,
    auth: Arc<Authenticator>,
    metrics: Arc<Metrics>,
    tls: Arc<Reloadable>,
) -> anyhow::Result<()> {
    let mut server_cfg = (**tls.quic.load()).clone();
    let mut transport = TransportConfig::default();
    transport.max_idle_timeout(Some(Duration::from_secs(60).try_into().unwrap()));
    server_cfg.transport_config(Arc::new(transport));

    let endpoint = Endpoint::server(server_cfg, addr).context("bind QUIC endpoint")?;
    tracing::info!(%addr, "tunnel: QUIC endpoint listening");

    // Watch for cert reloads and push the fresh server config into the endpoint.
    {
        let endpoint = endpoint.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            loop {
                tls.updated.notified().await;
                let mut new_cfg = (**tls.quic.load()).clone();
                let mut transport = TransportConfig::default();
                transport.max_idle_timeout(Some(Duration::from_secs(60).try_into().unwrap()));
                new_cfg.transport_config(Arc::new(transport));
                endpoint.set_server_config(Some(new_cfg));
                tracing::info!("tunnel: QUIC server config swapped");
            }
        });
    }

    while let Some(incoming) = endpoint.accept().await {
        let registry = registry.clone();
        let auth = auth.clone();
        let metrics = metrics.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_incoming(incoming, registry, auth, metrics).await {
                tracing::warn!(?err, "tunnel connection ended with error");
            }
        });
    }
    Ok(())
}

async fn handle_incoming(
    incoming: quinn::Incoming,
    registry: Arc<Registry>,
    auth: Arc<Authenticator>,
    metrics: Arc<Metrics>,
) -> anyhow::Result<()> {
    let conn = incoming.await.context("QUIC handshake")?;
    let remote = conn.remote_address();
    tracing::info!(%remote, "tunnel: new connection");

    let (mut send, mut recv) = conn.accept_bi().await.context("accept control stream")?;
    let hello_frame = Frame::read_from(&mut recv).await.context("read HELLO frame")?;
    let hello = Hello::from_frame(hello_frame).context("decode HELLO")?;

    if hello.version != PROTOCOL_VERSION {
        let reject = Reject {
            reason: RejectReason::VersionIncompatible,
            message: format!("server speaks v{PROTOCOL_VERSION}, client v{}", hello.version),
        };
        reject_and_close(&conn, &mut send, reject).await;
        return Ok(());
    }

    let subdomain = hello.subdomain.clone();
    let authorized = match auth.authorize(&hello.auth_token, &subdomain).await {
        Ok(authorized) => {
            tracing::info!(%remote, %subdomain, user_id = %authorized.user_id, "tunnel: authorized");
            auth.mark_token_used(&authorized.token_hash).await;
            authorized
        }
        Err(err) => {
            let reason = match err {
                AuthError::MissingToken | AuthError::InvalidToken => RejectReason::Unauthorized,
                AuthError::SubdomainNotReserved => RejectReason::SubdomainNotReserved,
                AuthError::SubdomainTaken => RejectReason::SubdomainTaken,
                AuthError::Backend(_) => RejectReason::ServerFull,
            };
            let reject = Reject { reason, message: err.to_string() };
            reject_and_close(&conn, &mut send, reject).await;
            return Ok(());
        }
    };

    // Build the ACCEPT frame before claiming the subdomain, so a malformed
    // frame can't leave a registry entry behind.
    let accept = Accept {
        server_version: PROTOCOL_VERSION,
        assigned_subdomain: subdomain.clone(),
    }
    .into_frame()?;

    let conn_arc = Arc::new(conn);
    let tunnel = Arc::new(Tunnel {
        conn: conn_arc.clone(),
        user_id: authorized.user_id,
        token_hash: authorized.token_hash,
    });

    match registry.insert_if_absent(subdomain.clone(), tunnel.clone()).await {
        Claim::Ok => {}
        Claim::Taken => {
            let reject = Reject {
                reason: RejectReason::SubdomainTaken,
                message: "subdomain already connected".into(),
            };
            reject_and_close(&conn_arc, &mut send, reject).await;
            return Ok(());
        }
        Claim::Full => {
            tracing::warn!(%subdomain, "tunnel: at capacity, rejecting");
            let reject = Reject {
                reason: RejectReason::ServerFull,
                message: "gateway is at tunnel capacity".into(),
            };
            reject_and_close(&conn_arc, &mut send, reject).await;
            return Ok(());
        }
    }

    // From here on we own the registry entry — every exit path releases it.
    if let Err(err) = accept.write_to(&mut send).await {
        registry.remove_if(&subdomain, &tunnel).await;
        return Err(err.into());
    }

    metrics.tunnel_connects_total.fetch_add(1, Ordering::Relaxed);
    let total = registry.len().await;
    tracing::info!(%subdomain, total, "tunnel: registered");

    let close_reason = conn_arc.closed().await;
    registry.remove_if(&subdomain, &tunnel).await;
    metrics.tunnel_disconnects_total.fetch_add(1, Ordering::Relaxed);
    tracing::info!(%subdomain, ?close_reason, "tunnel: disconnected");
    Ok(())
}

/// Periodically re-check every live tunnel against the auth backend.
///
/// Credentials are only validated during the handshake, so without this a
/// revoked token or a released subdomain keeps working until the client
/// happens to disconnect. The sweep is fail-safe: a tunnel is closed only
/// when Redis answers *and* says the credentials are no longer valid. A
/// backend outage leaves existing tunnels alone.
pub async fn revalidate_loop(
    registry: Arc<Registry>,
    auth: Arc<Authenticator>,
    interval: Duration,
) {
    tracing::info!(secs = interval.as_secs(), "tunnel: credential revalidation enabled");
    loop {
        tokio::time::sleep(interval).await;
        for (subdomain, tunnel) in registry.snapshot().await {
            match auth.authorize_hash(&tunnel.token_hash, &subdomain).await {
                Ok(user_id) if user_id == tunnel.user_id => {}
                Ok(other) => {
                    tracing::info!(
                        %subdomain, new_owner = %other,
                        "revalidate: subdomain reassigned, closing tunnel"
                    );
                    close_revoked(&registry, &subdomain, &tunnel).await;
                }
                Err(AuthError::Backend(err)) => {
                    tracing::warn!(
                        ?err,
                        "revalidate: auth backend unavailable, leaving tunnels open"
                    );
                    break;
                }
                Err(err) => {
                    tracing::info!(%subdomain, %err, "revalidate: credentials no longer valid");
                    close_revoked(&registry, &subdomain, &tunnel).await;
                }
            }
        }
    }
}

async fn close_revoked(registry: &Registry, subdomain: &str, tunnel: &Arc<Tunnel>) {
    tunnel.conn.close(0u32.into(), b"credentials revoked");
    registry.remove_if(subdomain, tunnel).await;
}

/// Best-effort delivery of a REJECT frame: write, signal end of stream, then
/// wait briefly for the client to read everything before letting the
/// Connection drop. Without this, the connection often tears down before the
/// REJECT bytes make it to the client.
async fn reject_and_close(
    conn: &quinn::Connection,
    send: &mut quinn::SendStream,
    reject: Reject,
) {
    if let Ok(frame) = reject.into_frame() {
        let _ = frame.write_to(send).await;
    }
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
}
