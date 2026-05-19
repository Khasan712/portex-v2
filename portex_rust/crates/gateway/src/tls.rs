use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use arc_swap::ArcSwap;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::sync::Notify;

/// ALPN identifiers we negotiate for each listener.
pub const ALPN_QUIC_TUNNEL: &[u8] = b"portex/1";
pub const ALPN_HTTP_1_1: &[u8] = b"http/1.1";

/// Pre-built configs for both listeners. Swapped atomically on cert renewal.
pub struct Reloadable {
    pub https: ArcSwap<rustls::ServerConfig>,
    pub quic: ArcSwap<quinn::ServerConfig>,
    /// Notified after a successful swap. Tunnel listener subscribes to push
    /// the new QUIC config into its endpoint.
    pub updated: Notify,
}

impl Reloadable {
    pub fn new(https: Arc<rustls::ServerConfig>, quic: quinn::ServerConfig) -> Self {
        Self {
            https: ArcSwap::from(https),
            quic: ArcSwap::from_pointee(quic),
            updated: Notify::new(),
        }
    }

    pub fn swap(&self, https: Arc<rustls::ServerConfig>, quic: quinn::ServerConfig) {
        self.https.store(https);
        self.quic.store(Arc::new(quic));
        self.updated.notify_waiters();
        tracing::info!("tls: cert reloaded");
    }
}

/// Build a Reloadable from cert + key files (or self-signed for dev).
pub fn build_reloadable(
    cert_path: Option<&Path>,
    key_path: Option<&Path>,
) -> anyhow::Result<Arc<Reloadable>> {
    let (certs, key) = match (cert_path, key_path) {
        (Some(c), Some(k)) => load_pem(c, k)?,
        _ => {
            tracing::warn!("No TLS cert/key provided — generating self-signed cert (dev only)");
            generate_self_signed()?
        }
    };
    let (https, quic) = build_configs(&certs, &key)?;
    Ok(Arc::new(Reloadable::new(https, quic)))
}

/// Reload from disk and atomically swap configs in place.
pub fn reload_from_disk(
    state: &Reloadable,
    cert_path: &Path,
    key_path: &Path,
) -> anyhow::Result<()> {
    let (certs, key) = load_pem(cert_path, key_path)?;
    let (https, quic) = build_configs(&certs, &key)?;
    state.swap(https, quic);
    Ok(())
}

fn build_configs(
    certs: &[CertificateDer<'static>],
    key: &PrivateKeyDer<'static>,
) -> anyhow::Result<(Arc<rustls::ServerConfig>, quinn::ServerConfig)> {
    let mut https_cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs.to_vec(), key.clone_key())
        .context("rustls https ServerConfig")?;
    https_cfg.alpn_protocols = vec![ALPN_HTTP_1_1.to_vec()];

    let mut quic_rustls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs.to_vec(), key.clone_key())
        .context("rustls quic ServerConfig")?;
    quic_rustls.alpn_protocols = vec![ALPN_QUIC_TUNNEL.to_vec()];
    let quic_crypto = quinn::crypto::rustls::QuicServerConfig::try_from(quic_rustls)
        .context("QuicServerConfig")?;
    let quic_cfg = quinn::ServerConfig::with_crypto(Arc::new(quic_crypto));

    Ok((Arc::new(https_cfg), quic_cfg))
}

fn load_pem(
    cert: &Path,
    key: &Path,
) -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let cert_bytes = std::fs::read(cert).with_context(|| format!("read cert {cert:?}"))?;
    let key_bytes = std::fs::read(key).with_context(|| format!("read key {key:?}"))?;
    let certs = rustls_pemfile::certs(&mut cert_bytes.as_slice())
        .collect::<Result<Vec<_>, _>>()
        .context("parse cert chain")?;
    let key = rustls_pemfile::private_key(&mut key_bytes.as_slice())
        .context("parse private key")?
        .context("no private key found in PEM")?;
    Ok((certs, key))
}

fn generate_self_signed()
    -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)>
{
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])
        .context("generate self-signed cert")?;
    let cert_der: CertificateDer<'static> = cert.cert.der().clone();
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        cert.key_pair.serialize_der(),
    ));
    Ok((vec![cert_der], key_der))
}
