//! The **continue key** (ADR 0623): an opaque string a trigger builds from a
//! fire's payload, so later fires about the same thing reach the same thread.
//!
//! The engine never parses a key and never names a source. A key spec lists
//! payload paths in the syntax trigger conditions use, and the key is the JSON
//! array of the values they resolve to. `system-knowhow/triggers.md`
//! § "Continue key" is the author's guide.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex as TokioMutex, OwnedMutexGuard};
use uuid::Uuid;

use crate::core::event_subscription::condition;

/// The most parts a spec may have, and the most paths one part may try.
const MAX_PARTS: usize = 8;
const MAX_PATHS_PER_PART: usize = 8;

/// One part of a key: a single payload path, or alternatives tried in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContinueKeyPart {
    Path(String),
    FirstOf(Vec<String>),
}

impl ContinueKeyPart {
    fn paths(&self) -> &[String] {
        match self {
            Self::Path(path) => std::slice::from_ref(path),
            Self::FirstOf(paths) => paths,
        }
    }

    /// The part's value: the first path that resolves to a non-empty scalar.
    fn value_in(&self, payload: &Value) -> Option<String> {
        self.paths()
            .iter()
            .find_map(|path| scalar_text(condition::resolve(payload, path)?))
    }
}

/// A trigger's key spec. Every part must resolve, or the fire has no key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContinueKeySpec(pub Vec<ContinueKeyPart>);

impl ContinueKeySpec {
    /// Parse a submitted spec. `null` and `[]` mean no spec, which clears one.
    pub fn parse(field: Option<&Value>) -> Result<Option<Self>, String> {
        let Some(field) = field.filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        let Some(parts) = field.as_array() else {
            return Err(SHAPE_ERROR.to_string());
        };
        if parts.is_empty() {
            return Ok(None);
        }
        if parts.len() > MAX_PARTS {
            return Err(format!("continue_key has at most {MAX_PARTS} parts"));
        }
        parts
            .iter()
            .map(parse_part)
            .collect::<Result<Vec<_>, _>>()
            .map(|parts| Some(Self(parts)))
    }

    /// Refuse a spec on a script trigger: a script fire starts no thread.
    pub fn check_run(spec: Option<&Self>, run: &crate::triggers::TriggerRun) -> Result<(), String> {
        match (spec, run) {
            (Some(_), crate::triggers::TriggerRun::Script { .. }) => Err(
                "continue_key needs an intent trigger: a script fire starts no thread to continue"
                    .to_string(),
            ),
            _ => Ok(()),
        }
    }

    /// Read a stored spec. A malformed one reads as none, so one bad event
    /// row cannot wedge the trigger registry.
    pub(crate) fn from_stored(field: Option<&Value>, trigger_id: &str) -> Option<Self> {
        Self::parse(field).unwrap_or_else(|e| {
            log!("[Triggers] Ignored continue_key on trigger {trigger_id}: {e}");
            None
        })
    }

    /// The key this payload yields, or `None` when any part has no value.
    pub fn key_for(&self, payload: &Value) -> Option<String> {
        let values = self
            .0
            .iter()
            .map(|part| part.value_in(payload))
            .collect::<Option<Vec<_>>>()?;
        serde_json::to_string(&values).ok()
    }
}

const SHAPE_ERROR: &str = "continue_key must be a list of parts. Each part is a payload \
     path, or a list of paths where the first one with a value wins. Example: \
     [\"payload.repository.full_name\", [\"payload.issue.number\", \"payload.pull_request.number\"]]";

fn parse_part(part: &Value) -> Result<ContinueKeyPart, String> {
    let path = |v: &Value| {
        v.as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| SHAPE_ERROR.to_string())
    };
    match part {
        Value::String(_) => path(part).map(ContinueKeyPart::Path),
        Value::Array(paths) if (1..=MAX_PATHS_PER_PART).contains(&paths.len()) => paths
            .iter()
            .map(path)
            .collect::<Result<Vec<_>, _>>()
            .map(ContinueKeyPart::FirstOf),
        _ => Err(SHAPE_ERROR.to_string()),
    }
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The open thread holding `key`, if any. Archiving a thread clears its key,
/// so every row that holds one is open.
pub(crate) async fn open_thread_for_key(
    pool: &sqlx::PgPool,
    key: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT thread_id FROM thread_summaries WHERE continue_key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
}

/// Exclusive use of one key, from the lookup until the new thread's
/// `TriggerStarted` records it. Two racing fires therefore never both start
/// a thread for the same key. In memory by design: a restart ends every fire
/// that holds one, and the key itself lives in `thread_summaries`.
pub(crate) struct ContinueKeyClaim {
    pub key: String,
    _guard: OwnedMutexGuard<()>,
}

pub(crate) async fn claim(key: String) -> ContinueKeyClaim {
    static LOCKS: OnceLock<std::sync::Mutex<HashMap<String, Arc<TokioMutex<()>>>>> =
        OnceLock::new();
    let lock = {
        let mut locks = LOCKS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A lock only the map still holds guards nothing.
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        locks.entry(key.clone()).or_default().clone()
    };
    ContinueKeyClaim {
        key,
        _guard: lock.lock_owned().await,
    }
}

/// Where an intent fire lands.
pub(crate) enum FireTarget {
    /// A new thread. A keyed fire carries its claim until the thread records
    /// the key.
    NewThread(Option<ContinueKeyClaim>),
    /// The open thread that already holds the fire's key.
    Continue(Uuid),
}

/// Decide where a fire lands. An unkeyed fire, or one whose payload yields no
/// key, always starts a new thread.
pub(crate) async fn resolve_fire_target(
    pool: &sqlx::PgPool,
    spec: Option<&ContinueKeySpec>,
    payload: Option<&Value>,
) -> Result<FireTarget, sqlx::Error> {
    let Some(key) = spec.zip(payload).and_then(|(spec, p)| spec.key_for(p)) else {
        return Ok(FireTarget::NewThread(None));
    };
    let claim = claim(key).await;
    Ok(match open_thread_for_key(pool, &claim.key).await? {
        Some(thread_id) => FireTarget::Continue(thread_id),
        None => FireTarget::NewThread(Some(claim)),
    })
}

#[cfg(test)]
#[path = "continue_key_tests.rs"]
mod tests;
