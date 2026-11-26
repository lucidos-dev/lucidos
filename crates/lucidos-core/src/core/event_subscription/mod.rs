pub mod condition;
pub mod frames;
pub mod known_names;

pub mod limits;

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::engine::thread_events::ThreadEvent;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventSubscription {
    pub event_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Value>,
}

impl EventSubscription {
    pub fn event_type_candidates(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.event_type.as_str()).chain(root_thread_alias(&self.event_type))
    }

    pub fn matches_type(&self, event_type: &str) -> bool {
        self.event_type_candidates().any(|name| name == event_type)
    }

    /// INVARIANT: Whether this subscription matches an incoming event.
    ///
    /// **The single predicate both dispatch paths run.** The trigger matcher and
    /// the event-wait dispatcher must agree on every (subscription, event) pair,
    /// so neither is allowed to re-implement the name comparison or reach into
    /// [`condition::evaluate`] directly. Callers still have to apply
    /// [`is_subscribable`] to the event first; this function deliberately does
    /// not, because the gate is about the *event* and belongs upstream of the
    /// whole fan-out rather than being re-run per subscription.
    pub fn matches(&self, event_type: &str, payload: &Value) -> bool {
        self.matches_type(event_type) && condition::evaluate(self.condition.as_ref(), payload)
    }

    pub fn any_matches(subs: &[EventSubscription], event_type: &str, payload: &Value) -> bool {
        subs.iter().any(|sub| sub.matches(event_type, payload))
    }

    pub fn normalize_list(
        subs: impl IntoIterator<Item = EventSubscription>,
    ) -> Vec<EventSubscription> {
        subs.into_iter()
            .filter_map(|sub| {
                let trimmed = sub.event_type.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(EventSubscription {
                        event_type: trimmed.to_string(),
                        condition: sub.condition,
                    })
                }
            })
            .collect()
    }

    pub fn from_object_entry(obj: &serde_json::Map<String, Value>) -> Option<Self> {
        let event_type = obj.get("event_type")?.as_str()?.trim();
        if event_type.is_empty() {
            return None;
        }
        let condition = obj.get("condition").filter(|v| !v.is_null()).cloned();
        Some(EventSubscription {
            event_type: event_type.to_string(),
            condition,
        })
    }
}

pub static ROOT_THREAD_ROLE_ALIASES: LazyLock<Vec<(&str, &str)>> = LazyLock::new(|| {
    ThreadEvent::LEGACY_TYPE_NAME_ALIASES
        .iter()
        .filter_map(|&legacy| {
            let suffix = legacy.strip_prefix("WorkItem")?;
            let current = ThreadEvent::RESERVED_TYPE_NAMES
                .iter()
                .copied()
                .find(|name| name.strip_prefix("RootThread") == Some(suffix))?;
            Some((legacy, current))
        })
        .collect()
});

fn root_thread_alias(event_type: &str) -> Option<&'static str> {
    ROOT_THREAD_ROLE_ALIASES
        .iter()
        .find_map(|&(old, new)| match event_type {
            name if name == old => Some(new),
            name if name == new => Some(old),
            _ => None,
        })
}

pub const THREAD_ID_KEY: &str = "thread_id";

pub fn matchable_payload(event_type: &str, payload: Value, thread_id: Option<Uuid>) -> Value {
    let mut payload = unwrap_adjacent_tag(event_type, payload);
    let Some(thread_id) = thread_id else {
        return payload;
    };
    if let Some(obj) = payload.as_object_mut() {
        obj.entry(THREAD_ID_KEY)
            .or_insert_with(|| Value::String(thread_id.to_string()));
    }
    payload
}

fn unwrap_adjacent_tag(event_type: &str, payload: Value) -> Value {
    if !crate::engine::event_bus::SystemEvent::is_persisted_type_name(event_type) {
        return payload;
    }
    let Some(obj) = payload.as_object() else {
        return payload;
    };
    if obj.get("type").and_then(Value::as_str) != Some(event_type) {
        return payload;
    }
    if !obj.keys().all(|k| k == "type" || k == "data") {
        return payload;
    }
    match obj.get("data") {
        Some(Value::Object(data)) => Value::Object(data.clone()),
        None => Value::Object(serde_json::Map::new()),
        Some(_) => payload,
    }
}

/// INVARIANT: [`matchable_payload`] for a live `BusEvent::Thread`, which is the form both
/// live consumers need: the event-wait dispatcher and the trigger matcher.
///
/// They call this rather than composing the two steps themselves, so the view
/// they match against is the same object by construction rather than by two
/// call sites happening to agree.
///
/// `meta` is the carrier's own, the value the bus persists the row with. Pass
/// it, never `EventMeta::NONE`: the stored payload holds `channel`, `actor` and
/// `request_event_id`, so dropping them here makes a condition on any of them
/// match on replay and never live.
pub fn matchable_thread_payload(
    event: &ThreadEvent,
    meta: &crate::engine::thread_events::EventMeta,
    thread_id: Uuid,
) -> Value {
    matchable_payload(event.event_type(), event.to_payload(meta), Some(thread_id))
}

/// INVARIANT: [`matchable_payload`] for a live `BusEvent::System`, the other half of the
/// pair above.
///
/// Built from `to_payload`, the same function the persisted row is written
/// from, so a `condition` behaves identically live and on replay. A system
/// event belongs to no thread, so no id is injected.
///
/// `stored_event_type`, not `event_type`, so the envelope gate compares against
/// the same name the row's column holds. They differ only for a `DomainEvent`,
/// whose payload the workspace authored and whose row is filed under the inner
/// name.
pub fn matchable_system_payload(event: &crate::engine::event_bus::SystemEvent) -> Value {
    matchable_payload(event.stored_event_type(), event.to_payload(), None)
}

pub const PER_TOKEN_STREAMING_EVENT_TYPES: &[&str] = &[
    "TextStreamed",
    "ThoughtStreamed",
    "CodingAgentTextStreamed",
    "CodingAgentThoughtStreamed",
];

pub const EVENT_WAIT_EVENT_TYPES: &[&str] = &[
    "EventWaitStarted",
    "EventWaitTimingUpdated",
    "EventWaitDelivered",
    "EventWaitExpired",
    "EventWaitCanceled",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionSurface {
    Wait,
    Trigger,
}

pub fn is_subscribable(event: &ThreadEvent) -> bool {
    !event.is_per_token_streaming()
        && !event.is_side_question_event()
        && !event.is_engine_turn_idle()
}

pub fn is_subscribable_system_event(event: &crate::engine::event_bus::SystemEvent) -> bool {
    matches!(
        event,
        crate::engine::event_bus::SystemEvent::DomainEvent { .. }
    ) || event.is_persisted()
}

const TRANSPORT_TYPE_NAMES: &[&str] = &["DomainEvent", "ThreadEvent"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionVerdict {
    KnownThreadEvent,
    KnownSystemEvent,
    UnknownName,
}

pub fn validate_subscribable_event_type(
    event_type: &str,
    surface: SubscriptionSurface,
) -> Result<SubscriptionVerdict, String> {
    let name = event_type.trim();
    if name.is_empty() {
        return Err("event_type must not be empty".to_string());
    }
    if PER_TOKEN_STREAMING_EVENT_TYPES.contains(&name) {
        return Err(format!(
            "'{name}' is a per-token streaming event, fired once per text chunk. \
             It is dropped before any subscriber sees it, so a subscription on it \
             can never match. Subscribe to the turn's outcome instead \
             (ResponseGenerated), or to a specific tool call (ToolCalled with a \
             condition on `name`)."
        ));
    }
    if ThreadEvent::SIDE_QUESTION_EVENT_TYPES.contains(&name) {
        return Err(format!(
            "'{name}' records a side question, which no agent, trigger or \
             event wait ever sees. It is dropped before any subscriber, so a \
             subscription on it can never match."
        ));
    }
    // INVARIANT: The one case the two surfaces disagree about. A trigger falls through to
    // `classify_event` below and resolves as an ordinary thread event.
    if surface == SubscriptionSurface::Wait && EVENT_WAIT_EVENT_TYPES.contains(&name) {
        return Err(format!(
            "'{name}' is part of the event-wait machinery, so waiting on it \
             would satisfy itself the moment any thread in this workspace \
             registers or resolves a wait. Wait on the event you actually care \
             about instead. A trigger may watch it."
        ));
    }
    // INVARIANT: A ThreadEvent wins over the two system checks below, because several
    // names are BOTH (`ChangeDiscarded` is a `SystemEvent` variant and a
    // `ThreadEvent` variant). The thread-scoped one is the one a subscription
    // sees. It is also the only one of the pair a `condition` can scope to a
    // thread. Keep this branch first.
    if crate::engine::thread_lifecycle::classify_event(name).is_some() {
        return Ok(SubscriptionVerdict::KnownThreadEvent);
    }
    if crate::engine::event_bus::SystemEvent::is_persisted_type_name(name) {
        return Ok(SubscriptionVerdict::KnownSystemEvent);
    }
    if TRANSPORT_TYPE_NAMES.contains(&name) {
        return Err(format!(
            "'{name}' is the wrapper the engine carries an event inside, not a \
             name any event is filed under. Subscribe to the event's own name."
        ));
    }
    if crate::engine::event_bus::SystemEvent::is_reserved_type_name(name) {
        let instead = transient_terminal_hint(name)
            .map(|hint| {
                format!(" Subscribe to {hint} instead, which is written to the event store.")
            })
            .unwrap_or_default();
        return Err(format!(
            "'{name}' is a transient system frame. The engine broadcasts it to \
             the UI without writing an event row, so a subscription on it can \
             never match.{instead}"
        ));
    }
    if ThreadEvent::LEGACY_TYPE_NAME_ALIASES.contains(&name) {
        return Err(format!(
            "'{name}' is a retired event name. Rows written before the rename \
             still read back under it, but the engine never emits it again, so \
             a subscription on it can only ever match history.{}",
            known_names::did_you_mean(name)
        ));
    }
    Ok(SubscriptionVerdict::UnknownName)
}

/// INVARIANT: Refuse a name that is a misspelling of an engine event.
///
/// **Split from [`validate_subscribable_event_type`] because it is not decidable
/// from the name alone.** A workspace that has emitted `CredentialStored` as
/// its own domain event owns that name, and no heuristic may take it away. The
/// caller checks the event store first and calls this only for a name nobody
/// here has ever emitted.
pub fn refuse_near_miss(event_type: &str) -> Result<(), String> {
    let name = event_type.trim();
    if !known_names::is_near_miss(name) {
        return Ok(());
    }
    Err(format!(
        "{name} is not an event Lucidos emits, and this workspace has never \
         emitted it either.{}",
        known_names::did_you_mean(name)
    ))
}

pub fn never_emitted_warning(event_type: &str) -> String {
    format!(
        "'{event_type}' has never been emitted in this workspace. That is fine \
         for a domain event you are about to start emitting. If you expected an \
         engine event, check the spelling with the events tool's event_types \
         action."
    )
}

const FIELD_PATH_SAMPLE_ROWS: i64 = 20;

const MAX_PATH_WARNINGS: usize = 3;

/// INVARIANT: The most recent stored payloads for `event_type`, **as the matcher will see
/// them**.
///
/// Built through [`matchable_payload`], so the envelope is unwrapped and
/// `thread_id` is injected exactly as at match time. A probe reading the raw
/// row would report a path the matcher resolves fine, which is worse than not
/// probing at all.
///
/// An unreadable store yields no samples, which yields no warning. A probe that
/// could not run is unknown, never a no.
///
/// Ordered by `(created DESC, id DESC)` to match `idx_events_type_created_id`
/// exactly, so the LIMIT streams off the index. Any recency proxy would do for
/// a payload sample, and this one is the one that is indexed: `sequence DESC`
/// reads the same but makes a million-row event type scan and sort, inside a
/// synchronous tool call.
async fn recent_matchable_payloads(pool: &sqlx::PgPool, event_type: &str) -> Vec<Value> {
    let rows: Vec<(Value, Option<Uuid>)> = match sqlx::query_as(
        "SELECT payload, thread_id FROM events \
         WHERE event_type = $1 ORDER BY created DESC, id DESC LIMIT $2",
    )
    .bind(event_type)
    .bind(FIELD_PATH_SAMPLE_ROWS)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            crate::log_legacy!("[EventSubscription] payload sample for {event_type} failed: {e}");
            return Vec::new();
        }
    };
    rows.into_iter()
        .map(|(payload, thread_id)| matchable_payload(event_type, payload, thread_id))
        .collect()
}

pub async fn uncorroborated_path_warnings(
    pool: &sqlx::PgPool,
    event_type: &str,
    condition: Option<&Value>,
) -> Vec<String> {
    let Some(condition) = condition else {
        return Vec::new();
    };
    let paths = condition::field_paths(condition);
    if paths.is_empty() {
        return Vec::new();
    }
    let samples = recent_matchable_payloads(pool, event_type).await;
    if samples.is_empty() {
        return Vec::new();
    }
    let unresolved: Vec<&String> = paths
        .iter()
        .filter(|path| !samples.iter().any(|s| condition::resolves(s, path)))
        .collect();

    let inspected = samples.len();
    let mut warnings: Vec<String> = unresolved
        .iter()
        .take(MAX_PATH_WARNINGS)
        .map(|path| {
            let elsewhere = samples
                .iter()
                .map(|s| condition::same_leaf_paths(s, path))
                .find(|paths| !paths.is_empty())
                .unwrap_or_default();
            let advice = match elsewhere.as_slice() {
                [] => "Read one with the events tool's query action, limit 1, and name a \
                       path you see there. An optional field may simply be absent from \
                       those rows."
                    .to_string(),
                [one] => format!("Did you mean '{one}'?"),
                many => format!("Did you mean one of {}?", many.join(", ")),
            };
            format!(
                "condition for '{event_type}': '{path}' resolves in none of \
                 {inspected} sampled {event_type} payload(s), so this subscription \
                 may never match. {advice}"
            )
        })
        .collect();
    if unresolved.len() > MAX_PATH_WARNINGS {
        warnings.push(format!(
            "condition for '{event_type}': {} further field path(s) are in none of them \
             either.",
            unresolved.len() - MAX_PATH_WARNINGS
        ));
    }
    warnings
}

/// INVARIANT: Has this workspace ever emitted an event by this name?
///
/// **An unreadable store answers "seen".** Every caller uses the answer to
/// decide whether to warn or to refuse, and both are worse when wrong. A false
/// alarm would tell the model a real event type is unknown. A refusal built on
/// one would block legitimate work outright.
pub async fn event_type_ever_emitted(pool: &sqlx::PgPool, event_type: &str) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM events WHERE event_type = $1)")
        .bind(event_type)
        .fetch_one(pool)
        .await
        .unwrap_or(true)
}

pub async fn check_subscriptions(
    pool: &sqlx::PgPool,
    subs: &[EventSubscription],
    surface: SubscriptionSurface,
) -> Result<Vec<String>, String> {
    let mut warnings = Vec::new();
    for sub in subs {
        let name = sub.event_type.trim();
        let verdict = validate_subscribable_event_type(name, surface)?;
        if let Some(cond) = &sub.condition {
            condition::validate(cond).map_err(|e| format!("condition for '{name}': {e}"))?;
        }
        // INVARIANT: The store is the escape hatch, so it is consulted BEFORE the
        // heuristic. A name this workspace has emitted is real by proof, and no
        // edit-distance rule may take it away.
        if verdict == SubscriptionVerdict::UnknownName && !event_type_ever_emitted(pool, name).await
        {
            refuse_near_miss(name)?;
            warnings.push(never_emitted_warning(name));
            continue;
        }
        warnings.extend(uncorroborated_path_warnings(pool, name, sub.condition.as_ref()).await);
    }
    Ok(warnings)
}

pub struct EventTypeCatalog {
    pub engine: Vec<&'static str>,
    pub workspace: Vec<String>,
}

pub async fn event_type_catalog(
    store: &crate::core::store::EventStore,
    surface: SubscriptionSurface,
) -> Result<EventTypeCatalog, String> {
    let engine = known_names::subscribable_event_type_names(surface);
    let workspace = store
        .distinct_event_types()
        .await
        .map_err(|e| format!("Failed to query event types: {e}"))?
        .into_iter()
        .filter(|seen| !engine.contains(&seen.as_str()))
        .filter(|seen| !ThreadEvent::LEGACY_TYPE_NAME_ALIASES.contains(&seen.as_str()))
        .filter(|seen| validate_subscribable_event_type(seen, surface).is_ok())
        .collect();
    Ok(EventTypeCatalog { engine, workspace })
}

fn transient_terminal_hint(name: &str) -> Option<&'static str> {
    match name {
        "SnapshotProgress" => Some("SnapshotCreated, SnapshotCaptureFailed or SnapshotChecked"),
        "BackupProgress" => Some("BackupCompleted or BackupFailed"),
        "RecommendedCleanupProgress" => {
            Some("RecommendedCleanupCompleted or RecommendedCleanupFailed")
        }
        "TreeBackfillProgressed" => Some("TreeBackfillCompleted or TreeBackfillEnded"),
        _ => None,
    }
}

/// INVARIANT: **The gate on every domain-event emit**, wherever it comes from.
///
/// Called from inside `LucidosEngine::emit_domain_event_inner`, so no caller
/// can skip it. The HTTP handlers call it too, so an app gets a 400 rather
/// than a 500. It lived only in `api/history.rs` once, and the LLM
/// `emit_event` tool went straight past it.
///
/// **A domain event may not borrow a system or thread event's name.** Two
/// distinct harms follow if it does.
///
/// `to_sse_json()` unwraps `DomainEvent` to `{"type": <event_type>, ...}`, so
/// on the wire a domain event is shaped exactly like a system frame. An
/// `emit_event("NotificationCreated", ...)` would forge a notification on every
/// connected client.
///
/// A domain event's `aggregate_id` is the event TYPE, where a thread event's is
/// a thread uuid. One row named `EventWaitStarted` therefore breaks every query
/// casting `aggregate_id::uuid` on that name, permanently, because events are
/// append-only. `event_wait::LIVE_WAITS_SQL` is the boot rebuild, and it is
/// scoped `aggregate = 'thread'` for exactly this reason.
pub fn validate_emittable_event_type(event_type: &str) -> Result<(), String> {
    if event_type.is_empty() {
        return Err("event_type is required".into());
    }
    if crate::engine::event_bus::SystemEvent::is_reserved_type_name(event_type) {
        return Err(format!(
            "event_type '{event_type}' is reserved for system events and cannot be emitted \
             as a domain event"
        ));
    }
    if crate::core::event_subscription::frames::ROUTE_FRAMES.contains(&event_type) {
        return Err(format!(
            "event_type '{event_type}' is a frame the event stream writes itself, so it \
             cannot be emitted as a domain event"
        ));
    }
    if crate::engine::thread_events::ThreadEvent::is_reserved_type_name(event_type) {
        return Err(format!(
            "event_type '{event_type}' is reserved for thread events and cannot be emitted \
             as a domain event. A domain event carries the event TYPE in `aggregate_id`, \
             where a thread event carries a thread uuid, so this row would be permanently \
             unreadable by every query that reads the name as a thread id. Pick a name of \
             your own."
        ));
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
/// INVARIANT: I8: the trigger matcher and event-wait dispatcher must agree.
/// Both use `EventSubscription::matches`. The trigger tests run this table
/// through `find_matching_event_triggers` to verify that it shares the predicate.
pub const PARITY_CASES: &[(&str, Option<&str>, &str, &str, bool)] = &[
    ("ChangeProposed", None, "ChangeProposed", r#"{}"#, true),
    ("ChangeProposed", None, "ChangeApplied", r#"{}"#, false),
    (
        "ChangeProposed",
        Some(r#"{"file_count": {"$gt": 2}}"#),
        "ChangeProposed",
        r#"{"file_count": 3}"#,
        true,
    ),
    (
        "ChangeProposed",
        Some(r#"{"file_count": {"$gt": 2}}"#),
        "ChangeProposed",
        r#"{"file_count": 1}"#,
        false,
    ),
    (
        "ResponseGenerated",
        Some(r#"{"model": {"$in": ["a", "b"]}}"#),
        "ResponseGenerated",
        r#"{"model": "b"}"#,
        true,
    ),
    (
        "ResponseGenerated",
        Some(r#"{"model": {"$in": ["a", "b"]}}"#),
        "ResponseGenerated",
        r#"{"model": "c"}"#,
        false,
    ),
    (
        "ToolCalled",
        Some(r#"{"name": "run_bash"}"#),
        "ToolCalled",
        r#"{"args": {}}"#,
        false,
    ),
    (
        "ToolCalled",
        Some(r#"{"name": "run_bash"}"#),
        "ToolCalled",
        r#"{"name": "run_bash"}"#,
        true,
    ),
    (
        "BackupFailed",
        None,
        "BackupFailed",
        r#"{"error": "x"}"#,
        true,
    ),
    (
        "BackupCompleted",
        None,
        "BackupFailed",
        r#"{"error": "x"}"#,
        false,
    ),
    (
        "BackupFailed",
        Some(r#"{"error": "disk full"}"#),
        "BackupFailed",
        r#"{"error": "disk full"}"#,
        true,
    ),
    (
        "BackupFailed",
        Some(r#"{"error": "disk full"}"#),
        "BackupFailed",
        r#"{"type": "BackupFailed", "data": {"error": "disk full"}}"#,
        false,
    ),
    (
        "GithubWorkflowRunStateChanged",
        Some(r#"{"action": "completed", "workflow_run.event": "schedule"}"#),
        "GithubWorkflowRunStateChanged",
        r#"{"action": "completed", "workflow_run": {"event": "schedule"}}"#,
        true,
    ),
    (
        "GithubWorkflowRunStateChanged",
        Some(r#"{"action": "completed", "workflow_run.event": "schedule"}"#),
        "GithubWorkflowRunStateChanged",
        r#"{"action": "completed", "workflow_run": {"event": "workflow_dispatch"}}"#,
        false,
    ),
    (
        "GithubWorkflowRunStateChanged",
        Some(r#"{"workflow_run.event": "schedule"}"#),
        "GithubWorkflowRunStateChanged",
        r#"{"action": "completed"}"#,
        false,
    ),
    (
        "ToolCalled",
        Some(r#"{"a.b": "literal"}"#),
        "ToolCalled",
        r#"{"a.b": "literal", "a": {"b": "nested"}}"#,
        true,
    ),
    (
        "GithubWorkflowRunStateChanged",
        Some(r#"{"workflow_run.conclusion": {"$nin": ["success", "skipped"]}}"#),
        "GithubWorkflowRunStateChanged",
        r#"{"workflow_run": {"conclusion": "failure"}}"#,
        true,
    ),
    (
        "GithubWorkflowRunStateChanged",
        Some(r#"{"workflow_run.conclusion": {"$nin": ["success", "skipped"]}}"#),
        "GithubWorkflowRunStateChanged",
        r#"{"workflow_run": {"conclusion": "success"}}"#,
        false,
    ),
    (
        "ToolCalled",
        Some(r#"{"args.command": {"$regex": "cargo test"}}"#),
        "ToolCalled",
        r#"{"name": "run_bash", "args": {"command": "cd r && cargo test --lib"}}"#,
        true,
    ),
    (
        "ToolCalled",
        Some(r#"{"args.command": {"$regex": "^cargo test"}}"#),
        "ToolCalled",
        r#"{"name": "run_bash", "args": {"command": "cd r && cargo test --lib"}}"#,
        false,
    ),
    (
        "ChangeProposed",
        Some(r#"{"repo": "r", "$or": [{"file_count": {"$gt": 9}}, {"risk": "high"}]}"#),
        "ChangeProposed",
        r#"{"repo": "r", "file_count": 1, "risk": "high"}"#,
        true,
    ),
    (
        "ChangeProposed",
        Some(r#"{"repo": "r", "$or": [{"file_count": {"$gt": 9}}, {"risk": "high"}]}"#),
        "ChangeProposed",
        r#"{"repo": "other", "file_count": 20, "risk": "high"}"#,
        false,
    ),
];

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
