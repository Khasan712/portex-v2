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

    /// Where to send apex + `www.` traffic — the Django control plane, as
    /// `host:port`. Without it the landing page, dashboard, admin and
    /// /install/ are unreachable on the public domain.
    #[arg(long, env = "PORTEX_APEX_UPSTREAM")]
    pub apex_upstream: Option<String>,

    /// Redis URL for token + subdomain reservation lookups. Required unless
    /// --allow-insecure-auth is passed.
    #[arg(long, env = "PORTEX_REDIS_URL")]
    pub redis_url: Option<String>,

    /// Run without Redis, accepting ANY token for ANY subdomain. Local
    /// development only — never set this on a reachable host.
    #[arg(long, env = "PORTEX_ALLOW_INSECURE_AUTH", default_value_t = false)]
    pub allow_insecure_auth: bool,

    /// How often to re-check live tunnels against the auth backend, in
    /// seconds. Bounds how long a revoked token keeps working. 0 disables.
    #[arg(long, env = "PORTEX_AUTH_REVALIDATE_SECS", default_value_t = 30)]
    pub auth_revalidate_secs: u64,

    /// Maximum concurrent tunnels. Further handshakes are rejected with
    /// ServerFull rather than degrading everyone already connected.
    /// 0 means unlimited.
    #[arg(long, env = "PORTEX_MAX_TUNNELS", default_value_t = 10_000)]
    pub max_tunnels: usize,

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
