//! The **event-wait dispatcher**: the second consumer of the EventBus, beside
//! the trigger matcher.
//!
//! A thread that called `await_event` holds a subscription. This module is what
//! wakes it: it holds the live waits, matches every subscribable event against
//! them with the shared [`EventSubscription::matches`], and drives the wake.
//!
//! # No table
//!
//! The persisted `EventWaitStarted` **is** the wait. [`LiveWaits`] is a cache,
//! rebuilt from the event store at boot by [`rebuild_live_waits`], and nothing
//! here holds state that cannot be reconstructed that way. This is ADR 0011's
//! shape applied to a new wake: the fan-in case rejected a second persisted
//! representation for the same reason.
//!
//! # The watermark closes two gaps with one mechanism
//!
//! Each wait records the event `sequence` at registration. Registration and the
//! boot rebuild both run the same catch-up scan forward from it
//! ([`catch_up_from_watermark`]), which covers:
//!
//! * events that landed while the engine was down, and
//! * the live race between emitting `EventWaitStarted` and this module
//!   inserting the cache entry.
//!
//! # One wake shape
//!
//! A subscribed thread is an ordinary **idle** thread. `await_event` returns
//! immediately, its turn ends with a real terminator, and the delivery wakes the
//! thread through the existing fan-in shape: a `UserPromptInjected` carrying the
//! event as prose, injected into a live turn as
//! `InjectedPromptKind::WakeFromEvent` or starting a new one. Exactly what a
//! child completion or a user follow-up does.
//!
//! It was not always so. A wait used to be *attached*, meaning `await_event`
//! ended the turn with its `tool_use` deliberately unpaired so the delivered
//! event could arrive as that call's result and the model could resume
//! mid-thought. That bought continuity inside one exchange, and it cost: an
//! unpaired `tool_use` is a provider 400 the moment anything else runs on the
//! thread, so it needed detach-on-interruption with a filler result, an
//! attachment probe at every resolution site, two wake-anchor kinds, a
//! `waiting_for_event` status, a restart guard, and a bar on the injection fast
//! path (an attached wake's prompt is empty, and an empty user block is its own
//! 400). That last one is what broke on 2026-08-06: the barred wake queued
//! behind a running turn and the 60 s Thread Queue backstop evicted the turn to
//! let it in. All of it is gone. See
//! `docs/plans/2026-08-06-every-event-wait-is-detached.md`.

mod dispatcher;
mod register;

pub(crate) use register::AwaitEventOutcome;
/// Re-exported so the `await_event` tool description can interpolate the real
/// cap instead of restating a number that would silently drift from the
/// refusal the model actually hits.
pub(crate) use register::MAX_CONSECUTIVE_SUBSCRIPTIONS;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::core::event_subscription::{is_subscribable, EventSubscription};
use crate::engine::thread_events::{EventMeta, ThreadEvent};

/// One live wait, as reconstructed from its `EventWaitStarted`.
///
/// Every field is a copy of that event's payload. Nothing is derived and kept
/// here, because anything derived would be state the boot rebuild has to guess
/// at.
#[derive(Debug, Clone)]
pub struct LiveWait {
    pub wait_id: Uuid,
    pub thread_id: Uuid,
    pub tool_use_id: String,
    pub on: Vec<EventSubscription>,
    pub reason: String,
    pub expires_at: DateTime<Utc>,
    pub watermark: i64,
}

impl LiveWait {
    /// Index of the first `on:` entry matching this event, if any.
    ///
    /// Returns the index rather than a bool because the delivery names which
    /// entry fired: a wait is a rendezvous, not a stream, so a model watching
    /// several events needs to know which one woke it.
    pub fn matched_index(&self, event_type: &str, payload: &Value) -> Option<usize> {
        self.on
            .iter()
            .position(|sub| sub.matches(event_type, payload))
    }
}

/// The live-waits cache: `wait_id` to [`LiveWait`].
///
/// Keyed by `wait_id` rather than by thread because a thread may hold several
/// at once (up to `MAX_LIVE_WAITS_PER_THREAD`), and because delivery consumes
/// exactly one wait.
#[derive(Debug, Default)]
pub struct LiveWaits {
    inner: RwLock<HashMap<Uuid, LiveWait>>,
}

impl LiveWaits {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, wait: LiveWait) {
        self.inner.write().await.insert(wait.wait_id, wait);
    }

    /// Remove and return a wait. **The one-shot gate.**
    ///
    /// Delivery, expiry and cancellation all resolve through this, and the
    /// caller must act only on `Some`. Two matching events arriving back to
    /// back therefore produce exactly one wake: the second `take` finds
    /// nothing, because a tool call has exactly one result and N deliveries for
    /// one `tool_use_id` are not expressible (I7).
    pub async fn take(&self, wait_id: Uuid) -> Option<LiveWait> {
        self.inner.write().await.remove(&wait_id)
    }

    /// Remove and return a wait, but only if it belongs to `thread_id`.
    ///
    /// One lock, so a caller addressing a wait by id cannot check membership
    /// and then take across a window in which the wait resolves. The thread
    /// scope is what makes an id from one thread's UI unable to cancel
    /// another's.
    pub async fn take_on_thread(&self, thread_id: Uuid, wait_id: Uuid) -> Option<LiveWait> {
        let mut inner = self.inner.write().await;
        match inner.get(&wait_id) {
            Some(w) if w.thread_id == thread_id => inner.remove(&wait_id),
            _ => None,
        }
    }

    /// Every live wait for one thread, for the subscription indicator and for
    /// the cancel-on-archive sweep.
    pub async fn for_thread(&self, thread_id: Uuid) -> Vec<LiveWait> {
        self.inner
            .read()
            .await
            .values()
            .filter(|w| w.thread_id == thread_id)
            .cloned()
            .collect()
    }

    /// Snapshot of every live wait, oldest watermark first so a burst of
    /// matches resolves in registration order.
    pub async fn snapshot(&self) -> Vec<LiveWait> {
        let mut all: Vec<LiveWait> = self.inner.read().await.values().cloned().collect();
        all.sort_by_key(|w| w.watermark);
        all
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}

/// Which waits a given event resolves, in registration order.
///
/// Pure, so the matching rule is testable without a bus or a database. The
/// caller still has to win the [`LiveWaits::take`] race for each one before
/// acting on it.
pub fn waits_matching(waits: &[LiveWait], event_type: &str, payload: &Value) -> Vec<(Uuid, usize)> {
    waits
        .iter()
        .filter_map(|w| w.matched_index(event_type, payload).map(|i| (w.wait_id, i)))
        .collect()
}

/// Whether this bus event should be offered to the wait matcher at all.
///
/// Two rules, and only the second is specific to waits:
///
/// 1. The shared subscribability gate ([`is_subscribable`]), so the per-token
///    firehose is dropped exactly as it is for triggers.
/// 2. The `EventWait*` family is dropped. Those stay *triggerable* (a trigger
///    notifying "a thread's wait timed out" is reasonable) but must never reach
///    a wait, or a wait would satisfy itself the instant any thread in the
///    workspace registers or resolves one. `await_event` also refuses them at
///    the tool boundary; this is the structural half of the same rule, and it
///    holds for a wait registered before that validation existed.
pub fn is_awaitable_event(event: &ThreadEvent) -> bool {
    if !is_subscribable(event) {
        return false;
    }
    !crate::core::event_subscription::EVENT_WAIT_EVENT_TYPES.contains(&event.event_type())
}

/// The `EventWaitStarted` rows on this thread that are still live, oldest
/// first.
///
/// "Live" means no `EventWaitDelivered` / `EventWaitExpired` /
/// `EventWaitCanceled` carries the same `wait_id` at a later sequence. This is
/// the boot rebuild's per-thread half and the query behind the live-wait cap.
/// `e.aggregate = 'thread'` is load-bearing, not tidiness. `aggregate_id` holds
/// a thread uuid only for thread events; on a DOMAIN event it holds the event
/// TYPE NAME, and `POST /api/v1/events/emit` lets an app UI (untrusted, by that
/// handler's own comment) emit a domain event under any name that is not a
/// reserved `SystemEvent`. `EventWaitStarted` is a `ThreadEvent` name, so it is
/// not reserved: one such call writes a permanent row whose `aggregate_id` is
/// the literal `'EventWaitStarted'`, and an unscoped `aggregate_id::uuid` then
/// fails the whole query with `invalid input syntax for type uuid`. Events are
/// append-only, so that would break the boot rebuild on EVERY later boot and
/// leave the live-wait cache permanently empty.
const LIVE_WAITS_SQL: &str = "\
    SELECT e.aggregate_id::uuid, e.payload, e.sequence \
    FROM events e \
    WHERE e.aggregate = 'thread' \
      AND e.event_type = 'EventWaitStarted' \
      AND NOT EXISTS ( \
          SELECT 1 FROM events r \
          WHERE r.aggregate_id = e.aggregate_id \
            AND r.sequence > e.sequence \
            AND r.event_type IN \
                ('EventWaitDelivered','EventWaitExpired','EventWaitCanceled') \
            AND r.payload->>'wait_id' = e.payload->>'wait_id' \
      ) \
    ORDER BY e.sequence";

/// Rebuild the live-waits cache from the event store. Runs at boot, and is the
/// reason no table is needed.
///
/// Returns the waits it loaded. Expired ones are loaded too rather than
/// filtered: the deadline sweep resolves them on its next tick, so a wait whose
/// deadline passed while the engine was down wakes its thread with an expiry
/// instead of vanishing (I3). Dropping them here would be the silent-stall the
/// whole design refuses.
pub async fn rebuild_live_waits(
    pool: &sqlx::PgPool,
    waits: &LiveWaits,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let rows: Vec<(Uuid, Value, i64)> = sqlx::query_as(LIVE_WAITS_SQL).fetch_all(pool).await?;
    let mut loaded = 0usize;
    for (thread_id, payload, sequence) in rows {
        match live_wait_from_payload(thread_id, &payload, sequence) {
            Some(wait) => {
                waits.insert(wait).await;
                loaded += 1;
            }
            None => crate::log!(
                "[EventWait] Skipped malformed EventWaitStarted payload on thread {} (seq {})",
                thread_id,
                sequence,
            ),
        }
    }
    Ok(loaded)
}

/// Parse a persisted `EventWaitStarted` payload back into a [`LiveWait`].
///
/// Reads the fields individually rather than deserializing the whole
/// `ThreadEvent`, because the persisted payload has its `type` tag stripped
/// (see `ThreadEvent::to_payload`). `sequence` is the row's own sequence, used
/// as the watermark fallback for a payload written before the field existed.
fn live_wait_from_payload(thread_id: Uuid, payload: &Value, sequence: i64) -> Option<LiveWait> {
    let wait_id = payload.get("wait_id")?.as_str()?.parse().ok()?;
    let tool_use_id = payload.get("tool_use_id")?.as_str()?.to_string();
    let on: Vec<EventSubscription> = serde_json::from_value(payload.get("on")?.clone()).ok()?;
    let reason = payload
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let expires_at = payload
        .get("expires_at")?
        .as_str()?
        .parse::<DateTime<Utc>>()
        .ok()?;
    let watermark = payload
        .get("watermark")
        .and_then(|v| v.as_i64())
        .unwrap_or(sequence);
    Some(LiveWait {
        wait_id,
        thread_id,
        tool_use_id,
        on,
        reason,
        expires_at,
        watermark,
    })
}

/// How many rows the catch-up scan reads per round-trip.
///
/// The scan pages instead of reading everything, because a wait is a rendezvous
/// and only its FIRST match matters. `MAX_TIMEOUT_SECS` is 24 hours and the
/// tool positively recommends high-cardinality subscriptions (`ToolCalled` with
/// a `condition` on `name`), so a wait registered a day before a restart can
/// have tens of thousands of candidate rows behind it, payloads and all. The
/// old unbounded `fetch_all` pulled every one of them into memory on the
/// startup path to look at the first.
const CATCH_UP_PAGE: i64 = 200;

/// The first event after a wait's watermark that matches it, if any.
///
/// The catch-up scan (S7). Filters by the subscribed event types in SQL and
/// evaluates the conditions in Rust, so the `$eq/$ne/$lt/$gt/$in` language has
/// exactly one implementation. That split is why this cannot be a bare
/// `LIMIT 1`: SQL can narrow to the subscribed names, but only Rust can say
/// whether a `condition` holds, so the scan pages forward until a row passes
/// both. Returns `(event_id, event_type, payload, matched_index)`.
pub async fn catch_up_from_watermark(
    pool: &sqlx::PgPool,
    wait: &LiveWait,
) -> Result<Option<(Uuid, String, Value, usize)>, Box<dyn std::error::Error + Send + Sync>> {
    let types: Vec<String> = wait.on.iter().map(|s| s.event_type.clone()).collect();
    if types.is_empty() {
        return Ok(None);
    }
    let mut after = wait.watermark;
    loop {
        let rows: Vec<(Uuid, String, Value, i64)> = sqlx::query_as(
            "SELECT id, event_type, payload, sequence FROM events \
             WHERE sequence > $1 AND event_type = ANY($2) \
             ORDER BY sequence LIMIT $3",
        )
        .bind(after)
        .bind(&types)
        .bind(CATCH_UP_PAGE)
        .fetch_all(pool)
        .await?;

        let exhausted = (rows.len() as i64) < CATCH_UP_PAGE;
        if let Some(&(_, _, _, last_seq)) = rows.last() {
            after = last_seq;
        }
        for (id, event_type, payload, _) in rows {
            if let Some(idx) = wait.matched_index(&event_type, &payload) {
                return Ok(Some((id, event_type, payload, idx)));
            }
        }
        if exhausted {
            return Ok(None);
        }
    }
}

/// Has this wait already been resolved?
///
/// The same predicate `LIVE_WAITS_SQL`'s `NOT EXISTS` encodes, asked of one
/// wait. Used where a caller has taken a wait out of the cache and has to
/// decide whether putting it back is safe: a resolution that landed meanwhile
/// (a cancel racing a detach) must not be undone.
pub async fn wait_is_resolved(
    pool: &sqlx::PgPool,
    wait: &LiveWait,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let resolved: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM events r \
             WHERE r.aggregate = 'thread' \
               AND r.aggregate_id = $1 \
               AND r.event_type IN \
                   ('EventWaitDelivered','EventWaitExpired','EventWaitCanceled') \
               AND r.payload->>'wait_id' = $2 \
         )",
    )
    .bind(wait.thread_id.to_string())
    .bind(wait.wait_id.to_string())
    .fetch_one(pool)
    .await?;
    Ok(resolved)
}

use crate::llm::tool_names::AWAIT_EVENT;

/// The one thing a *delivery* has to say beyond the payload.
///
/// Delivery is the only resolution that CONSUMES the subscription, and it was
/// the only one whose text said nothing about it: [`expiry_text`] already says
/// report rather than wait again, and a cancel is the user ending it. So at the
/// one moment re-subscribing is the right move, the result itself was silent.
///
/// Scoped to THIS subscription and deliberately not to the thread: a thread may
/// hold up to `MAX_LIVE_WAITS_PER_THREAD` at once, so "nothing is watching any
/// more" would be a lie whenever another is live, and would talk the model into
/// re-registering one the duplicate refusal in `register` then rejects.
///
/// A live thread on 2026-08-06 woke on a delivery, reported the event, wrote
/// "Re-arming the watch now, so I'll keep reporting each edit" as its closing
/// sentence, and ended the turn with no second call. The user was left looking
/// at an idle thread that had just promised to keep watching. That is why the
/// text names the failure rather than only stating the fact.
///
/// That last sentence ("Narrating it does not do it …") is a **temporary
/// measure**, registered in `docs/temporary-measures.md` § "\"Narrating it does
/// not do it\" on an event-wait re-arm": it carries no system fact and exists
/// only to pre-empt the observed mistake. The rest of the notice is permanent,
/// since a perfect model still has to be told the subscription is gone.
const WAIT_SPENT_NOTICE: &str = "\n\nThis subscription is now spent: the first match \
     resolves a wait, so it has stopped watching. To catch the next one, call \
     await_event again before this turn ends. Narrating it does not do it: a turn that \
     ends with no new call leaves nothing watching for this, whatever the sentence said.";

/// The body of the expiry wake.
///
/// `never_seen` names any watched event type this workspace has **never**
/// emitted. That is the one moment the fact is worth stating: `await_event`
/// deliberately accepts an unknown name (it may be a domain event nobody has
/// emitted yet, which is the case the tool is most useful for), and registration
/// confirms only that the subscription was accepted, so a typo is invisible
/// until exactly here. Naming it turns a baffling timeout into an obvious one.
fn expiry_text(wait: &LiveWait, never_seen: &[String]) -> String {
    let watched: Vec<&str> = wait.on.iter().map(|s| s.event_type.as_str()).collect();
    let mut text = format!(
        "Timed out. Nothing matching {} happened before the deadline you set \
         (reason: {}). Report what you were waiting for rather than subscribing \
         again to the same thing, unless you have a reason to think it is about \
         to happen.",
        watched.join(", "),
        wait.reason,
    );
    if !never_seen.is_empty() {
        text.push_str(&format!(
            "\n\nWorth checking: this workspace has never emitted {}. If that is a \
             typo, the corrected name is what to wait on; if it is an event \
             something is supposed to start emitting, say so rather than waiting \
             again.",
            never_seen.join(", "),
        ));
    }
    text
}

/// The text a delivery wake carries. Closes with [`WAIT_SPENT_NOTICE`], which
/// the wake needs more than the old attached tool result did: it starts a fresh
/// turn, so the model has no open call to remind it that it was ever watching.
pub fn delivery_wake_text(event_type: &str, payload: &Value, reason: &str) -> String {
    format!(
        "An event you subscribed to has arrived (you were waiting because: {reason}).\n\n\
         {event_type}:\n{}{WAIT_SPENT_NOTICE}",
        serde_json::to_string_pretty(payload)
            .unwrap_or_else(|_| "<unserializable payload>".to_string()),
    )
}

/// The text an expiry wake carries.
pub fn expiry_wake_text(wait: &LiveWait, never_seen: &[String]) -> String {
    format!(
        "A subscription you registered has timed out.\n\n{}",
        expiry_text(wait, never_seen)
    )
}

/// How a resolved wait hands its payload back to the thread.
///
/// Produced by [`emit_delivery`] / [`emit_expiry`] so the *emit* writes the
/// anchor and the dispatcher just actuates the re-entry.
#[derive(Debug, Clone)]
pub struct EventWake {
    /// The already-persisted `UserPromptInjected` this re-entry anchors to.
    /// Never a fresh `MessageReceived`: nobody said anything.
    pub anchor_event_id: Uuid,
    /// The prompt for the re-entry, the same prose the anchor carries.
    pub text: String,
}

/// What **Stop waiting** did to one wait.
///
/// Three states rather than a bool because the two failures are not the same
/// thing to a user: a wait that already resolved is gone and the button was
/// stale, while a wait whose cancel could not be written is still there and
/// still cancellable. Collapsing them told the user "already resolved" about a
/// wait that was about to keep waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelWaitOutcome {
    Canceled,
    /// No live wait with that id on that thread.
    NotLive,
    /// The wait is live; persisting the cancel failed.
    EmitFailed,
}

/// One wake in flight, as it travels over the engine's wake channel.
///
/// Plain data on purpose. Handing the turn to a consumer task instead of
/// awaiting it in place is what keeps `run_agentic_loop`'s future from
/// containing itself, since registration can resolve a wait inline through its
/// catch-up scan. See `EVENT_WAKE_RX` for the full argument.
#[derive(Debug, Clone)]
pub struct EventWakeRequest {
    pub thread_id: Uuid,
    pub wake: EventWake,
}

/// Emit a resolution plus its **wake anchor**, and report what the dispatcher
/// should re-enter with.
///
/// The anchor is the second event of the pair, and there is exactly one shape:
/// a `UserPromptInjected` carrying the payload as prose. It is an
/// `EXCHANGE_START_TYPES` member on the frontend, so the wake reads as the new
/// turn it genuinely is, exactly like the child-completion fan-in.
///
/// Emission order matters: the resolution goes first, so a crash between the
/// two leaves a *resolved* wait (the boot rebuild will not re-arm it) rather
/// than an anchor for a wait that still looks live. A crash after both is what
/// `lost_event_wakes` picks up, which is why the anchor is one recognisable
/// event shape.
pub async fn emit_delivery(
    bus: &crate::engine::event_bus::EventBus,
    wait: &LiveWait,
    event_id: Uuid,
    event_type: &str,
    payload: &Value,
    matched_index: usize,
) -> Result<EventWake, ResolutionEmitError> {
    emit_resolution(
        bus,
        wait,
        ThreadEvent::EventWaitDelivered {
            wait_id: wait.wait_id,
            event_id,
            event_type: event_type.to_string(),
            payload: payload.clone(),
            matched_index,
        },
        || delivery_wake_text(event_type, payload, &wait.reason),
    )
    .await
}

/// Emit the resolution event for an expired wait plus its wake anchor. Same
/// shape and same ordering rule as [`emit_delivery`].
pub async fn emit_expiry(
    bus: &crate::engine::event_bus::EventBus,
    wait: &LiveWait,
    never_seen: &[String],
) -> Result<EventWake, ResolutionEmitError> {
    emit_resolution(
        bus,
        wait,
        ThreadEvent::EventWaitExpired {
            wait_id: wait.wait_id,
        },
        || expiry_wake_text(wait, never_seen),
    )
    .await
}

/// Emit the resolution event for a **canceled** wait.
///
/// The one resolution with no wake behind it, and therefore no anchor: the user
/// ended the subscription, so the thread is left exactly as it was. There is
/// nothing to close, because `await_event` paired its own call at registration.
///
/// `actor` is the device that ended it, and it is the one resolution that HAS
/// one: a delivery and an expiry are the engine acting on its own, but every
/// cancel cause is a person pressing something. Stamped per
/// `.claude/rules/rust.md` so the timeline can say who, the same way the Stop
/// button's `ResponseCanceled` does.
pub async fn emit_cancel(
    bus: &crate::engine::event_bus::EventBus,
    wait: &LiveWait,
    cause: crate::engine::thread_events::EventWaitCancelCause,
    actor: Option<crate::engine::thread_events::MessageOrigin>,
) -> Result<(), ResolutionEmitError> {
    emit_persisted_with(
        bus,
        wait.thread_id,
        ThreadEvent::EventWaitCanceled {
            wait_id: wait.wait_id,
            cause,
        },
        EventMeta::with_actor(actor),
    )
    .await
    .map_err(ResolutionEmitError::Unresolved)?;
    Ok(())
}

/// Why a resolution emit failed, and therefore what the caller owes the wait.
///
/// The two arms are NOT the same recovery, which is the whole reason this is a
/// typed error rather than a boxed string (the structural justification
/// `.claude/rules/rust.md` asks for: the caller branches on it, it does not
/// just format it):
///
/// * `Unresolved` means nothing was written. The persisted `EventWaitStarted`
///   is still live and unresolved, so the caller MUST put the wait back in the
///   cache. Without that, a transient write failure strands the thread: gone
///   from the live set so nothing can match or expire it, still live in the
///   event store so the boot rebuild would only re-arm it on the next restart.
/// * `AnchorMissing` means the resolution landed but its wake anchor did not.
///   Re-arming there would be wrong (the wait IS resolved, and re-arming would
///   let it deliver twice). The thread is recovered by the ordinary restart
///   machinery instead: the resolution is its last word, so the orphan sweep
///   settles the dangling call and the user gets Continue.
#[derive(Debug)]
pub enum ResolutionEmitError {
    /// Nothing was persisted. The wait is still live.
    Unresolved(Box<dyn std::error::Error + Send + Sync>),
    /// The resolution was persisted; its anchor was not.
    AnchorMissing(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Display for ResolutionEmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unresolved(e) => write!(f, "the resolution was not persisted: {e}"),
            Self::AnchorMissing(e) => write!(f, "the wake anchor was not persisted: {e}"),
        }
    }
}

impl std::error::Error for ResolutionEmitError {}

/// The shared body of [`emit_delivery`] and [`emit_expiry`]: resolution first,
/// then its wake anchor.
async fn emit_resolution(
    bus: &crate::engine::event_bus::EventBus,
    wait: &LiveWait,
    resolution: ThreadEvent,
    wake_text: impl FnOnce() -> String,
) -> Result<EventWake, ResolutionEmitError> {
    // Whether the anchor can point back at this resolution for a structured
    // render. Only a DELIVERY carries the matched event's name and payload as
    // fields; an expiry has nothing but the same words its own text already
    // says, so linking one would buy the client nothing. Read before the emit
    // moves the value.
    let resolution_is_delivery = matches!(resolution, ThreadEvent::EventWaitDelivered { .. });
    let resolution_id = emit_persisted(bus, wait.thread_id, resolution)
        .await
        .map_err(ResolutionEmitError::Unresolved)?;

    let text = wake_text();
    let anchor_event_id = emit_persisted(
        bus,
        wait.thread_id,
        ThreadEvent::UserPromptInjected {
            text: text.clone(),
            mode: crate::engine::thread_events::ActorMode::Agent,
            origin: None,
            injected_message_id: None,
            // The prose in `text` stays exactly as the model needs it; this is
            // the same delivery addressed to the client instead, so it can name
            // the event and fold the payload away. See the field's doc comment.
            delivered_event_id: resolution_is_delivery.then_some(resolution_id),
        },
    )
    .await
    .map_err(ResolutionEmitError::AnchorMissing)?;
    Ok(EventWake {
        anchor_event_id,
        text,
    })
}

/// Emit one persisted thread event and return its row id.
///
/// Errors rather than defaulting when the bus reports no `EmitResult`: every
/// event here is persisted, so `None` means the write did not happen, and a
/// synthesized id would anchor a re-entry to a row that does not exist.
async fn emit_persisted(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    event: ThreadEvent,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    emit_persisted_with(bus, thread_id, event, EventMeta::NONE).await
}

/// [`emit_persisted`] with caller-supplied meta, for the one resolution that
/// carries an actor.
async fn emit_persisted_with(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    event: ThreadEvent,
    meta: EventMeta,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let event_type = event.event_type();
    let emitted = bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: event.clone(),
            meta,
        })
        .await?;
    emitted
        .map(|r| r.event_id)
        .ok_or_else(|| format!("[EventWait] {event_type} was not persisted").into())
}

/// How often the deadline sweep looks for expired waits.
///
/// A wait's `timeout_secs` is the user-visible promise, and this is the
/// resolution it is kept to: an expiry can land up to this late. Ten seconds is
/// far below the granularity anyone waiting tens of minutes for a release will
/// notice, and cheap because the sweep is a scan of an in-memory map that holds
/// at most a handful of entries per subscribed thread.
pub const DEADLINE_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Wakes the event store says were persisted but never ran (I3b), oldest
/// first.
///
/// The selection is the whole recovery rule, so it lives here on the pool
/// rather than inside the engine method that actuates it: this way it can be
/// tested against a real database without standing up an engine.
///
/// A resolution's wake **never ran** when the very next event on its thread is
/// that resolution's own wake anchor and nothing follows it. `emit_resolution`
/// writes exactly one anchor shape, the `UserPromptInjected`, so that is the
/// whole exclusion list. Anything else after the resolution (a `TextStreamed`,
/// a `ToolCalled`, a terminator, a later `MessageReceived`) means the wake WAS
/// consumed, and re-driving it would double-run a turn.
///
/// A pre-2026-08-06 attached delivery anchored on the paired `await_event`
/// `ToolResult` instead. Those are deliberately NOT selected: the boot sweep in
/// `settle_legacy_attached_event_waits` closes that shape out once, so a wake
/// from the old world is settled rather than re-driven into a turn whose
/// message array no longer has a slot for it.
///
/// Discarded threads are skipped: the user threw the thread away, and reviving
/// it to read an event it no longer cares about is the archive-curtain problem
/// in a different costume.
pub async fn lost_event_wakes(
    pool: &sqlx::PgPool,
) -> Result<Vec<EventWakeRequest>, Box<dyn std::error::Error + Send + Sync>> {
    let rows: Vec<(String, Uuid, Option<String>)> = sqlx::query_as(
        "SELECT e.aggregate_id, anchor.id, anchor.payload->>'text' \
         FROM events e \
         JOIN thread_summaries t ON t.thread_id = e.aggregate_id::uuid \
         JOIN LATERAL ( \
             SELECT a.id, a.event_type, a.payload, a.sequence FROM events a \
             WHERE a.aggregate = 'thread' \
               AND a.aggregate_id = e.aggregate_id \
               AND a.sequence > e.sequence \
             ORDER BY a.sequence LIMIT 1 \
         ) anchor ON TRUE \
         WHERE e.aggregate = 'thread' \
           AND e.event_type IN ('EventWaitDelivered','EventWaitExpired') \
           AND t.state IS DISTINCT FROM 'discarded' \
           AND anchor.event_type = 'UserPromptInjected' \
           AND NOT EXISTS ( \
               SELECT 1 FROM events later \
               WHERE later.aggregate = 'thread' \
                 AND later.aggregate_id = e.aggregate_id \
                 AND later.sequence > anchor.sequence \
           ) \
         ORDER BY e.sequence",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(thread_str, anchor_id, anchor_text)| {
            let thread_id = thread_str
                .parse::<Uuid>()
                .inspect_err(|_| {
                    crate::log!(
                        "[EventWait] Skipping lost wake on malformed thread id {:?}",
                        thread_str
                    )
                })
                .ok()?;
            Some(EventWakeRequest {
                thread_id,
                wake: EventWake {
                    anchor_event_id: anchor_id,
                    text: anchor_text.unwrap_or_default(),
                },
            })
        })
        .collect())
}

/// One-off boot sweep for threads caught mid-**attached** wait by the upgrade
/// that removed that shape (2026-08-06).
///
/// Such a thread has an `await_event` `ToolCalled` with no `ToolResult`, which
/// is a provider 400 on its very next turn, and it may also carry a legacy
/// `waiting_for_event` status. Closing the pair is all that is owed: any wait
/// still unresolved is re-armed by `rebuild_live_waits` as an ordinary
/// subscription and will wake the thread the new way, and any wait already
/// resolved has its payload sitting in the events the thread will read anyway.
///
/// Returns the threads that still carry one. Temporary measure: see
/// `docs/temporary-measures.md`.
pub async fn settle_legacy_attached_event_waits(
    pool: &sqlx::PgPool,
) -> Result<Vec<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT tc.aggregate_id \
         FROM events tc \
         JOIN thread_summaries t ON t.thread_id = tc.aggregate_id::uuid \
         WHERE tc.aggregate = 'thread' \
           AND tc.event_type = 'ToolCalled' \
           AND tc.payload->>'name' = $1 \
           AND t.state IS DISTINCT FROM 'discarded' \
           AND NOT EXISTS ( \
               SELECT 1 FROM events tr \
               WHERE tr.aggregate = 'thread' \
                 AND tr.aggregate_id = tc.aggregate_id \
                 AND tr.sequence > tc.sequence \
                 AND tr.event_type = 'ToolResult' \
                 AND tr.payload->>'name' = $1 \
           ) \
         ORDER BY tc.sequence",
    )
    .bind(AWAIT_EVENT)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(thread_str,)| thread_str.parse::<Uuid>().ok())
        .collect())
}

/// The `ToolResult` [`settle_legacy_attached_event_waits`] writes to close an
/// old attached call. Says plainly that nothing was lost, so a model reading it
/// on resume does not re-subscribe out of doubt.
pub fn legacy_attached_settle_tool_result() -> ThreadEvent {
    ThreadEvent::ToolResult {
        name: AWAIT_EVENT.to_string(),
        result: "Lucidos was upgraded while this call was open. Subscriptions no longer \
                 hold a turn open: any subscription of yours that is still live will wake \
                 this thread as a new message when it matches, exactly as before. Nothing \
                 was lost and nothing needs re-registering. Carry on."
            .to_string(),
        images: vec![],
        success: true,
        tool_called_event_id: None,
    }
}

/// Waits whose deadline has passed, oldest first. The pure half of the
/// deadline sweep, so the boundary is testable without a clock.
pub fn expired_waits(waits: &[LiveWait], now: DateTime<Utc>) -> Vec<Uuid> {
    waits
        .iter()
        .filter(|w| w.expires_at <= now)
        .map(|w| w.wait_id)
        .collect()
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
