//! ACME wildcard cert acquisition + renewal.
//!
//! Flow on startup:
//!   1. If `state_dir/cert.pem` + `key.pem` exist AND cert expires in > 30 days,
//!      reuse them.
//!   2. Otherwise, run a fresh ACME order with DNS-01 challenges via the
//!      configured DNS provider. Persist results to `state_dir`.
//!
//! The renewal task wakes every 12 hours and re-runs step 2 when the cert is
//! within the 30-day renewal window. Listeners pick up the new cert on next
//! restart (hot reload is a planned improvement).

mod cloudflare;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use instant_acme::{
    Account, AuthorizationStatus, ChallengeType, Identifier, LetsEncrypt, NewAccount,
    NewOrder, OrderStatus,
};
use tokio::time::sleep;

use cloudflare::Cloudflare;

use crate::tls::{self, Reloadable};

const ACCOUNT_FILE: &str = "acme-account.json";
const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";
const RENEWAL_WINDOW_DAYS: i64 = 30;
const CHECK_INTERVAL: Duration = Duration::from_secs(12 * 3600);

#[derive(Clone)]
pub struct AcmeConfig {
    pub apex_domain: String,
    pub email: String,
    pub cloudflare_token: String,
    pub cloudflare_zone_id: String,
    pub state_dir: PathBuf,
    pub staging: bool,
}

impl AcmeConfig {
    pub fn from_args(args: &crate::config::Args) -> Option<Self> {
        Some(Self {
            apex_domain: args.acme_domain.clone()?,
            email: args.acme_email.clone()?,
            cloudflare_token: args.cloudflare_token.clone()?,
            cloudflare_zone_id: args.cloudflare_zone_id.clone()?,
            state_dir: args.state_dir.clone(),
            staging: args.acme_staging,
        })
    }
}

pub struct CertPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// Ensure a valid cert exists on disk; obtain it if absent or near expiry.
pub async fn bootstrap(cfg: &AcmeConfig) -> Result<CertPaths> {
    std::fs::create_dir_all(&cfg.state_dir)
        .with_context(|| format!("create state dir {:?}", cfg.state_dir))?;

    let paths = CertPaths {
        cert: cfg.state_dir.join(CERT_FILE),
        key: cfg.state_dir.join(KEY_FILE),
    };

    let cf = Cloudflare::new(cfg.cloudflare_token.clone(), cfg.cloudflare_zone_id.clone());
    cf.verify().await.context("cloudflare credentials")?;

    let days_left = existing_days_left(&paths.cert).unwrap_or(0);
    if days_left > RENEWAL_WINDOW_DAYS {
        tracing::info!(days_left, "acme: reusing on-disk cert");
        return Ok(paths);
    }

    tracing::info!(domain = %cfg.apex_domain, "acme: obtaining new wildcard cert");
    obtain_and_persist(cfg, &cf, &paths).await?;
    Ok(paths)
}

/// Background task: every 12h, renew if we're inside the renewal window.
/// After a successful renewal the shared `Reloadable` is updated and all
/// listeners pick up the new cert without restart.
pub async fn renewal_loop(cfg: AcmeConfig, paths: CertPaths, reloadable: Arc<Reloadable>) {
    loop {
        sleep(CHECK_INTERVAL).await;
        let days = existing_days_left(&paths.cert).unwrap_or(0);
        if days > RENEWAL_WINDOW_DAYS {
            tracing::debug!(days_left = days, "acme: cert still fresh");
            continue;
        }
        tracing::info!(days_left = days, "acme: renewing cert");
        let cf = Cloudflare::new(cfg.cloudflare_token.clone(), cfg.cloudflare_zone_id.clone());
        match obtain_and_persist(&cfg, &cf, &paths).await {
            Ok(()) => match tls::reload_from_disk(&reloadable, &paths.cert, &paths.key) {
                Ok(()) => tracing::info!("acme: cert renewed and reloaded"),
                Err(err) => tracing::warn!(?err, "acme: cert renewed on disk but reload failed"),
            },
            Err(err) => tracing::warn!(?err, "acme: renewal failed, will retry"),
        }
    }
}

async fn obtain_and_persist(cfg: &AcmeConfig, cf: &Cloudflare, paths: &CertPaths) -> Result<()> {
    let directory_url = if cfg.staging {
        LetsEncrypt::Staging.url().to_owned()
    } else {
        LetsEncrypt::Production.url().to_owned()
    };

    let account = load_or_create_account(cfg, &directory_url).await?;

    let identifiers = vec![
        Identifier::Dns(cfg.apex_domain.clone()),
        Identifier::Dns(format!("*.{}", cfg.apex_domain)),
    ];
    let mut order = account
        .new_order(&NewOrder { identifiers: &identifiers })
        .await
        .context("acme new_order")?;

    let authorizations = order.authorizations().await.context("acme authorizations")?;
    let mut published: Vec<String> = Vec::new();

    for authz in &authorizations {
        match authz.status {
            AuthorizationStatus::Valid => continue,
            AuthorizationStatus::Pending => {}
            other => anyhow::bail!("unexpected auth status: {other:?}"),
        }
        let challenge = authz
            .challenges
            .iter()
            .find(|c| c.r#type == ChallengeType::Dns01)
            .context("no DNS-01 challenge offered")?;

        let domain = match &authz.identifier {
            Identifier::Dns(d) => d.clone(),
        };
        let base = domain.trim_start_matches("*.");
        let txt_name = format!("_acme-challenge.{base}");
        let txt_value = order.key_authorization(challenge).dns_value();

        tracing::info!(name = %txt_name, "acme: publishing DNS-01 challenge");
        let id = cf
            .create_txt(&txt_name, &txt_value, 60)
            .await
            .with_context(|| format!("publish TXT for {domain}"))?;
        published.push(id);

        sleep(Duration::from_secs(20)).await;

        order
            .set_challenge_ready(&challenge.url)
            .await
            .context("acme set_challenge_ready")?;
    }

    let mut delay = Duration::from_secs(2);
    loop {
        let state = order.refresh().await.context("acme refresh")?;
        tracing::debug!(?state.status, "acme: order poll");
        match state.status {
            OrderStatus::Ready | OrderStatus::Valid => break,
            OrderStatus::Invalid => anyhow::bail!("order invalid: {state:?}"),
            _ => {
                sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        }
    }

    // Generate our own keypair + CSR so we keep the private key.
    let mut params = rcgen::CertificateParams::new(vec![
        cfg.apex_domain.clone(),
        format!("*.{}", cfg.apex_domain),
    ])
    .context("rcgen params")?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    let key_pair = rcgen::KeyPair::generate().context("rcgen keypair")?;
    let csr = params.serialize_request(&key_pair).context("rcgen csr")?;
    order.finalize(csr.der()).await.context("acme finalize")?;

    let cert_chain_pem = loop {
        match order.certificate().await.context("acme certificate")? {
            Some(pem) => break pem,
            None => sleep(Duration::from_secs(2)).await,
        }
    };

    write_cert_and_key(&paths.cert, &paths.key, &cert_chain_pem, &key_pair.serialize_pem())?;

    for id in published {
        if let Err(err) = cf.delete_record(&id).await {
            tracing::warn!(?err, %id, "acme: failed to delete TXT (non-fatal)");
        }
    }
    Ok(())
}

async fn load_or_create_account(cfg: &AcmeConfig, directory_url: &str) -> Result<Account> {
    let path = cfg.state_dir.join(ACCOUNT_FILE);
    if path.exists() {
        let raw = std::fs::read_to_string(&path).context("read acme account")?;
        let creds: instant_acme::AccountCredentials =
            serde_json::from_str(&raw).context("parse acme account")?;
        return Account::from_credentials(creds).await.context("load acme account");
    }
    let (account, creds) = Account::create(
        &NewAccount {
            contact: &[&format!("mailto:{}", cfg.email)],
            terms_of_service_agreed: true,
            only_return_existing: false,
        },
        directory_url,
        None,
    )
    .await
    .context("create acme account")?;
    let serialized = serde_json::to_string(&creds).context("serialize acme account")?;
    std::fs::write(&path, serialized).context("persist acme account")?;
    Ok(account)
}

fn existing_days_left(cert_path: &Path) -> Result<i64> {
    let pem = std::fs::read_to_string(cert_path)?;
    let mut bytes = pem.as_bytes();
    let cert_der = rustls_pemfile::certs(&mut bytes)
        .next()
        .context("no cert in PEM")??;
    let (_, parsed) =
        x509_parser::parse_x509_certificate(cert_der.as_ref()).context("parse x509")?;
    let ts = parsed.validity().not_after.timestamp();
    let not_after = time::OffsetDateTime::from_unix_timestamp(ts)?;
    Ok((not_after - time::OffsetDateTime::now_utc()).whole_days())
}

fn write_cert_and_key(
    cert_path: &Path,
    key_path: &Path,
    cert_chain_pem: &str,
    key_pem: &str,
) -> Result<()> {
    use std::fs;
    fs::write(cert_path, cert_chain_pem).with_context(|| format!("write {cert_path:?}"))?;
    fs::write(key_path, key_pem).with_context(|| format!("write {key_path:?}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(key_path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
