//! Zero-copy-ish proxy helpers.
//!
//! The hot path never parses the HTTP request — we only locate the Host
//! header, look up the right tunnel, and pipe raw bytes both ways.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use quinn::Connection;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::metrics::Metrics;

const MAX_HEAD_BYTES: usize = 32 * 1024;

/// Read bytes until we see the end of the HTTP/1.1 request head
/// (`\r\n\r\n`). Returns the buffered prefix, which may already contain a
/// few body bytes — these MUST be replayed onto the tunnel stream verbatim.
pub async fn read_request_head<S>(sock: &mut S) -> anyhow::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    loop {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            anyhow::bail!("client closed before request head completed");
        }
        buf.extend_from_slice(&tmp[..n]);
        if find_double_crlf(&buf).is_some() {
            return Ok(buf);
        }
        if buf.len() > MAX_HEAD_BYTES {
            anyhow::bail!("request head exceeded {MAX_HEAD_BYTES} bytes");
        }
    }
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// Extract the value of the `Host` header from the buffered TCP bytes.
///
/// The buffer may contain body bytes after `\r\n\r\n` (binary, possibly
/// invalid UTF-8) — we slice off the head before decoding so a non-text body
/// can't trip up parsing.
pub fn extract_host(buf: &[u8]) -> Option<&str> {
    let head_end = find_double_crlf(buf)?;
    let head_str = std::str::from_utf8(&buf[..head_end]).ok()?;
    for line in head_str.split("\r\n").skip(1) {
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("host") {
            return Some(value.trim());
        }
    }
    None
}

/// `acme.portex.live` + `portex.live` → `Some("acme")`. Rejects apex.
pub fn strip_subdomain<'a>(host: &'a str, base: &str) -> Option<&'a str> {
    let host = host.split(':').next()?;
    let suffix = format!(".{base}");
    if !host.ends_with(&suffix) {
        return None;
    }
    let sub = &host[..host.len() - suffix.len()];
    if sub.is_empty() || sub.contains('.') {
        return None;
    }
    Some(sub)
}

/// Open a fresh bi-directional QUIC stream and splice bytes between the
/// public socket (TCP or TLS-wrapped TCP) and the stream. The already-buffered
/// request head is written first so the client sees the complete request.
pub async fn splice<S>(
    sock: S,
    conn: &Arc<Connection>,
    buffered_head: Vec<u8>,
    metrics: &Metrics,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut quic_send, mut quic_recv) = conn.open_bi().await?;
    let head_bytes = buffered_head.len() as u64;
    quic_send.write_all(&buffered_head).await?;

    let (mut sock_read, mut sock_write) = tokio::io::split(sock);

    let client_to_tunnel = async {
        let r = tokio::io::copy(&mut sock_read, &mut quic_send).await;
        let _ = quic_send.finish();
        r
    };
    let tunnel_to_client = async {
        let r = tokio::io::copy(&mut quic_recv, &mut sock_write).await;
        let _ = sock_write.shutdown().await;
        r
    };

    let (c2t, t2c) = tokio::join!(client_to_tunnel, tunnel_to_client);
    let up = head_bytes + c2t.unwrap_or(0);
    let down = t2c.unwrap_or(0);
    metrics.bytes_upstream_total.fetch_add(up, Ordering::Relaxed);
    metrics.bytes_downstream_total.fetch_add(down, Ordering::Relaxed);
    tracing::debug!(up_bytes = up, down_bytes = down, "ingress: spliced");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_finds_lowercase_value() {
        let head = b"GET /a HTTP/1.1\r\nHost: acme.portex.live\r\nX: y\r\n\r\n";
        assert_eq!(extract_host(head), Some("acme.portex.live"));
    }

    #[test]
    fn extract_host_is_case_insensitive() {
        let head = b"GET /a HTTP/1.1\r\nhOST: acme.portex.live\r\n\r\n";
        assert_eq!(extract_host(head), Some("acme.portex.live"));
    }

    #[test]
    fn extract_host_ignores_binary_body() {
        let mut buf =
            b"POST / HTTP/1.1\r\nHost: acme.portex.live\r\nContent-Length: 3\r\n\r\n".to_vec();
        buf.extend_from_slice(&[0xff, 0xfe, 0x00]); // non-UTF-8 body bytes
        assert_eq!(extract_host(&buf), Some("acme.portex.live"));
    }

    #[test]
    fn strip_subdomain_basic() {
        assert_eq!(strip_subdomain("acme.portex.live", "portex.live"), Some("acme"));
    }

    #[test]
    fn strip_subdomain_with_port() {
        assert_eq!(strip_subdomain("acme.portex.live:8080", "portex.live"), Some("acme"));
    }

    #[test]
    fn strip_subdomain_rejects_apex() {
        assert_eq!(strip_subdomain("portex.live", "portex.live"), None);
    }

    #[test]
    fn strip_subdomain_rejects_nested() {
        assert_eq!(strip_subdomain("a.b.portex.live", "portex.live"), None);
    }
}
