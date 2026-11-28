pub mod op;

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use sqlx::PgPool;
use tokio::time::Instant;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};

const SCHEME: &str = "op://";

pub const FRESH_FOR: Duration = Duration::from_secs(10 * 60);

pub const STALE_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SecretReference(String);

impl SecretReference {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let Some(path) = raw.strip_prefix(SCHEME) else {
            return Err(format!("a secret reference starts with {SCHEME}"));
        };
        let segments: Vec<&str> = path.split('/').collect();
        let well_formed = matches!(segments.len(), 3 | 4)
            && segments.iter().all(|segment| {
                !segment.trim().is_empty() && !segment.chars().any(char::is_control)
            });
        if !well_formed {
            return Err(format!(
                "'{raw}' is not a secret reference. Write op://vault/item/field, \
                 or op://vault/item/section/field"
            ));
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretReference({})", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretField {
    Stored(String),
    Reference(SecretReference),
}

impl SecretField {
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.starts_with(SCHEME) {
            SecretReference::parse(raw).map(Self::Reference)
        } else {
            Ok(Self::Stored(raw.to_string()))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveFailure {
    Transport(String),
    /// INVARIANT: 1Password answered no: the item or field is gone, or access is denied.
    /// No cached value may outlive it.
    Definitive(String),
}

impl std::fmt::Display for ResolveFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(reason) | Self::Definitive(reason) => f.write_str(reason),
        }
    }
}

#[async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve(&self, reference: &SecretReference) -> Result<String, ResolveFailure>;

    async fn record_outcome(&self, _service_name: &str, _failure: Option<&str>) {}
}

struct CachedValue {
    value: String,
    fetched_at: Instant,
}

#[derive(Default)]
pub struct ReferenceCache {
    values: Mutex<HashMap<SecretReference, CachedValue>>,
    /// INVARIANT: Bumped by [`Self::clear`]. A resolve that started before a clear keeps
    /// its value out of the cache, since it may have read under the old
    /// service account.
    generation: std::sync::atomic::AtomicU64,
}

impl ReferenceCache {
    pub async fn resolve<R: SecretResolver + ?Sized>(
        &self,
        inner: &R,
        reference: &SecretReference,
    ) -> Result<String, ResolveFailure> {
        if let Some(value) = self.younger_than(reference, FRESH_FOR) {
            return Ok(value);
        }
        let started = self.generation.load(std::sync::atomic::Ordering::SeqCst);
        match inner.resolve(reference).await {
            Ok(value) => {
                let mut values = self.lock();
                if self.generation.load(std::sync::atomic::Ordering::SeqCst) == started {
                    values.insert(
                        reference.clone(),
                        CachedValue {
                            value: value.clone(),
                            fetched_at: Instant::now(),
                        },
                    );
                }
                Ok(value)
            }
            Err(failure @ ResolveFailure::Transport(_)) => {
                self.younger_than(reference, STALE_LIMIT).ok_or(failure)
            }
            Err(failure @ ResolveFailure::Definitive(_)) => {
                self.lock().remove(reference);
                Err(failure)
            }
        }
    }

    pub fn clear(&self) {
        let mut values = self.lock();
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        values.clear();
    }

    fn younger_than(&self, reference: &SecretReference, age: Duration) -> Option<String> {
        self.lock()
            .get(reference)
            .filter(|cached| cached.fetched_at.elapsed() < age)
            .map(|cached| cached.value.clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SecretReference, CachedValue>> {
        self.values.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Cached<'a, R> {
    cache: &'a ReferenceCache,
    inner: R,
}

impl<'a, R> Cached<'a, R> {
    pub fn new(cache: &'a ReferenceCache, inner: R) -> Self {
        Self { cache, inner }
    }
}

#[async_trait]
impl<R: SecretResolver> SecretResolver for Cached<'_, R> {
    async fn resolve(&self, reference: &SecretReference) -> Result<String, ResolveFailure> {
        self.cache.resolve(&self.inner, reference).await
    }

    async fn record_outcome(&self, service_name: &str, failure: Option<&str>) {
        self.inner.record_outcome(service_name, failure).await;
    }
}

static CACHE: LazyLock<ReferenceCache> = LazyLock::new(ReferenceCache::default);

pub fn engine_resolver(pool: &PgPool) -> Cached<'static, op::OpResolver> {
    Cached::new(&CACHE, op::OpResolver::new(pool.clone()))
}

/// INVARIANT: Forget every resolved value. A credential write calls this, so a changed
/// reference or service account is read afresh.
pub fn forget_resolved_values() {
    CACHE.clear();
}

static NOTICE_BUS: OnceLock<EventBus> = OnceLock::new();

static OUTAGES: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

pub fn install_notice_bus(bus: &EventBus) {
    NOTICE_BUS.get_or_init(|| bus.clone());
}

pub(crate) async fn notify_outage(service_name: &str, failure: Option<&str>) {
    let newly_failing = {
        let mut outages = OUTAGES.lock().unwrap_or_else(|e| e.into_inner());
        match failure {
            Some(_) => outages.insert(service_name.to_string()),
            None => {
                outages.remove(service_name);
                false
            }
        }
    };
    let (Some(reason), true, Some(bus)) = (failure, newly_failing, NOTICE_BUS.get()) else {
        return;
    };
    bus.emit_or_log(
        BusEvent::System(SystemEvent::NotificationCreated {
            id: uuid::Uuid::new_v4().to_string(),
            title: format!("1Password could not supply {service_name}"),
            message: format!(
                "Lucidos could not read the credential {service_name} from 1Password, \
                 so whatever uses it fails until this is fixed. {reason}"
            ),
            task_id: None,
            app_id: None,
            thread_id: None,
            event_id: None,
            tap: Default::default(),
            actor: None,
            audience: None,
            push_group: None,
        }),
        "[SecretReference] NotificationCreated",
    )
    .await;
}

#[cfg(any(test, feature = "test-support"))]
#[path = "mod_tests.rs"]
pub mod tests;
