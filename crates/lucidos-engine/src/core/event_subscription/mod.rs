//! The **event subscription** primitive: one `{event_type, condition}` shape and
//! one matcher, shared by every consumer that watches the EventBus.
//!
//! Two consumers today, and the point of this module is that they cannot drift:
//!
//! * a **trigger**'s `on:` list ([`crate::triggers::find_matching_event_triggers`]),
//!   which spawns a new thread when an event matches;
//! * a thread's **event wait**, which resumes an existing parked thread.
//!
//! Both call [`EventSubscription::matches`], so a `condition` that fires for one
//! fires for the other. This lived in `triggers/config.rs` while a trigger was
//! the only subscriber; it moved here when a thread became one too, because the
//! name has to describe what the thing is now.
//!
//! The other half of the shared contract is the **subscribability gate**, one
//! function per carrier: [`is_subscribable`] drops the per-token streaming
//! firehose before either matcher sees a thread event, and
//! [`is_subscribable_system_event`] decides which system frames reach them at
//! all. The third is [`matchable_payload`], which decides what a `condition` is
//! evaluated against.

pub mod condition;
pub mod known_names;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::engine::thread_events::ThreadEvent;

/// One event a subscriber listens for, with an optional payload filter scoped
/// to that event. A subscriber may carry several entries: it matches when an
/// incoming event matches *any* entry's `event_type` AND that entry's
/// condition (if set) evaluates true against the payload. Conditions are
/// per-entry so a single subscriber can watch events with different payload
/// shapes without one filter constraining the other.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventSubscription {
    pub event_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Value>,
}

impl EventSubscription {
    /// Whether this subscription matches an incoming event.
    ///
    /// **The single predicate both dispatch paths run.** The trigger matcher and
    /// the event-wait dispatcher must agree on every (subscription, event) pair,
    /// so neither is allowed to re-implement the name comparison or reach into
    /// [`condition::evaluate`] directly. Callers still have to apply
    /// [`is_subscribable`] to the event first; this function deliberately does
    /// not, because the gate is about the *event* and belongs upstream of the
    /// whole fan-out rather than being re-run per subscription.
    pub fn matches(&self, event_type: &str, payload: &Value) -> bool {
        self.event_type == event_type && condition::evaluate(self.condition.as_ref(), payload)
    }

    /// Whether any subscription in `subs` matches. The per-entry OR semantics of
    /// a subscriber's `on:` list, in one place.
    pub fn any_matches(subs: &[EventSubscription], event_type: &str, payload: &Value) -> bool {
        subs.iter().any(|sub| sub.matches(event_type, payload))
    }

    /// Trim `event_type` on every entry and drop ones that become empty.
    /// Used by both create and update endpoints (HTTP + LLM tool) so the
    /// blank-event-type drop rule lives in one place.
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

    /// Build a subscription from a JSON object entry (`{event_type, condition?}`).
    /// Returns `None` when `event_type` is missing or blank. The caller decides
    /// whether to treat that as silent-skip (stored events) or an error (LLM tool).
    pub(crate) fn from_object_entry(obj: &serde_json::Map<String, Value>) -> Option<Self> {
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

// ── The matchable payload ───────────────────────────────────────────

/// The key [`matchable_payload`] injects. The same spelling the `threads` list
/// returns, so a subscriber scopes a wait with a value it already holds.
pub const THREAD_ID_KEY: &str = "thread_id";

/// The **matchable payload**: what a `condition` is evaluated against.
///
/// An event's own serialized fields, plus the id of the thread it belongs to.
/// Every consumer that offers a payload to [`EventSubscription::matches`] builds
/// its view here, so the live dispatcher, the catch-up scan, the arming lookback
/// and trigger dispatch cannot disagree about what a condition can name.
///
/// **Why the thread id is not a field on the events themselves.** It already has
/// two canonical homes and neither is the payload: the carrier
/// (`BusEvent::Thread { thread_id, event }`, so every thread event has exactly
/// one by construction) and the `events.thread_id` column, which
/// `20260314120000_strip_thread_id_from_payloads.sql` deliberately made the
/// source of truth by removing the key from stored payloads. Persisting it per
/// event would duplicate that column into every row forever and would still
/// leave the next event type unscopable. Injecting it here costs nothing on
/// disk, and makes every thread event scopable at once.
///
/// `thread_id` is `None` for a row that belongs to no thread (a system frame, a
/// workspace domain event), and such an event is deliberately NOT thread-
/// scopable: the live bus could sometimes supply an originating thread where the
/// stored row has a NULL column, and a condition that matched live but not on
/// replay is the exact asymmetry this function exists to prevent.
///
/// Insert-if-absent, never overwrite: an event that declares its own
/// `thread_id` keeps its value, and so does a user-authored domain payload.
///
/// `event_type` is taken so the adjacent-tag envelope a `SystemEvent` stores can
/// be unwrapped here; see [`unwrap_adjacent_tag`].
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

/// Flatten the `{"type": …, "data": {…}}` envelope a `SystemEvent` serializes
/// into, so a `condition` names the event's own fields.
///
/// `SystemEvent` is `#[serde(tag = "type", content = "data")]`, so the row a
/// backup completion writes is `{"type": "BackupCompleted", "data": {…}}`.
/// Without this, `{"filename": …}` would name nothing. It is also what the agent
/// reads back, since `delivery_reentry_text` prints this same value.
///
/// **A field path does not make this optional.** A condition could now reach the
/// same value as `data.filename`. But every stored condition on a system event
/// names the field directly, so dropping the flattening breaks all of them.
///
/// The gate is the name, then the shape. Only a persisted `SystemEvent` name
/// writes this envelope, and a workspace cannot emit one: `is_reserved_type_name`
/// refuses the whole reserved set at the emit endpoint. The shape alone is
/// ambiguous. A domain event a workspace authored as
/// `{"type": "ReleasePublished", "data": {…}}` would lose its own payload to
/// `data`.
///
/// Live and replay pass the same stored name, so both flatten identically. A
/// thread event and a workspace domain event pass through untouched: their
/// payloads are already the fields themselves.
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
        // A unit-like variant serializes to `{"type": …}` with no content.
        None => Value::Object(serde_json::Map::new()),
        // A non-object content cannot carry named fields, so flattening it
        // would lose the value entirely. Leave the envelope for that case.
        Some(_) => payload,
    }
}

/// [`matchable_payload`] for a live `BusEvent::Thread`, which is the form both
/// live consumers need: the event-wait dispatcher and the trigger matcher.
///
/// They call this rather than composing the two steps themselves, so the view
/// they match against is the same object by construction rather than by two
/// call sites happening to agree. `EventMeta::NONE` because the meta a
/// particular emit stamped is not part of it: the live paths never see the meta
/// at all ([`crate::engine::event_bus::EmittedEvent`] does not carry it), so a
/// meta field would be conditionable on replay only.
pub fn matchable_thread_payload(event: &ThreadEvent, thread_id: Uuid) -> Value {
    matchable_payload(
        event.event_type(),
        event.to_payload(&crate::engine::thread_events::EventMeta::NONE),
        Some(thread_id),
    )
}

/// [`matchable_payload`] for a live `BusEvent::System`, the other half of the
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

// ── The shared subscribability gate ─────────────────────────────────

/// Names of the per-token streaming variants, mirroring
/// [`ThreadEvent::is_per_token_streaming`] for the cases where only the *name*
/// is in hand (validating an LLM tool argument, seeding the `on_event:`
/// dropdown) rather than a constructed event.
///
/// `per_token_streaming_names_match_the_predicate` pins the two together.
pub const PER_TOKEN_STREAMING_EVENT_TYPES: &[&str] = &[
    "TextStreamed",
    "ThoughtStreamed",
    "CodingAgentTextStreamed",
    "CodingAgentThoughtStreamed",
];

/// The `EventWait*` family. **Triggerable but never awaitable.**
///
/// A wait on `EventWaitStarted` self-satisfies on the next registration
/// anywhere in the workspace; a wait on `EventWaitDelivered` self-satisfies the
/// instant any wait delivers. They stay triggerable, because a trigger that
/// notifies "a thread's wait timed out" is a reasonable thing to want.
///
/// This is the ONE name family the two subscription surfaces disagree about,
/// which is why [`SubscriptionSurface`] exists at all.
pub const EVENT_WAIT_EVENT_TYPES: &[&str] = &[
    "EventWaitStarted",
    "EventWaitDelivered",
    "EventWaitExpired",
    "EventWaitCanceled",
];

/// Which of the two subscription surfaces a name is being validated for.
///
/// The accepted sets are nested rather than merely similar: `Trigger` accepts
/// everything `Wait` does, plus [`EVENT_WAIT_EVENT_TYPES`]. So a suggestion
/// drawn from the wait corpus is always usable at either surface, which is what
/// lets the near-match heuristic stay surface-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionSurface {
    /// A thread's `await_event`, which parks the calling thread.
    Wait,
    /// A trigger's `on:` list, which spawns a new thread.
    Trigger,
}

/// **The shared subscribability gate.** An event reaches the trigger matcher and
/// the event-wait dispatcher iff this returns true.
///
/// Blocklist semantics on purpose: any persisted `ThreadEvent` is subscribable
/// by default, so a new lifecycle or per-action variant becomes watchable
/// without touching this function. Only the per-token firehose is dropped, and
/// only because running the matcher per token would be pure waste. Subscribers
/// scope high-cardinality per-action variants (`ToolCalled`, and friends) with
/// a `condition:` instead.
pub fn is_subscribable(event: &ThreadEvent) -> bool {
    !event.is_per_token_streaming()
}

/// **The system-side gate.** A live `SystemEvent` reaches the trigger matcher
/// and the event-wait dispatcher iff this returns true.
///
/// An allowlist, where [`is_subscribable`] is a blocklist. Two kinds qualify:
///
/// * a workspace's own domain event, transient or not, because `emit_event`
///   chose the name and the caller is the subscriber;
/// * any persisted frame, because persisted means subscribable (ADR 0113). A
///   `BackupCompleted` has no thread event and no domain event beside it, so
///   this is the only path to it.
///
/// Everything else is transient engine chatter, refused at registration by
/// [`validate_subscribable_event_type`] too. Both fan-outs call this one
/// function rather than each testing the pair, which is what makes I8
/// structural.
pub fn is_subscribable_system_event(event: &crate::engine::event_bus::SystemEvent) -> bool {
    matches!(
        event,
        crate::engine::event_bus::SystemEvent::DomainEvent { .. }
    ) || event.is_persisted()
}

/// The two reserved names that are wire wrappers, not events. A row is always
/// filed under the inner name, so neither is ever an `event_type` a
/// subscription can match. They need their own refusal, because calling either
/// one transient would be a lie.
const TRANSPORT_TYPE_NAMES: &[&str] = &["DomainEvent", "ThreadEvent"];

/// What [`validate_subscribable_event_type`] concluded about a name it accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionVerdict {
    /// A known `ThreadEvent` name that survives the gate. The dispatcher will
    /// see it if it is ever emitted.
    KnownThreadEvent,
    /// A `SystemEvent` name the engine writes an `events` row for. Persisted
    /// means subscribable (ADR 0113): the row is a durable fact, so both the
    /// wait matcher and the trigger matcher are offered the frame.
    KnownSystemEvent,
    /// Not a `ThreadEvent` name. Accepted, because it may be a *domain* event
    /// (`emit_event`) that nobody has emitted yet, and refusing those would
    /// make the tool useless for exactly the workspace-defined events it is
    /// most useful for.
    ///
    /// The caller is expected to layer one corroborating check on top: a name
    /// the workspace has never emitted gets a *note* in the tool result. That
    /// one is not decidable here, because it is a question about this
    /// workspace's event store rather than about the name.
    UnknownName,
}

/// Validate a name supplied to a subscription, structurally.
///
/// **A refused subscription is an error at the tool boundary, not a silent
/// no-op.** A trigger that subscribes to `TextStreamed` validates, persists and
/// then never fires: a documented footgun the user discovers days later. Both
/// write surfaces are synchronous. Each hands the caller an error naming the
/// blocked variant, so it can pick another event in the same turn.
///
/// The messages are surface-neutral, because every case but one is shared.
/// Writing "a wait would never resolve" into a trigger's refusal would describe
/// machinery the caller is not using.
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
    // The one case the two surfaces disagree about. A trigger falls through to
    // `classify_event` below and resolves as an ordinary thread event.
    if surface == SubscriptionSurface::Wait && EVENT_WAIT_EVENT_TYPES.contains(&name) {
        return Err(format!(
            "'{name}' is part of the event-wait machinery, so waiting on it \
             would satisfy itself the moment any thread in this workspace \
             registers or resolves a wait. Wait on the event you actually care \
             about instead. A trigger may watch it."
        ));
    }
    // A ThreadEvent wins over the two system checks below, because several
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

/// Refuse a name that is a misspelling of an engine event.
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

/// The note on a name that validates but which nobody here has ever emitted.
///
/// Not an error. A domain event is legitimate before its first emit. That is
/// the ordinary order of work: make X emit an event, then subscribe to it.
pub fn never_emitted_warning(event_type: &str) -> String {
    format!(
        "'{event_type}' has never been emitted in this workspace. That is fine \
         for a domain event you are about to start emitting. If you expected an \
         engine event, check the spelling with the events tool's event_types \
         action."
    )
}

/// Has this workspace ever emitted an event by this name?
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

/// **The one check every subscription site runs**, over every entry in the list.
///
/// `await_event`, the trigger HTTP API and the trigger LLM tool all call this,
/// so a subscription refused by one is refused by all three. Returns the
/// warnings to show beside the success, or the first hard error.
///
/// Both halves of an entry are checked, the `event_type` and the `condition`,
/// because both fail the same silent way. A misspelled name arms clean and
/// never matches, and so does an unsupported operator. Only a synchronous
/// refusal turns that into something the caller can fix in the same turn.
/// [`condition::validate`] owns the second half's rules.
///
/// The first failure refuses the whole call. A partly armed subscription reads
/// as a success while watching for less than the caller asked. That is the
/// failure this module exists to end.
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
        if verdict != SubscriptionVerdict::UnknownName {
            continue;
        }
        // The store is the escape hatch, so it is consulted BEFORE the
        // heuristic. A name this workspace has emitted is real by proof, and no
        // edit-distance rule may take it away.
        if event_type_ever_emitted(pool, name).await {
            continue;
        }
        refuse_near_miss(name)?;
        warnings.push(never_emitted_warning(name));
    }
    Ok(warnings)
}

/// The two halves of "what can I subscribe to here?", kept apart because they
/// carry different guarantees.
pub struct EventTypeCatalog {
    /// What the engine emits, derived from its own enumerations. A closed set:
    /// a name that merely looks like one of these is refused.
    pub engine: Vec<&'static str>,
    /// The workspace's own domain events: what this store has seen and the
    /// engine does not emit. An open set, so a name outside it is only warned
    /// about.
    pub workspace: Vec<String>,
}

/// **The answer to "which event types exist here?"**, for the tool action the
/// error messages point callers at.
///
/// The engine half is the same list [`check_subscriptions`]
/// validates against, so a name read off this catalog always validates. That is
/// the whole point of the action: an agent that guessed wrong needs somewhere
/// to look the real name up.
///
/// The store half is run through the validator for the same reason. A store
/// holds every name this workspace ever wrote, retired engine names included.
/// Offering it raw would hand the caller a name the next subscription refuses.
/// Filtering by the validator keeps the two aligned by construction, rather
/// than by a second list of exclusions.
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
        .filter(|seen| validate_subscribable_event_type(seen, surface).is_ok())
        .collect();
    Ok(EventTypeCatalog { engine, workspace })
}

/// The persisted event that ends the run a transient progress frame reports on.
///
/// Advice only, so a frame with no terminal twin simply gets a shorter message.
/// `MemoryRebuildProgress` and `RecoveryProgress` are the current such cases:
/// neither run writes a terminal row today.
fn transient_terminal_hint(name: &str) -> Option<&'static str> {
    match name {
        "BackupProgress" => Some("BackupCompleted or BackupFailed"),
        _ => None,
    }
}

/// **The gate on every domain-event emit**, wherever it comes from.
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

// `pub(crate)` so `triggers::tests` can run the same `PARITY_CASES` table
// through the trigger dispatch path (I8).
#[cfg(test)]
#[path = "mod_tests.rs"]
pub(crate) mod tests;
