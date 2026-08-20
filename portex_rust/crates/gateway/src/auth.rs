use anyhow::Context;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use thiserror::Error;

use crate::config::Args;

/// Validates client auth tokens and subdomain reservations.
///
/// Source of truth lives in Django (`AuthToken`, `ReservedSubdomain` models).
/// Django writes a Redis index on token creation / subdomain reservation;
/// the gateway only reads it on the hot path.
///
/// Redis keys:
///   token:{token_hash}          → user_id (string)
///   sub:{subdomain}             → user_id (string)
pub struct Authenticator {
    redis: Option<ConnectionManager>,
}

impl Authenticator {
    pub async fn from_args(args: &Args) -> anyhow::Result<Self> {
        let redis = match &args.redis_url {
            Some(url) => {
                let client = redis::Client::open(url.as_str())
                    .context("invalid PORTEX_REDIS_URL")?;
                let manager = ConnectionManager::new(client).await
                    .context("connect to Redis")?;
                Some(manager)
            }
            None if args.allow_insecure_auth => {
                tracing::warn!(
                    "PORTEX_REDIS_URL not set and --allow-insecure-auth given — \
                     AUTH IS DISABLED: any token opens any subdomain"
                );
                None
            }
            None => anyhow::bail!(
                "PORTEX_REDIS_URL is not set. The gateway will not start without an \
                 auth backend, because it would accept any token for any subdomain. \
                 Set PORTEX_REDIS_URL, or pass --allow-insecure-auth for local development."
            ),
        };
        Ok(Self { redis })
    }

    /// Validate a plaintext token against a subdomain reservation.
    pub async fn authorize(&self, token: &[u8], subdomain: &str) -> Result<Authorized, AuthError> {
        if self.redis.is_none() {
            return Ok(Authorized { user_id: DEV_USER.into(), token_hash: String::new() });
        }
        if token.is_empty() {
            return Err(AuthError::MissingToken);
        }
        let token_hash = hash_token(token);
        let user_id = self.authorize_hash(&token_hash, subdomain).await?;
        Ok(Authorized { user_id, token_hash })
    }

    /// Same check against an already-hashed token. The revalidation sweep
    /// uses this to re-verify live tunnels without keeping plaintext tokens
    /// in memory.
    pub async fn authorize_hash(
        &self,
        token_hash: &str,
        subdomain: &str,
    ) -> Result<String, AuthError> {
        let Some(mut conn) = self.redis.clone() else {
            return Ok(DEV_USER.into());
        };
        let user_for_token: Option<String> = conn
            .get(format!("token:{token_hash}"))
            .await
            .map_err(AuthError::Backend)?;
        let user_id = user_for_token.ok_or(AuthError::InvalidToken)?;

        let user_for_sub: Option<String> = conn
            .get(format!("sub:{subdomain}"))
            .await
            .map_err(AuthError::Backend)?;
        match user_for_sub {
            None => Err(AuthError::SubdomainNotReserved),
            Some(owner) if owner == user_id => Ok(user_id),
            Some(_) => Err(AuthError::SubdomainTaken),
        }
    }

    /// Record that a token was just used, for the dashboard's "last used"
    /// column. Best-effort: the hot path must not fail over bookkeeping.
    ///
    /// The gateway never touches Postgres, so it leaves a timestamp in Redis
    /// and Django folds it into the database when it renders the dashboard.
    pub async fn mark_token_used(&self, token_hash: &str) {
        let Some(mut conn) = self.redis.clone() else { return };
        if token_hash.is_empty() {
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // TTL is a backstop: revoking a token deletes this key too, but a key
        // orphaned by a crash should not linger forever.
        let result: Result<(), redis::RedisError> = conn
            .set_ex(format!("token_used:{token_hash}"), now, TOKEN_USED_TTL_SECS)
            .await;
        if let Err(err) = result {
            tracing::debug!(?err, "could not record token usage");
        }
    }
}

/// Ninety days — comfortably longer than any realistic dashboard visit gap.
const TOKEN_USED_TTL_SECS: u64 = 90 * 24 * 3600;

/// Placeholder identity used when the gateway runs with --allow-insecure-auth.
const DEV_USER: &str = "dev";

fn hash_token(token: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(token);
    hex::encode(digest)
}

/// Who a handshake was admitted as, plus the hashed credential it presented.
#[derive(Debug, Clone)]
pub struct Authorized {
    pub user_id: String,
    /// Empty in insecure-auth mode, where there is nothing to revalidate.
    pub token_hash: String,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("missing auth token")]
    MissingToken,
    #[error("invalid auth token")]
    InvalidToken,
    #[error("subdomain not reserved for any user")]
    SubdomainNotReserved,
    #[error("subdomain reserved by a different user")]
    SubdomainTaken,
    #[error("auth backend error: {0}")]
    Backend(#[from] redis::RedisError),
}
