use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;

use quinn::Connection;
use tokio::sync::RwLock;

/// One live tunnel: the QUIC connection plus the credentials it was admitted
/// with, so they can be re-checked later against the auth backend.
pub struct Tunnel {
    pub conn: Arc<Connection>,
    pub user_id: String,
    pub token_hash: String,
}

/// Tracks which subdomain is currently bound to which tunnel.
///
/// Generic over the stored value so the claim/release behaviour can be unit
/// tested without standing up a real QUIC connection.
///
/// MVP uses an in-process map. Multi-instance deployments will replace this
/// with a Redis-backed routing layer + per-node sharding.
pub struct Registry<T = Tunnel> {
    inner: RwLock<HashMap<String, Arc<T>>>,
    /// Maximum concurrent tunnels; 0 means unlimited.
    capacity: usize,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

/// Outcome of trying to claim a subdomain.
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    Ok,
    /// Another connection already holds this subdomain.
    Taken,
    /// The gateway is at its configured tunnel capacity.
    Full,
}

impl<T> Registry<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self { inner: RwLock::new(HashMap::new()), capacity }
    }

    /// Claim `subdomain`, but only if it is free. Returns `false` if someone
    /// already holds it.
    ///
    /// The check and the insert happen under a single write lock. Doing them
    /// as separate `lookup` + `insert` calls let two concurrent handshakes
    /// both pass the check, after which the loser's disconnect would evict
    /// the winner's entry and silently blackhole a live tunnel.
    pub async fn insert_if_absent(&self, subdomain: String, value: Arc<T>) -> Claim {
        let mut map = self.inner.write().await;
        if self.capacity > 0 && map.len() >= self.capacity {
            return Claim::Full;
        }
        match map.entry(subdomain) {
            Entry::Occupied(_) => Claim::Taken,
            Entry::Vacant(slot) => {
                slot.insert(value);
                Claim::Ok
            }
        }
    }

    /// Release `subdomain`, but only if it still points at `value`. A tunnel
    /// that never owned the entry must not evict the one that does.
    pub async fn remove_if(&self, subdomain: &str, value: &Arc<T>) -> bool {
        let mut map = self.inner.write().await;
        match map.get(subdomain) {
            Some(current) if Arc::ptr_eq(current, value) => {
                map.remove(subdomain);
                true
            }
            _ => false,
        }
    }

    pub async fn lookup(&self, subdomain: &str) -> Option<Arc<T>> {
        self.inner.read().await.get(subdomain).cloned()
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Point-in-time copy of every registered tunnel, for background sweeps
    /// that must not hold the lock while doing I/O.
    pub async fn snapshot(&self) -> Vec<(String, Arc<T>)> {
        self.inner
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(n: u32) -> Arc<u32> {
        Arc::new(n)
    }

    #[tokio::test]
    async fn first_claim_wins_and_second_is_refused() {
        let reg: Registry<u32> = Registry::default();
        let first = val(1);
        let second = val(2);
        assert_eq!(reg.insert_if_absent("acme".into(), first.clone()).await, Claim::Ok);
        assert_eq!(reg.insert_if_absent("acme".into(), second.clone()).await, Claim::Taken);
        assert!(Arc::ptr_eq(&reg.lookup("acme").await.unwrap(), &first));
    }

    #[tokio::test]
    async fn capacity_is_enforced_and_frees_up_again() {
        let reg: Registry<u32> = Registry::with_capacity(2);
        let a = val(1);
        assert_eq!(reg.insert_if_absent("a".into(), a.clone()).await, Claim::Ok);
        assert_eq!(reg.insert_if_absent("b".into(), val(2)).await, Claim::Ok);
        assert_eq!(reg.insert_if_absent("c".into(), val(3)).await, Claim::Full);

        reg.remove_if("a", &a).await;
        assert_eq!(reg.insert_if_absent("c".into(), val(3)).await, Claim::Ok);
    }

    #[tokio::test]
    async fn zero_capacity_means_unlimited() {
        let reg: Registry<u32> = Registry::with_capacity(0);
        for n in 0..50u32 {
            assert_eq!(reg.insert_if_absent(n.to_string(), val(n)).await, Claim::Ok);
        }
        assert_eq!(reg.len().await, 50);
    }

    #[tokio::test]
    async fn loser_cannot_evict_the_winner() {
        let reg: Registry<u32> = Registry::default();
        let winner = val(1);
        let loser = val(2);
        reg.insert_if_absent("acme".into(), winner.clone()).await;

        // The refused connection tears down and tries to clean up after
        // itself — it must not take the live tunnel with it.
        assert!(!reg.remove_if("acme", &loser).await);
        assert!(Arc::ptr_eq(&reg.lookup("acme").await.unwrap(), &winner));

        assert!(reg.remove_if("acme", &winner).await);
        assert!(reg.lookup("acme").await.is_none());
    }

    #[tokio::test]
    async fn concurrent_claims_produce_exactly_one_winner() {
        let reg: Arc<Registry<u32>> = Arc::new(Registry::default());
        let mut tasks = Vec::new();
        for n in 0..64u32 {
            let reg = reg.clone();
            tasks.push(tokio::spawn(async move {
                reg.insert_if_absent("acme".into(), Arc::new(n)).await
            }));
        }
        let mut winners = 0;
        for t in tasks {
            if t.await.unwrap() == Claim::Ok {
                winners += 1;
            }
        }
        assert_eq!(winners, 1, "exactly one handshake may claim a subdomain");
        assert_eq!(reg.len().await, 1);
    }
}
