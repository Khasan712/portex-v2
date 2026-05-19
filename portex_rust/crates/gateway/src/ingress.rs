use std::net::SocketAddr;
use std::sync::Arc;
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

/// Plain HTTP listener — used either as the primary public ingress, or as
/// the redirect-to-HTTPS port when an HTTPS listener is also configured.
pub async fn serve(
    addr: SocketAddr,
    registry: Arc<Registry>,
    metrics: Arc<Metrics>,
    base_domain: String,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await.context("bind HTTP listener")?;
    tracing::info!(%addr, %base_domain, "ingress: HTTP listening");

    loop {
        let (sock, peer) = listener.accept().await?;
        sock.set_nodelay(true).ok();
        let registry = registry.clone();
        let metrics = metrics.clone();
        let base_domain = base_domain.clone();
        tokio::spawn(async move {
            if let Err(err) = handle(sock, peer, registry, metrics, base_domain).await {
                tracing::debug!(%peer, ?err, "ingress: connection ended");
            }
        });
    }
}

/// HTTPS listener — terminates TLS using the current cert from `tls`, then
/// runs the same proxy::splice pipeline on the decrypted stream. The cert
/// is loaded fresh on each accept so renewals take effect without restart.
pub async fn serve_https(
    addr: SocketAddr,
    registry: Arc<Registry>,
    metrics: Arc<Metrics>,
    base_domain: String,
    tls: Arc<Reloadable>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await.context("bind HTTPS listener")?;
    tracing::info!(%addr, %base_domain, "ingress: HTTPS listening");

    loop {
        let (sock, peer) = listener.accept().await?;
        sock.set_nodelay(true).ok();
        let acceptor = TlsAcceptor::from(tls.https.load_full());
        let registry = registry.clone();
        let metrics = metrics.clone();
        let base_domain = base_domain.clone();
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
            if let Err(err) = handle(tls_stream, peer, registry, metrics, base_domain).await {
                tracing::debug!(%peer, ?err, "ingress: HTTPS connection ended");
            }
        });
    }
}

async fn handle<S>(
    mut sock: S,
    peer: SocketAddr,
    registry: Arc<Registry>,
    metrics: Arc<Metrics>,
    base_domain: String,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let head_read = timeout(Duration::from_secs(10), proxy::read_request_head(&mut sock));
    let buffered_head = match head_read.await {
        Ok(Ok(buf)) => buf,
        Ok(Err(err)) => {
            metrics.request_errors_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return write_error(&mut sock, 400, "Bad Request", err.to_string()).await;
        }
        Err(_) => {
            metrics.request_errors_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return write_error(&mut sock, 408, "Request Timeout", "header read timed out".into()).await;
        }
    };

    let host = match proxy::extract_host(&buffered_head) {
        Some(h) => h,
        None => {
            metrics.request_errors_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return write_error(&mut sock, 400, "Bad Request", "missing Host header".into()).await;
        }
    };
    let subdomain = match proxy::strip_subdomain(host, &base_domain) {
        Some(s) => s,
        None => {
            metrics.request_errors_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return write_error(&mut sock, 404, "Not Found", "subdomain not recognized".into()).await;
        }
    };

    let conn = match registry.lookup(&subdomain).await {
        Some(c) => c,
        None => {
            metrics.request_errors_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return write_error(&mut sock, 502, "Bad Gateway", format!("no tunnel for '{subdomain}'")).await;
        }
    };

    metrics.requests_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    tracing::debug!(%peer, %subdomain, head_bytes = buffered_head.len(), "ingress: routing");

    proxy::splice(sock, &conn, buffered_head, &metrics).await
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
