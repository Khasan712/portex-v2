use std::collections::HashMap;
use std::sync::Arc;

use quinn::Connection;
use tokio::sync::RwLock;

/// Tracks which subdomain is currently bound to which QUIC connection.
///
/// MVP uses an in-process map. Multi-instance deployments will replace this
/// with a Redis-backed routing layer + per-node sharding.
#[derive(Default)]
pub struct Registry {
    inner: RwLock<HashMap<String, Arc<Connection>>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, subdomain: String, conn: Arc<Connection>) -> Option<Arc<Connection>> {
        self.inner.write().await.insert(subdomain, conn)
    }

    pub async fn remove(&self, subdomain: &str) -> Option<Arc<Connection>> {
        self.inner.write().await.remove(subdomain)
    }

    pub async fn lookup(&self, subdomain: &str) -> Option<Arc<Connection>> {
        self.inner.read().await.get(subdomain).cloned()
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }
}
