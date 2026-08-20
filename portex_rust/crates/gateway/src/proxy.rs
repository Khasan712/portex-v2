//! Zero-copy-ish proxy helpers.
//!
//! The hot path never parses the HTTP request — we only locate the Host
//! header, look up the right tunnel, and pipe raw bytes both ways.

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use quinn::Connection;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

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

/// Where a public request belongs, based on its `Host` header.
#[derive(Debug, PartialEq, Eq)]
pub enum Route<'a> {
    /// The apex domain or its `www.` alias — control plane traffic.
    Apex,
    /// A tunnel subdomain, normalised to lowercase.
    Tunnel(Cow<'a, str>),
}

/// `acme.portex.live` → `Tunnel("acme")`; `portex.live` and `www.portex.live`
/// → `Apex`; anything else → `None`.
///
/// Host names are case-insensitive per RFC 4343, while the registry is keyed
/// on the lowercase names Django stores — so `ACME.portex.live` has to resolve
/// to the same tunnel as `acme.portex.live`. The comparison is done in place
/// and only allocates for the rare host that actually contains uppercase.
pub fn route<'a>(host: &'a str, base: &str) -> Option<Route<'a>> {
    let host = host.split(':').next()?;
    if host.eq_ignore_ascii_case(base) {
        return Some(Route::Apex);
    }

    let split_at = host.len().checked_sub(base.len() + 1)?;
    if !host.is_char_boundary(split_at) {
        return None;
    }
    let (sub, suffix) = host.split_at(split_at);
    if !suffix.starts_with('.') || !suffix[1..].eq_ignore_ascii_case(base) {
        return None;
    }
    if sub.is_empty() || sub.contains('.') {
        return None;
    }
    if sub.eq_ignore_ascii_case("www") {
        return Some(Route::Apex);
    }
    Some(Route::Tunnel(if sub.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(sub.to_ascii_lowercase())
    } else {
        Cow::Borrowed(sub)
    }))
}

/// Extract the request target from the HTTP request line (`GET /path HTTP/1.1`).
pub fn request_target(buf: &[u8]) -> Option<&str> {
    let line_end = buf.windows(2).position(|w| w == b"\r\n")?;
    let line = std::str::from_utf8(&buf[..line_end]).ok()?;
    line.split(' ').nth(1)
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

/// Splice the public socket to a plain TCP upstream — the Django control
/// plane serving the apex domain. Same byte-for-byte pipe as the tunnel path,
/// just with a TCP peer instead of a QUIC stream.
pub async fn splice_tcp<S>(
    sock: S,
    upstream: &str,
    buffered_head: Vec<u8>,
    metrics: &Metrics,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let up = TcpStream::connect(upstream).await?;
    up.set_nodelay(true).ok();
    let (mut up_read, mut up_write) = tokio::io::split(up);

    let head_bytes = buffered_head.len() as u64;
    up_write.write_all(&buffered_head).await?;

    let (mut sock_read, mut sock_write) = tokio::io::split(sock);

    let client_to_upstream = async {
        let r = tokio::io::copy(&mut sock_read, &mut up_write).await;
        let _ = up_write.shutdown().await;
        r
    };
    let upstream_to_client = async {
        let r = tokio::io::copy(&mut up_read, &mut sock_write).await;
        let _ = sock_write.shutdown().await;
        r
    };

    let (c2u, u2c) = tokio::join!(client_to_upstream, upstream_to_client);
    let up_total = head_bytes + c2u.unwrap_or(0);
    let down = u2c.unwrap_or(0);
    metrics.bytes_upstream_total.fetch_add(up_total, Ordering::Relaxed);
    metrics.bytes_downstream_total.fetch_add(down, Ordering::Relaxed);
    tracing::debug!(up_bytes = up_total, down_bytes = down, "ingress: spliced to apex");
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

    fn tunnel(name: &str) -> Option<Route<'_>> {
        Some(Route::Tunnel(Cow::Borrowed(name)))
    }

    #[test]
    fn route_basic() {
        assert_eq!(route("acme.portex.live", "portex.live"), tunnel("acme"));
    }

    #[test]
    fn route_with_port() {
        assert_eq!(route("acme.portex.live:8080", "portex.live"), tunnel("acme"));
    }

    #[test]
    fn route_is_case_insensitive() {
        // Host names are case-insensitive, but the registry is keyed lowercase.
        assert_eq!(route("ACME.PORTEX.LIVE", "portex.live"), tunnel("acme"));
        assert_eq!(route("AcMe.Portex.Live", "portex.live"), tunnel("acme"));
    }

    #[test]
    fn route_apex_and_www_go_to_the_control_plane() {
        assert_eq!(route("portex.live", "portex.live"), Some(Route::Apex));
        assert_eq!(route("PORTEX.LIVE", "portex.live"), Some(Route::Apex));
        assert_eq!(route("www.portex.live", "portex.live"), Some(Route::Apex));
        assert_eq!(route("WWW.portex.live", "portex.live"), Some(Route::Apex));
    }

    #[test]
    fn route_rejects_nested_and_foreign() {
        assert_eq!(route("a.b.portex.live", "portex.live"), None);
        assert_eq!(route("evil.com", "portex.live"), None);
        assert_eq!(route("notportex.live", "portex.live"), None);
    }

    #[test]
    fn route_handles_multibyte_hosts_without_panicking() {
        assert_eq!(route("ünïcode.example", "portex.live"), None);
        assert_eq!(route("日本語", "portex.live"), None);
    }

    #[test]
    fn request_target_reads_the_path() {
        assert_eq!(request_target(b"GET /a/b?c=1 HTTP/1.1\r\nHost: x\r\n\r\n"), Some("/a/b?c=1"));
        assert_eq!(request_target(b"nonsense"), None);
    }
}
