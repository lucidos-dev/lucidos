use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::remote_gateways::RemoteGateway;

pub const RELAY_PROBE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProbeKey {
    name: String,
    address: String,
    credential: [u8; 32],
    slug: String,
}

impl ProbeKey {
    pub fn new(remote: &RemoteGateway, slug: &str) -> Self {
        Self {
            name: remote.name.clone(),
            address: remote.address.clone(),
            credential: Sha256::digest(remote.credential.as_bytes()).into(),
            slug: slug.to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    pub binding: Option<String>,
    pub peer_client: bool,
}

enum Probed {
    Passed { verdict: Verdict, at: Instant },
    Failed,
}

type Slot = Arc<tokio::sync::Mutex<Option<Probed>>>;

pub struct RelayProbeCache {
    ttl: Duration,
    slots: Mutex<(u64, HashMap<ProbeKey, Slot>)>,
}

impl Default for RelayProbeCache {
    fn default() -> Self {
        Self::with_ttl(RELAY_PROBE_TTL)
    }
}

impl RelayProbeCache {
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            ttl,
            slots: Mutex::default(),
        }
    }

    pub async fn resolve<E, F>(
        &self,
        key: &ProbeKey,
        generation: u64,
        probe: impl FnOnce() -> F,
    ) -> Result<Verdict, E>
    where
        F: Future<Output = Result<Verdict, E>>,
    {
        let slot = {
            let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            if slots.0 != generation {
                *slots = (generation, HashMap::new());
            }
            slots.1.entry(key.clone()).or_default().clone()
        };
        let mut probed = slot.lock().await;
        match probed.as_ref() {
            Some(Probed::Passed { verdict, at }) if at.elapsed() < self.ttl => {
                if verdict.binding.is_some() {
                    return Ok(verdict.clone());
                }
                drop(probed);
                return probe().await;
            }
            Some(Probed::Failed) => {
                drop(probed);
                return probe().await;
            }
            _ => {}
        }
        let result = probe().await;
        *probed = Some(match &result {
            Ok(verdict) => Probed::Passed {
                verdict: verdict.clone(),
                at: Instant::now(),
            },
            Err(_) => {
                let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
                if slots.1.get(key).is_some_and(|s| Arc::ptr_eq(s, &slot)) {
                    slots.1.remove(key);
                }
                Probed::Failed
            }
        });
        result
    }

    pub fn evict(&self, key: &ProbeKey) {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        slots.1.remove(key);
    }
}

pub fn voids_verdict(status: Option<axum::http::StatusCode>) -> bool {
    status.is_none_or(|s| matches!(s.as_u16(), 401 | 403 | 404 | 426 | 503))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn key(slug: &str) -> ProbeKey {
        ProbeKey {
            name: "cloud".into(),
            address: "https://b.test".into(),
            credential: [0; 32],
            slug: slug.into(),
        }
    }

    async fn probe_count(cache: &RelayProbeCache, generation: u64, binding: Option<&str>) -> usize {
        let probes = AtomicUsize::new(0);
        let _ = cache
            .resolve::<(), _>(&key("dev"), generation, || async {
                probes.fetch_add(1, Ordering::SeqCst);
                Ok(Verdict {
                    binding: binding.map(String::from),
                    peer_client: false,
                })
            })
            .await;
        probes.load(Ordering::SeqCst)
    }

    #[tokio::test]
    async fn a_verdict_stands_for_its_ttl_and_no_longer() {
        let cache = RelayProbeCache::default();
        assert_eq!(probe_count(&cache, 0, Some("b")).await, 1);
        assert_eq!(probe_count(&cache, 0, Some("b")).await, 0);
        let expired = RelayProbeCache::with_ttl(Duration::ZERO);
        assert_eq!(probe_count(&expired, 0, Some("b")).await, 1);
        assert_eq!(probe_count(&expired, 0, Some("b")).await, 1);
    }

    #[tokio::test]
    async fn each_trigger_drops_the_verdict() {
        let cache = RelayProbeCache::default();
        probe_count(&cache, 0, Some("b")).await;
        assert_eq!(probe_count(&cache, 1, Some("b")).await, 1, "store write");
        cache.evict(&key("dev"));
        assert_eq!(probe_count(&cache, 1, Some("b")).await, 1, "evicted");
        for status in [401, 403, 404, 426, 503] {
            assert!(voids_verdict(Some(status.try_into().unwrap())), "{status}");
        }
        assert!(voids_verdict(None), "transport error");
        assert!(!voids_verdict(Some(axum::http::StatusCode::OK)));
    }

    #[tokio::test]
    async fn a_failed_or_unbound_probe_caches_nothing() {
        let cache = RelayProbeCache::default();
        let failed = || async { Err::<Verdict, _>(()) };
        assert!(cache.resolve(&key("dev"), 0, failed).await.is_err());
        assert_eq!(probe_count(&cache, 0, Some("b")).await, 1);
        cache.evict(&key("dev"));
        assert_eq!(probe_count(&cache, 0, None).await, 1);
        assert_eq!(
            probe_count(&cache, 0, None).await,
            1,
            "old peer probes per request"
        );
    }

    #[tokio::test]
    async fn concurrent_misses_share_one_probe() {
        let cache = RelayProbeCache::default();
        let (probes, key) = (AtomicUsize::new(0), key("dev"));
        let resolve = || {
            cache.resolve::<(), _>(&key, 0, || async {
                probes.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
                Ok(Verdict {
                    binding: Some("b".into()),
                    peer_client: false,
                })
            })
        };
        futures::future::join_all((0..20).map(|_| resolve())).await;
        assert_eq!(probes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn waiters_behind_a_failed_probe_probe_alone_and_leave_no_slot() {
        let cache = RelayProbeCache::default();
        let (in_flight, most, key) = (AtomicUsize::new(0), AtomicUsize::new(0), key("gone"));
        let resolve = || {
            cache.resolve::<(), _>(&key, 0, || async {
                most.fetch_max(
                    in_flight.fetch_add(1, Ordering::SeqCst) + 1,
                    Ordering::SeqCst,
                );
                for _ in 0..3 {
                    tokio::task::yield_now().await;
                }
                in_flight.fetch_sub(1, Ordering::SeqCst);
                Err(())
            })
        };
        futures::future::join_all((0..20).map(|_| resolve())).await;
        assert!(most.load(Ordering::SeqCst) > 1, "waiters must not queue");
        let slots = cache.slots.lock().unwrap();
        assert!(slots.1.is_empty(), "a failed key keeps no slot");
    }
}
