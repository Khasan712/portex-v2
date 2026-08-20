use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use portex_common::{Accept, Frame, Hello, Reject, PROTOCOL_VERSION};
use portex_common::frame::FrameType;
use quinn::{ClientConfig, Endpoint, TransportConfig};

use crate::args::HttpOpts;
use crate::config;
use crate::local;

pub async fn run(opts: HttpOpts) -> anyhow::Result<()> {
    let token = config::resolve_token(opts.token.clone())?;

    let (host, port) = parse_host_port(&opts.server)?;
    let server_addr = (host.as_str(), port)
        .to_socket_addrs()?
        .next()
        .with_context(|| format!("could not resolve {}", opts.server))?;

    // Bind the local socket in the same address family the gateway resolved
    // to. A dual-stack host with an AAAA record resolves to IPv6 first, and a
    // v4-bound socket cannot reach it.
    let bind_addr: std::net::SocketAddr = if server_addr.is_ipv6() {
        "[::]:0".parse()?
    } else {
        "0.0.0.0:0".parse()?
    };
    let mut endpoint = Endpoint::client(bind_addr)?;
    endpoint.set_default_client_config(client_config(opts.insecure)?);

    tracing::info!(server = %opts.server, subdomain = %opts.subdomain, "connecting");
    let conn = endpoint.connect(server_addr, &host)?.await?;

    let (mut send, mut recv) = conn.open_bi().await?;
    let hello = Hello {
        version: PROTOCOL_VERSION,
        subdomain: opts.subdomain.clone(),
        auth_token: token.into_bytes(),
    };
    hello.into_frame()?.write_to(&mut send).await?;

    let reply = match Frame::read_from(&mut recv).await {
        Ok(f) => f,
        Err(err) => anyhow::bail!(
            "server closed the connection before replying ({err}). \
             Likely causes: invalid auth token, subdomain not reserved for your account, \
             subdomain already in use, or server unreachable."
        ),
    };
    match reply.ty {
        FrameType::Accept => {
            let accept = Accept::from_frame(reply)?;
            println!(
                "✓ tunneling https://{}.portex.live → http://127.0.0.1:{}",
                accept.assigned_subdomain, opts.port
            );
        }
        FrameType::Reject => {
            let reject = Reject::from_frame(reply)?;
            anyhow::bail!(
                "server rejected tunnel: {} ({:?})",
                reject.message,
                reject.reason
            );
        }
        other => anyhow::bail!("unexpected reply: {:?}", other),
    }

    let conn = Arc::new(conn);
    loop {
        tokio::select! {
            incoming = conn.accept_bi() => match incoming {
                Ok((send, recv)) => {
                    let local_port = opts.port;
                    tokio::spawn(async move {
                        if let Err(err) = local::pipe_stream(send, recv, local_port).await {
                            tracing::warn!(?err, "stream forward ended with error");
                        }
                    });
                }
                Err(err) => {
                    tracing::warn!(?err, "tunnel closed");
                    return Err(err.into());
                }
            },
            _ = shutdown_signal() => {
                // Tell the gateway we are going away. Without this the
                // subdomain stays claimed until the 60s idle timeout expires,
                // so restarting a tunnel right after Ctrl-C fails with
                // SubdomainTaken and the slot counts against server capacity.
                println!("\nclosing tunnel…");
                conn.close(0u32.into(), b"client shutting down");
                let _ = tokio::time::timeout(Duration::from_secs(3), endpoint.wait_idle()).await;
                return Ok(());
            }
        }
    }
}

/// Resolves on Ctrl-C, or on SIGTERM where the platform has it.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = &mut ctrl_c => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = ctrl_c.await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn parse_host_port(s: &str) -> anyhow::Result<(String, u16)> {
    let (host, port) = s.rsplit_once(':').context("server must be host:port")?;
    let port: u16 = port.parse().context("invalid port")?;
    Ok((host.to_string(), port))
}

fn client_config(insecure: bool) -> anyhow::Result<ClientConfig> {
    let roots = root_store()?;

    let mut crypto = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"portex/1".to_vec()];

    if insecure {
        tracing::warn!("TLS verification disabled");
        crypto
            .dangerous()
            .set_certificate_verifier(Arc::new(insecure::NoVerify));
    }

    let quic = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?;
    let mut cfg = ClientConfig::new(Arc::new(quic));
    cfg.transport_config(Arc::new(transport_config()));
    Ok(cfg)
}

fn transport_config() -> TransportConfig {
    let mut t = TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(10)));
    t.max_idle_timeout(Some(Duration::from_secs(60).try_into().unwrap()));
    t
}

/// Build the trust anchors used to verify the gateway's certificate.
///
/// The OS trust store comes first, so a self-hosted gateway using a private
/// or corporate CA works without extra flags. If the platform store is
/// unavailable or empty — a slim container with no `ca-certificates`, for
/// instance — we fall back to the Mozilla root program bundled into the
/// binary, so the CLI still verifies rather than silently trusting nothing.
fn root_store() -> anyhow::Result<rustls::RootCertStore> {
    let mut roots = rustls::RootCertStore::empty();

    let native = rustls_native_certs::load_native_certs();
    for err in &native.errors {
        tracing::debug!(%err, "could not read part of the OS trust store");
    }
    for cert in native.certs {
        roots.add(cert).ok();
    }

    if roots.is_empty() {
        tracing::warn!(
            "OS trust store is empty or unreadable — falling back to the bundled Mozilla roots"
        );
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }

    if roots.is_empty() {
        anyhow::bail!(
            "no trust anchors available: cannot verify the gateway certificate. \
             Install your system's ca-certificates package, or pass --insecure \
             to skip verification (development only)."
        );
    }

    tracing::debug!(anchors = roots.len(), "loaded trust anchors");
    Ok(roots)
}

mod insecure {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, SignatureScheme};

    #[derive(Debug)]
    pub struct NoVerify;

    impl ServerCertVerifier for NoVerify {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            vec![
                SignatureScheme::RSA_PKCS1_SHA256,
                SignatureScheme::RSA_PSS_SHA256,
                SignatureScheme::ECDSA_NISTP256_SHA256,
                SignatureScheme::ED25519,
            ]
        }
    }
}
