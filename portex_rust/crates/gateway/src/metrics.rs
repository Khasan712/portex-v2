//! Prometheus-style metrics, served as plain text on a separate admin port.
//!
//! Format follows the OpenMetrics / Prometheus text exposition format. We
//! don't pull in a full metrics crate — counters/gauges are atomics owned
//! by `Metrics`, and we render them on each scrape.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use crate::registry::Registry;

#[derive(Default)]
pub struct Metrics {
    pub tunnel_connects_total: AtomicU64,
    pub tunnel_disconnects_total: AtomicU64,
    pub requests_total: AtomicU64,
    pub request_errors_total: AtomicU64,
    pub bytes_upstream_total: AtomicU64,
    pub bytes_downstream_total: AtomicU64,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn render(&self, active_tunnels: usize) -> String {
        let mut out = String::with_capacity(512);
        line(&mut out, "portex_active_tunnels", "Currently connected tunnels", "gauge", active_tunnels as u64);
        line(&mut out, "portex_tunnel_connects_total", "Tunnels accepted since start", "counter", self.tunnel_connects_total.load(Ordering::Relaxed));
        line(&mut out, "portex_tunnel_disconnects_total", "Tunnels closed since start", "counter", self.tunnel_disconnects_total.load(Ordering::Relaxed));
        line(&mut out, "portex_requests_total", "Public requests dispatched to a tunnel", "counter", self.requests_total.load(Ordering::Relaxed));
        line(&mut out, "portex_request_errors_total", "Public requests rejected by the ingress", "counter", self.request_errors_total.load(Ordering::Relaxed));
        line(&mut out, "portex_bytes_upstream_total", "Bytes sent from public client into the tunnel", "counter", self.bytes_upstream_total.load(Ordering::Relaxed));
        line(&mut out, "portex_bytes_downstream_total", "Bytes returned from tunnel to public client", "counter", self.bytes_downstream_total.load(Ordering::Relaxed));
        out
    }
}

fn line(out: &mut String, name: &str, help: &str, kind: &str, value: u64) {
    out.push_str("# HELP ");
    out.push_str(name);
    out.push(' ');
    out.push_str(help);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(name);
    out.push(' ');
    out.push_str(kind);
    out.push('\n');
    out.push_str(name);
    out.push(' ');
    out.push_str(&value.to_string());
    out.push('\n');
}

pub async fn serve(
    addr: SocketAddr,
    metrics: Arc<Metrics>,
    registry: Arc<Registry>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await.context("bind metrics listener")?;
    tracing::info!(%addr, "metrics: /metrics listening");

    loop {
        let (mut sock, _peer) = listener.accept().await?;
        let metrics = metrics.clone();
        let registry = registry.clone();
        tokio::spawn(async move {
            let active = registry.len().await;
            let body = metrics.render(active);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body,
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        });
    }
}
