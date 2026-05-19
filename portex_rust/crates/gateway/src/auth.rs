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
            None => {
                tracing::warn!("PORTEX_REDIS_URL not set — auth disabled (dev mode)");
                None
            }
        };
        Ok(Self { redis })
    }

    pub async fn authorize(&self, token: &[u8], subdomain: &str) -> Result<UserId, AuthError> {
        let Some(redis) = self.redis.clone() else {
            return Ok(UserId("dev".into()));
        };
        if token.is_empty() {
            return Err(AuthError::MissingToken);
        }
        let token_hash = hash_token(token);
        let mut conn = redis;
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
            Some(owner) if owner == user_id => Ok(UserId(user_id)),
            Some(_) => Err(AuthError::SubdomainTaken),
        }
    }
}

fn hash_token(token: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(token);
    hex::encode(digest)
}

#[derive(Debug, Clone)]
pub struct UserId(pub String);

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
