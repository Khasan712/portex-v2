use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;

use crate::metrics::Metrics;
use crate::proxy;
use crate::registry::Registry;
use crate::tls::Reloadable;

/// Everything the ingress needs to route one request. Cloned per connection.
#[derive(Clone)]
pub struct Ingress {
    pub registry: Arc<Registry>,
    pub metrics: Arc<Metrics>,
    pub base_domain: String,
    /// Where apex (and `www.`) traffic goes — the Django control plane.
    /// Without it the landing page, dashboard and admin are unreachable.
    pub apex_upstream: Option<String>,
}

/// Plain HTTP listener — the primary public ingress, or a redirect-to-HTTPS
/// port when an HTTPS listener is also configured.
pub async fn serve(addr: SocketAddr, ingress: Ingress, redirect_https: bool) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await.context("bind HTTP listener")?;
    tracing::info!(%addr, base_domain = %ingress.base_domain, redirect_https, "ingress: HTTP listening");

    loop {
        let (sock, peer) = listener.accept().await?;
        sock.set_nodelay(true).ok();
        let ingress = ingress.clone();
        tokio::spawn(async move {
            let result = if redirect_https {
                redirect(sock, peer, &ingress).await
            } else {
                handle(sock, peer, ingress).await
            };
            if let Err(err) = result {
                tracing::debug!(%peer, ?err, "ingress: connection ended");
            }
        });
    }
}

/// Port 80 when TLS is configured: send everything to HTTPS instead of
/// serving it in the clear. Secure cookies depend on this actually happening.
async fn redirect<S>(mut sock: S, peer: SocketAddr, ingress: &Ingress) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let head = match timeout(Duration::from_secs(10), proxy::read_request_head(&mut sock)).await {
        Ok(Ok(buf)) => buf,
        _ => {
            ingress.metrics.connection_errors_total.fetch_add(1, Ordering::Relaxed);
            return write_error(&mut sock, 400, "Bad Request", "could not read request".into()).await;
        }
    };
    let host = proxy::extract_host(&head).unwrap_or(&ingress.base_domain);
    let target = proxy::request_target(&head).unwrap_or("/");
    let location = format!("https://{}{}", host.split(':').next().unwrap_or(host), target);
    tracing::debug!(%peer, %location, "ingress: redirecting to HTTPS");

    let resp = format!(
        "HTTP/1.1 301 Moved Permanently\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    sock.write_all(resp.as_bytes()).await.ok();
    sock.shutdown().await.ok();
    Ok(())
}

/// HTTPS listener — terminates TLS using the current cert from `tls`, then
/// runs the same proxy::splice pipeline on the decrypted stream. The cert
/// is loaded fresh on each accept so renewals take effect without restart.
pub async fn serve_https(
    addr: SocketAddr,
    ingress: Ingress,
    tls: Arc<Reloadable>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await.context("bind HTTPS listener")?;
    tracing::info!(%addr, base_domain = %ingress.base_domain, "ingress: HTTPS listening");

    loop {
        let (sock, peer) = listener.accept().await?;
        sock.set_nodelay(true).ok();
        let acceptor = TlsAcceptor::from(tls.https.load_full());
        let ingress = ingress.clone();
        tokio::spawn(async move {
            let tls_stream = match timeout(Duration::from_secs(10), acceptor.accept(sock)).await {
                Ok(Ok(s)) => s,
                Ok(Err(err)) => {
                    tracing::debug!(%peer, ?err, "ingress: TLS handshake failed");
                    return;
                }
                Err(_) => {
                    tracing::debug!(%peer, "ingress: TLS handshake timed out");
                    return;
                }
            };
            if let Err(err) = handle(tls_stream, peer, ingress).await {
                tracing::debug!(%peer, ?err, "ingress: HTTPS connection ended");
            }
        });
    }
}

async fn handle<S>(mut sock: S, peer: SocketAddr, ingress: Ingress) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Ingress { registry, metrics, base_domain, apex_upstream } = ingress;
    let head_read = timeout(Duration::from_secs(10), proxy::read_request_head(&mut sock));
    let buffered_head = match head_read.await {
        Ok(Ok(buf)) => buf,
        Ok(Err(err)) => {
            metrics.connection_errors_total.fetch_add(1, Ordering::Relaxed);
            return write_error(&mut sock, 400, "Bad Request", err.to_string()).await;
        }
        Err(_) => {
            metrics.connection_errors_total.fetch_add(1, Ordering::Relaxed);
            return write_error(&mut sock, 408, "Request Timeout", "header read timed out".into()).await;
        }
    };

    let host = match proxy::extract_host(&buffered_head) {
        Some(h) => h,
        None => {
            metrics.connection_errors_total.fetch_add(1, Ordering::Relaxed);
            return write_error(&mut sock, 400, "Bad Request", "missing Host header".into()).await;
        }
    };
    let route = match proxy::route(host, &base_domain) {
        Some(r) => r,
        None => {
            metrics.connection_errors_total.fetch_add(1, Ordering::Relaxed);
            return write_error(&mut sock, 404, "Not Found", "host not recognized".into()).await;
        }
    };

    let subdomain = match route {
        proxy::Route::Apex => {
            let Some(upstream) = apex_upstream else {
                metrics.connection_errors_total.fetch_add(1, Ordering::Relaxed);
                return write_error(
                    &mut sock,
                    404,
                    "Not Found",
                    "no control plane configured for this domain".into(),
                )
                .await;
            };
            metrics.connections_total.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(%peer, %upstream, "ingress: routing to control plane");
            return proxy::splice_tcp(sock, &upstream, buffered_head, &metrics).await;
        }
        proxy::Route::Tunnel(sub) => sub,
    };

    let tunnel = match registry.lookup(&subdomain).await {
        Some(t) => t,
        None => {
            metrics.connection_errors_total.fetch_add(1, Ordering::Relaxed);
            return write_error(&mut sock, 502, "Bad Gateway", format!("no tunnel for '{subdomain}'")).await;
        }
    };

    metrics.connections_total.fetch_add(1, Ordering::Relaxed);
    tracing::debug!(%peer, %subdomain, head_bytes = buffered_head.len(), "ingress: routing");

    proxy::splice(sock, &tunnel.conn, buffered_head, &metrics).await
}

async fn write_error<S>(
    sock: &mut S,
    status: u16,
    reason: &str,
    body: String,
) -> anyhow::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(resp.as_bytes()).await.ok();
    sock.shutdown().await.ok();
    Ok(())
}
