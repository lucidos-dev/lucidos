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
//! The other half of the shared contract is the **subscribability gate**
//! ([`is_subscribable`]): the per-token streaming firehose is dropped before
//! either matcher sees it. The third is [`matchable_payload`], which decides
//! what a `condition` is evaluated against.

pub mod condition;

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
pub fn matchable_payload(payload: Value, thread_id: Option<Uuid>) -> Value {
    let Some(thread_id) = thread_id else {
        return payload;
    };
    let mut payload = payload;
    if let Some(obj) = payload.as_object_mut() {
        obj.entry(THREAD_ID_KEY)
            .or_insert_with(|| Value::String(thread_id.to_string()));
    }
    payload
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
        event.to_payload(&crate::engine::thread_events::EventMeta::NONE),
        Some(thread_id),
    )
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
pub const EVENT_WAIT_EVENT_TYPES: &[&str] = &[
    "EventWaitStarted",
    "EventWaitDelivered",
    "EventWaitExpired",
    "EventWaitCanceled",
];

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

/// What [`validate_awaitable_event_type`] concluded about a name it accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwaitableVerdict {
    /// A known `ThreadEvent` name that survives the gate. The dispatcher will
    /// see it if it is ever emitted.
    KnownThreadEvent,
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

/// Validate a name supplied to `await_event`, structurally.
///
/// **A refused subscription is an error at the tool boundary, not a silent
/// no-op.** A trigger that subscribes to `TextStreamed` validates, persists and
/// then never fires: a documented footgun the user discovers days later.
/// `await_event` is synchronous, so it can hand the model an error naming the
/// blocked variant and let it pick another event in the same turn.
pub fn validate_awaitable_event_type(event_type: &str) -> Result<AwaitableVerdict, String> {
    let name = event_type.trim();
    if name.is_empty() {
        return Err("event_type must not be empty".to_string());
    }
    if PER_TOKEN_STREAMING_EVENT_TYPES.contains(&name) {
        return Err(format!(
            "'{name}' is a per-token streaming event, fired once per text chunk. \
             It is dropped before any subscriber sees it, so a wait on it would \
             never resolve. Wait on the turn's outcome instead \
             (ResponseGenerated), or on a specific tool call (ToolCalled with a \
             condition on `name`)."
        ));
    }
    if EVENT_WAIT_EVENT_TYPES.contains(&name) {
        return Err(format!(
            "'{name}' is part of the event-wait machinery, so waiting on it \
             would satisfy itself the moment any thread in this workspace \
             registers or resolves a wait. Wait on the event you actually care \
             about instead."
        ));
    }
    // A ThreadEvent wins over the reserved-name check below, because several
    // names are BOTH (`ChangeDiscarded` is a `SystemEvent` variant and a
    // `ThreadEvent` variant). The thread-scoped one is the one a wait can see.
    if crate::engine::thread_lifecycle::classify_event(name).is_some() {
        return Ok(AwaitableVerdict::KnownThreadEvent);
    }
    if crate::engine::event_bus::SystemEvent::is_reserved_type_name(name) {
        return Err(format!(
            "'{name}' is a system event. The wait matcher sees thread events and \
             workspace domain events (the ones `emit_event` writes), not system \
             frames, so a wait on it would never resolve. Wait on the thread event \
             or the domain event that accompanies it instead."
        ));
    }
    Ok(AwaitableVerdict::UnknownName)
}

// `pub(crate)` so `triggers::tests` can run the same `PARITY_CASES` table
// through the trigger dispatch path (I8).
#[cfg(test)]
#[path = "mod_tests.rs"]
pub(crate) mod tests;
