//! *Secret references*: a credential field that holds `op://vault/item/field`
//! instead of the secret, read from 1Password when the credential is used.
//!
//! A reference lives inside the sealed `auth_value`, so storage and the agent
//! role treat it like any stored secret. The resolved value lives only in this
//! module's cache, in memory: never on disk, in a row, in an event or a log.
//!
//! Resolution sits behind [`SecretResolver`]. The engine's resolver is
//! [`engine_resolver`]: the `op` command line tool ([`op::OpResolver`])
//! behind the shared [`ReferenceCache`].

pub mod op;

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use sqlx::PgPool;
use tokio::time::Instant;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};

/// The scheme every reference starts with.
const SCHEME: &str = "op://";

/// How long a resolved value serves without asking 1Password again.
pub const FRESH_FOR: Duration = Duration::from_secs(10 * 60);

/// How long the last good value serves while 1Password cannot be reached.
pub const STALE_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);

/// A validated `op://vault/item/[section/]field`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SecretReference(String);

impl SecretReference {
    /// Parse a reference, refusing anything `op read` could not address.
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

/// One secret field of a credential: the value itself, or where 1Password
/// keeps it. Never both, and never a resolved value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretField {
    Stored(String),
    Reference(SecretReference),
}

impl SecretField {
    /// Read a stored field. A value starting with `op://` is a reference, and
    /// a malformed one is refused rather than sent as a secret.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.starts_with(SCHEME) {
            SecretReference::parse(raw).map(Self::Reference)
        } else {
            Ok(Self::Stored(raw.to_string()))
        }
    }
}

/// Why a reference gave no value. The cache branches on the two kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveFailure {
    /// 1Password could not be asked: `op` missing, a timeout, no network.
    /// The last good value may still serve.
    Transport(String),
    /// 1Password answered no: the item or field is gone, or access is denied.
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

/// Reads one reference's value. A failure's reason never holds a value.
#[async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve(&self, reference: &SecretReference) -> Result<String, ResolveFailure>;

    /// Hear how reading a referenced credential went. The engine's resolver
    /// tells the user about an outage; any other ignores it.
    async fn record_outcome(&self, _service_name: &str, _failure: Option<&str>) {}
}

struct CachedValue {
    value: String,
    fetched_at: Instant,
}

/// Resolved values by reference, held in memory only.
///
/// - A value younger than [`FRESH_FOR`] serves without a resolve.
/// - A transport failure serves the last good value up to [`STALE_LIMIT`].
/// - A definitive failure drops the value, so a deleted secret ends at once.
#[derive(Default)]
pub struct ReferenceCache {
    values: Mutex<HashMap<SecretReference, CachedValue>>,
    /// Bumped by [`Self::clear`]. A resolve that started before a clear keeps
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

    /// Drop every value, so the next use asks 1Password again.
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
        // A panic mid-insert leaves at worst a stale entry, never a torn one.
        self.values.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A resolver behind a cache.
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

/// The engine's one cache. A restart empties it.
static CACHE: LazyLock<ReferenceCache> = LazyLock::new(ReferenceCache::default);

/// The engine's resolver: `op`, with this workspace's service account,
/// behind the shared cache.
pub fn engine_resolver(pool: &PgPool) -> Cached<'static, op::OpResolver> {
    Cached::new(&CACHE, op::OpResolver::new(pool.clone()))
}

/// Forget every resolved value. A credential write calls this, so a changed
/// reference or service account is read afresh.
pub fn forget_resolved_values() {
    CACHE.clear();
}

/// The bus outage notices go out on, set once the engine has one.
static NOTICE_BUS: OnceLock<EventBus> = OnceLock::new();

/// Credentials whose reference failed on their last use.
static OUTAGES: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// Let outage notices reach the user. Engine construction calls this; the
/// first engine in the process keeps the bus.
pub fn install_notice_bus(bus: &EventBus) {
    NOTICE_BUS.get_or_init(|| bus.clone());
}

/// Record the outcome of reading a referenced credential, and tell the user
/// once per outage: on the first failure after a success.
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
        }),
        "[SecretReference] NotificationCreated",
    )
    .await;
}

#[cfg(test)]
#[path = "mod_tests.rs"]
pub(crate) mod tests;
