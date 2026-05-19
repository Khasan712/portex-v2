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
use crate::registry::Registry;
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
    match auth.authorize(&hello.auth_token, &subdomain).await {
        Ok(user) => {
            tracing::info!(%remote, %subdomain, user_id = %user.0, "tunnel: authorized");
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
    }

    if registry.lookup(&subdomain).await.is_some() {
        let reject = Reject {
            reason: RejectReason::SubdomainTaken,
            message: "subdomain already connected".into(),
        };
        reject_and_close(&conn, &mut send, reject).await;
        return Ok(());
    }

    Accept {
        server_version: PROTOCOL_VERSION,
        assigned_subdomain: subdomain.clone(),
    }
    .into_frame()?
    .write_to(&mut send)
    .await?;

    let conn_arc = Arc::new(conn);
    registry.insert(subdomain.clone(), conn_arc.clone()).await;
    metrics.tunnel_connects_total.fetch_add(1, Ordering::Relaxed);
    let total = registry.len().await;
    tracing::info!(%subdomain, total, "tunnel: registered");

    let close_reason = conn_arc.closed().await;
    registry.remove(&subdomain).await;
    metrics.tunnel_disconnects_total.fetch_add(1, Ordering::Relaxed);
    tracing::info!(%subdomain, ?close_reason, "tunnel: disconnected");
    Ok(())
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
