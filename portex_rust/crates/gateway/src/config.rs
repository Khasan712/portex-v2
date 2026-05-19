use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser, Clone)]
#[command(name = "portex-gateway", version, about = "Portex tunnel gateway")]
pub struct Args {
    /// Public HTTP listener (e.g. 0.0.0.0:80 or 0.0.0.0:443 when TLS-terminated upstream).
    #[arg(long, env = "PORTEX_HTTP_ADDR", default_value = "0.0.0.0:8080")]
    pub http_addr: SocketAddr,

    /// Optional HTTPS listener — requires --tls-cert and --tls-key.
    #[arg(long, env = "PORTEX_HTTPS_ADDR")]
    pub https_addr: Option<SocketAddr>,

    /// QUIC tunnel listener (UDP).
    #[arg(long, env = "PORTEX_TUNNEL_ADDR", default_value = "0.0.0.0:4443")]
    pub tunnel_addr: SocketAddr,

    /// Optional metrics endpoint (Prometheus format). Bind it on a private
    /// network only — there is no auth on /metrics.
    #[arg(long, env = "PORTEX_METRICS_ADDR")]
    pub metrics_addr: Option<SocketAddr>,

    /// Apex domain used to strip the subdomain off the Host header.
    /// Example: `portex.live` → `acme.portex.live` resolves to subdomain `acme`.
    #[arg(long, env = "PORTEX_BASE_DOMAIN", default_value = "portex.live")]
    pub base_domain: String,

    /// PEM certificate for QUIC (and HTTPS later).
    #[arg(long, env = "PORTEX_TLS_CERT")]
    pub tls_cert: Option<PathBuf>,

    /// PEM private key for QUIC (and HTTPS later).
    #[arg(long, env = "PORTEX_TLS_KEY")]
    pub tls_key: Option<PathBuf>,

    /// Redis URL for token + subdomain reservation lookups. If unset, auth is disabled (dev only).
    #[arg(long, env = "PORTEX_REDIS_URL")]
    pub redis_url: Option<String>,

    /// Apex domain the ACME wildcard cert covers (e.g. `portex.live`).
    /// When set, the gateway acquires/renews a cert for `*.{domain}` + `{domain}`.
    #[arg(long, env = "PORTEX_ACME_DOMAIN")]
    pub acme_domain: Option<String>,

    /// Email registered on the ACME account.
    #[arg(long, env = "PORTEX_ACME_EMAIL")]
    pub acme_email: Option<String>,

    /// Cloudflare API token with Zone:DNS:Edit on the apex zone.
    #[arg(long, env = "CLOUDFLARE_API_TOKEN")]
    pub cloudflare_token: Option<String>,

    /// Cloudflare zone ID (uuid-ish) of the apex domain.
    #[arg(long, env = "CLOUDFLARE_ZONE_ID")]
    pub cloudflare_zone_id: Option<String>,

    /// Directory where ACME state (account key, cert, key) is persisted.
    #[arg(long, env = "PORTEX_STATE_DIR", default_value = "/var/lib/portex")]
    pub state_dir: PathBuf,

    /// Use the Let's Encrypt staging endpoint (no rate limits, untrusted certs).
    /// Recommended while testing.
    #[arg(long, env = "PORTEX_ACME_STAGING", default_value_t = false)]
    pub acme_staging: bool,
}
