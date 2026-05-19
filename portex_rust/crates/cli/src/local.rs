use quinn::{RecvStream, SendStream};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

/// Forward a single QUIC stream to the local HTTP server and back.
///
/// We never parse the HTTP exchange — both directions are raw byte copies.
pub async fn pipe_stream(
    mut quic_send: SendStream,
    mut quic_recv: RecvStream,
    local_port: u16,
) -> anyhow::Result<()> {
    let mut local = TcpStream::connect(("127.0.0.1", local_port)).await?;
    local.set_nodelay(true).ok();
    let (mut local_read, mut local_write) = local.split();

    let tunnel_to_local = async {
        let r = tokio::io::copy(&mut quic_recv, &mut local_write).await;
        let _ = local_write.shutdown().await;
        r
    };
    let local_to_tunnel = async {
        let r = tokio::io::copy(&mut local_read, &mut quic_send).await;
        let _ = quic_send.finish();
        r
    };

    let (t2l, l2t) = tokio::join!(tunnel_to_local, local_to_tunnel);
    tracing::debug!(
        down_bytes = t2l.unwrap_or(0),
        up_bytes = l2t.unwrap_or(0),
        "stream complete"
    );
    Ok(())
}
