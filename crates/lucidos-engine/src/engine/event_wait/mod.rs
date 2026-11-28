mod agent_surface;
mod background_task;
mod dispatcher;
pub mod keep_alive;
mod pending_cards;
mod pending_wake;
mod ping_plan;
mod register;
mod remembered_requests;
mod wait_durations;

pub use agent_surface::CancelEventWaitOutcome;
pub(crate) use keep_alive::KeepAliveBeats;
#[cfg(any(test, feature = "test-support"))]
pub use keep_alive::WaitAgent;
pub use pending_cards::PendingCardPage;
pub use pending_wake::PendingWake;
pub(crate) use ping_plan::ReEstimates;
pub(crate) use register::describe_subscriptions;
#[cfg(any(test, feature = "test-support"))]
pub use register::MAX_LIVE_WAITS_PER_THREAD;
pub use register::MAX_TIMEOUT_SECS;
pub use register::{AwaitEventOutcome, RegisteredEventWait};
pub(crate) use register::{MAX_RECENT_SUBSCRIPTIONS, RECENT_SUBSCRIPTION_WINDOW_SECS};
pub(crate) use remembered_requests::RememberedRequests;
pub(crate) use wait_durations::WaitDurationsCache;
pub use {background_task::OwnerTurn, background_task::StoppedTask};
#[cfg(any(test, feature = "test-support"))]
pub use {pending_wake::MissedEstimate, pending_wake::PendingChild, pending_wake::PendingWait};

use crate::engine::thread_events::not_an_engine_turn_idle_sql;
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::core::event_subscription::{is_subscribable, matchable_payload, EventSubscription};
use crate::engine::event_bus::SystemEvent;
use crate::engine::thread_events::{
    BackgroundWaitTiming, EngineReason, EventMeta, EventWaitOutcome, MessageOrigin, ThreadEvent,
};

#[derive(Debug, Clone)]
pub struct LiveWait {
    pub wait_id: Uuid,
    pub thread_id: Uuid,
    pub tool_use_id: String,
    pub on: Vec<EventSubscription>,
    pub reason: String,
    pub armed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub background_timing: Option<BackgroundWaitTiming>,
    pub watermark: i64,
}

impl LiveWait {
    pub fn matched_index(&self, event_type: &str, payload: &Value) -> Option<usize> {
        if event_type == SystemEvent::SUB_THREAD_CARD_OPENED
            && payload
                .get("ancestor_thread_id")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
                != Some(self.thread_id)
        {
            return None;
        }
        self.on
            .iter()
            .position(|sub| sub.matches(event_type, payload))
    }

    /// Relay rows are addressed and forward-only even when a pre-arming
    /// opening's post-commit broadcast arrives after the wait was installed.
    fn matched_index_at(
        &self,
        event_type: &str,
        payload: &Value,
        sequence: Option<i64>,
    ) -> Option<usize> {
        if event_type == SystemEvent::SUB_THREAD_CARD_OPENED
            && sequence.is_none_or(|seq| seq <= self.watermark)
        {
            return None;
        }
        self.matched_index(event_type, payload)
    }

    pub fn watches(&self, event_type: &str) -> bool {
        self.on.iter().any(|sub| sub.matches_type(event_type))
    }
}

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

    pub(super) async fn update_timing(
        &self,
        wait_id: Uuid,
        reason: String,
        expires_at: DateTime<Utc>,
        background_timing: BackgroundWaitTiming,
    ) {
        if let Some(wait) = self.inner.write().await.get_mut(&wait_id) {
            wait.reason = reason;
            wait.expires_at = expires_at;
            wait.background_timing = Some(background_timing);
        }
    }

    /// INVARIANT: Remove and return a wait. **The one-shot gate.**
    ///
    /// Delivery, expiry and cancellation all resolve through this, and the
    /// caller must act only on `Some`. Two matching events arriving back to
    /// back therefore produce exactly one delivery: the second `take` finds
    /// nothing, because a tool call has exactly one result and N deliveries for
    /// one `tool_use_id` are not expressible (I7).
    pub async fn take(&self, wait_id: Uuid) -> Option<LiveWait> {
        self.inner.write().await.remove(&wait_id)
    }

    pub async fn take_on_thread(&self, thread_id: Uuid, wait_id: Uuid) -> Option<LiveWait> {
        let mut inner = self.inner.write().await;
        match inner.get(&wait_id) {
            Some(w) if w.thread_id == thread_id => inner.remove(&wait_id),
            _ => None,
        }
    }

    pub async fn for_thread(&self, thread_id: Uuid) -> Vec<LiveWait> {
        self.inner
            .read()
            .await
            .values()
            .filter(|w| w.thread_id == thread_id)
            .cloned()
            .collect()
    }

    pub async fn snapshot(&self) -> Vec<LiveWait> {
        let mut all: Vec<LiveWait> = self.inner.read().await.values().cloned().collect();
        all.sort_by_key(|w| w.watermark);
        all
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}

/// INVARIANT: Which waits a given event resolves, in registration order.
///
/// Pure, so the matching rule is testable without a bus or a database. The
/// caller still has to win the [`LiveWaits::take`] race for each one before
/// acting on it.
#[cfg(test)]
fn waits_matching(waits: &[LiveWait], event_type: &str, payload: &Value) -> Vec<(Uuid, usize)> {
    waits_matching_at(waits, event_type, payload, None)
}

fn waits_matching_at(
    waits: &[LiveWait],
    event_type: &str,
    payload: &Value,
    sequence: Option<i64>,
) -> Vec<(Uuid, usize)> {
    waits
        .iter()
        .filter_map(|w| {
            w.matched_index_at(event_type, payload, sequence)
                .map(|i| (w.wait_id, i))
        })
        .collect()
}

pub fn waits_matching_row(waits: &[LiveWait], row: &crate::core::EventRow) -> Vec<(Uuid, usize)> {
    waits_matching_at(
        waits,
        &row.event_type,
        &matchable_payload(&row.event_type, row.payload.clone(), row.thread_id),
        row.sequence,
    )
}

/// INVARIANT: Whether this bus event should be offered to the wait matcher at all.
///
/// Two rules, and only the second is specific to waits:
///
/// 1. The shared subscribability gate ([`is_subscribable`]), so the per-token
///    firehose is dropped exactly as it is for triggers.
/// 2. The `EventWait*` family is dropped. Those stay *triggerable*, but must
///    never reach a wait, or a wait would satisfy itself the instant any thread
///    registers or resolves one. `await_event` also refuses them at the tool
///    boundary. This is the structural half of the same rule, and it holds for
///    a wait registered before that validation existed.
pub fn is_awaitable_event(event: &ThreadEvent) -> bool {
    if !is_subscribable(event) {
        return false;
    }
    !crate::core::event_subscription::EVENT_WAIT_EVENT_TYPES.contains(&event.event_type())
}

/// INVARIANT: The `EventWaitStarted` rows on this thread that are still live, oldest
/// first.
///
/// "Live" means no `EventWaitDelivered` / `EventWaitExpired` /
/// `EventWaitCanceled` carries the same `wait_id` at a later sequence. This is
/// the boot rebuild's per-thread half and the query behind the live-wait cap.
/// `e.aggregate = 'thread'` is load-bearing, not tidiness. `aggregate_id` holds
/// a thread uuid only for thread events. On a DOMAIN event it holds the event
/// TYPE NAME.
///
/// An untrusted caller of the emit endpoint can choose any name that is not a
/// reserved `SystemEvent`, and `EventWaitStarted` is a `ThreadEvent` name. One
/// such call writes a permanent row whose `aggregate_id` is that literal, and
/// an unscoped cast then fails the whole query. Events are append-only, so that
/// would break the boot rebuild forever and leave the cache empty.
macro_rules! unresolved_waits_sql {
    ($($bound:tt)+) => {
        concat!(
            "SELECT e.aggregate_id::uuid, e.payload || COALESCE(( \
                 SELECT u.payload FROM events u WHERE u.aggregate = 'thread' \
                   AND u.aggregate_id = e.aggregate_id AND u.sequence > e.sequence \
                   AND u.event_type = 'EventWaitTimingUpdated' \
                   AND u.payload->>'wait_id' = e.payload->>'wait_id' \
                 ORDER BY u.sequence DESC LIMIT 1 \
             ), '{}'::jsonb), e.sequence, e.created \
             FROM events e \
             WHERE e.aggregate = 'thread' \
               AND e.event_type = 'EventWaitStarted' \
               AND ",
            $($bound)+,
            " AND NOT EXISTS ( \
                 SELECT 1 FROM events r \
                 WHERE r.aggregate_id = e.aggregate_id \
                   AND r.sequence > e.sequence \
                   AND r.event_type IN \
                       ('EventWaitDelivered','EventWaitExpired','EventWaitCanceled') \
                   AND r.payload->>'wait_id' = e.payload->>'wait_id' \
             ) \
             ORDER BY e.sequence"
        )
    };
}
const LIVE_WAITS_SQL: &str = unresolved_waits_sql!(concat!(
    "e.sequence > ",
    crate::core::snapshot::boundary::restore_floor_sql!()
));
const ARCHIVED_WAITS_SQL: &str = unresolved_waits_sql!("e.sequence <= $1");

/// INVARIANT: Rebuild the live-waits cache from the event store (I3). Runs at boot, and is
/// the reason no table is needed.
///
/// Returns the waits it loaded. Expired ones are loaded too rather than
/// filtered. The deadline sweep resolves them on its next tick. A wait that
/// timed out while the engine was down then re-enters its thread with an expiry
/// instead of vanishing. Dropping them here would be the silent stall the whole
/// design refuses.
pub async fn rebuild_live_waits(
    pool: &sqlx::PgPool,
    waits: &LiveWaits,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let rows: Vec<(Uuid, Value, i64, DateTime<Utc>)> =
        sqlx::query_as(LIVE_WAITS_SQL).fetch_all(pool).await?;
    let mut loaded = 0usize;
    for (thread_id, payload, sequence, created) in rows {
        match live_wait_from_payload(thread_id, &payload, sequence, created) {
            Some(wait) => {
                waits.insert(wait).await;
                loaded += 1;
            }
            None => crate::log_legacy!(
                "[EventWait] Skipped malformed EventWaitStarted payload on thread {} (seq {})",
                thread_id,
                sequence,
            ),
        }
    }
    Ok(loaded)
}

pub(crate) async fn settle_restored_waits(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(boundary) = crate::core::snapshot::boundary::read(pool).await? else {
        return Ok(());
    };
    let rows: Vec<(Uuid, Value, i64, DateTime<Utc>)> = sqlx::query_as(ARCHIVED_WAITS_SQL)
        .bind(boundary.event_sequence)
        .fetch_all(pool)
        .await?;
    for (thread_id, payload, sequence, created) in rows {
        let Some(wait) = live_wait_from_payload(thread_id, &payload, sequence, created) else {
            crate::log_legacy!(
                "[EventWait] Skipped malformed archived EventWaitStarted on thread {} (seq {})",
                thread_id,
                sequence,
            );
            continue;
        };
        emit_cancel(
            bus,
            &wait,
            crate::engine::thread_events::EventWaitCancelCause::SnapshotRestored,
            None,
        )
        .await?;
    }
    Ok(())
}

fn live_wait_from_payload(
    thread_id: Uuid,
    payload: &Value,
    sequence: i64,
    created: DateTime<Utc>,
) -> Option<LiveWait> {
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
    let armed_at = payload
        .get("armed_at")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or(created);
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
        armed_at,
        expires_at,
        background_timing: payload
            .get("background_timing")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        watermark,
    })
}

const CATCH_UP_PAGE: i64 = 200;

/// INVARIANT: How many pages the **arming lookback** may read before giving up.
///
/// The forward scan has no such budget and needs none. It stops at its first
/// match, and it is the mechanism that must not miss one. The lookback is
/// advisory and runs synchronously inside `await_event`. An unbounded page
/// count would add database round trips to every registration on a busy
/// workspace. Three pages is far past the point where the newest few matches
/// are what the model meant.
const ARMING_LOOKBACK_MAX_PAGES: usize = 3;

pub async fn catch_up_from_watermark(
    pool: &sqlx::PgPool,
    wait: &LiveWait,
) -> Result<Option<(Uuid, String, Value, usize)>, Box<dyn std::error::Error + Send + Sync>> {
    let relay = SystemEvent::SUB_THREAD_CARD_OPENED;
    let ordinary_types: Vec<_> = wait
        .on
        .iter()
        .flat_map(EventSubscription::event_type_candidates)
        .filter(|name| *name != relay)
        .collect();
    let watches_relay = wait.watches(relay);
    if ordinary_types.is_empty() && !watches_relay {
        return Ok(None);
    }
    let aggregate = SystemEvent::SUB_THREAD_CARD_AGGREGATE;
    let mut after = wait.watermark;
    loop {
        // The addressed branch uses the aggregate/ID/sequence index before
        // paging. Keep it separate from the workspace-wide scan even on a
        // mixed wait, so a busy family cannot fill another thread's pages.
        let rows: Vec<(Uuid, String, Value, Option<Uuid>, i64)> = sqlx::query_as(&format!(
            "SELECT id, event_type, payload, thread_id, sequence FROM (
                 (SELECT id, event_type, payload, thread_id, sequence FROM events
                  WHERE sequence > $1 AND event_type = ANY($2) AND {}
                  ORDER BY sequence LIMIT $3)
                 UNION ALL
                 (SELECT id, event_type, payload, thread_id, sequence FROM events
                  WHERE $4 AND aggregate = '{aggregate}' AND aggregate_id = $5
                    AND event_type = '{relay}' AND sequence > $1
                  ORDER BY sequence LIMIT $3)
             ) candidates ORDER BY sequence LIMIT $3",
            not_an_engine_turn_idle_sql("")
        ))
        .bind(after)
        .bind(&ordinary_types)
        .bind(CATCH_UP_PAGE)
        .bind(watches_relay)
        .bind(wait.thread_id.to_string())
        .fetch_all(pool)
        .await?;

        let exhausted = (rows.len() as i64) < CATCH_UP_PAGE;
        if let Some(&(_, _, _, _, last_seq)) = rows.last() {
            after = last_seq;
        }
        for (id, event_type, payload, thread_id, seq) in rows {
            let payload = matchable_payload(&event_type, payload, thread_id);
            if let Some(idx) = wait.matched_index_at(&event_type, &payload, Some(seq)) {
                return Ok(Some((id, event_type, payload, idx)));
            }
        }
        if exhausted {
            return Ok(None);
        }
    }
}

#[derive(Debug, Clone)]
pub struct LookbackMatch {
    pub event_type: String,
    pub payload: Value,
    pub age_secs: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ArmingLookback {
    pub matches: Vec<LookbackMatch>,
    /// INVARIANT: There were more than the limit. Deliberately a bool rather than a total:
    /// counting them all would mean scanning the whole window, which is the
    /// unbounded read this function exists to avoid.
    pub more: bool,
}

impl ArmingLookback {
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }
}

pub async fn arming_lookback_matches(
    pool: &sqlx::PgPool,
    on: &[EventSubscription],
    watermark: i64,
    window_secs: i64,
    already_delivered: &std::collections::HashSet<Uuid>,
    limit: usize,
) -> Result<ArmingLookback, Box<dyn std::error::Error + Send + Sync>> {
    let types: Vec<_> = on
        .iter()
        .flat_map(EventSubscription::event_type_candidates)
        .filter(|name| *name != SystemEvent::SUB_THREAD_CARD_OPENED)
        .collect();
    if types.is_empty() {
        return Ok(ArmingLookback::default());
    }
    let wanted = limit.saturating_add(1);
    let mut found: Vec<LookbackMatch> = Vec::with_capacity(wanted);
    let mut upper = watermark;
    let mut pages = 0;
    while found.len() < wanted {
        if pages == ARMING_LOOKBACK_MAX_PAGES {
            crate::log_legacy!(
                "[EventWait] Arming lookback hit its {ARMING_LOOKBACK_MAX_PAGES}-page budget \
                 for {types:?}; reporting the {} match(es) found so far",
                found.len(),
            );
            break;
        }
        pages += 1;
        // INVARIANT: `now()` on both sides of the window and of the age: the cutoff and
        // the elapsed time are the database's own arithmetic, so neither can be
        // thrown off by the engine host's clock. See the doc comment.
        // `thread_id` for the same reason as the catch-up scan above: it is the
        // column that answers "which thread, or none" across mixed rows.
        let rows: Vec<(Uuid, String, Value, Option<Uuid>, i64, i64)> = sqlx::query_as(&format!(
            "SELECT id, event_type, payload, thread_id, \
                    EXTRACT(EPOCH FROM now() - created)::bigint, sequence \
             FROM events \
             WHERE sequence <= $1 \
               AND created >= now() - make_interval(secs => $2) \
               AND event_type = ANY($3) AND {} \
             ORDER BY sequence DESC LIMIT $4",
            not_an_engine_turn_idle_sql("")
        ))
        .bind(upper)
        .bind(window_secs)
        .bind(&types)
        .bind(CATCH_UP_PAGE)
        .fetch_all(pool)
        .await?;

        let exhausted = (rows.len() as i64) < CATCH_UP_PAGE;
        if let Some(&(_, _, _, _, _, last_seq)) = rows.last() {
            upper = last_seq - 1;
        }
        for (id, event_type, payload, thread_id, age_secs, _) in rows {
            let payload = matchable_payload(&event_type, payload, thread_id);
            if !already_delivered.contains(&id)
                && EventSubscription::any_matches(on, &event_type, &payload)
            {
                found.push(LookbackMatch {
                    event_type,
                    payload,
                    age_secs,
                });
                if found.len() == wanted {
                    break;
                }
            }
        }
        if exhausted {
            break;
        }
    }

    let more = found.len() > limit;
    found.truncate(limit);
    Ok(ArmingLookback {
        matches: found,
        more,
    })
}

/// The one thing a *delivery* has to say beyond the payload.
///
/// Delivery is the only resolution that CONSUMES the subscription, and it was
/// the only one whose text said nothing about it: [`expiry_text`] already says
/// report rather than wait again, and a cancel is the user ending it. So at the
/// one moment re-subscribing is the right move, the result itself was silent.
///
/// Scoped to THIS subscription and deliberately not to the thread. A thread may
/// hold several at once, so "nothing is watching any more" would be a lie
/// whenever another is live. It would also talk the model into re-registering
/// one the duplicate refusal then rejects.
///
/// The text names the failure rather than only stating the fact. A model that
/// narrates a re-arm and ends the turn leaves the user looking at an idle
/// thread that just promised to keep watching.
///
/// The closing sentence is a **temporary measure**, registered in
/// `docs/temporary-measures.md` under "Narrating it does not do it". It carries
/// no system fact and exists only to pre-empt the observed mistake. The rest of the notice is permanent, since a
/// perfect model still has to be told the subscription is gone.
const WAIT_SPENT_NOTICE: &str = "\n\nThis subscription is now spent: the first match \
     resolves a wait, so it has stopped watching. To catch the next one, call \
     await_event again before this turn ends. Narrating it does not do it: a turn that \
     ends with no new call leaves nothing watching for this, whatever the sentence said.";

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

pub fn delivery_reentry_text(event_type: &str, payload: &Value, reason: &str) -> String {
    format!(
        "An event you subscribed to has arrived (you were waiting because: {reason}).\n\n\
         {event_type}:\n{}{WAIT_SPENT_NOTICE}",
        serde_json::to_string_pretty(&tail_long_strings(payload))
            .unwrap_or_else(|_| "<unserializable payload>".to_string()),
    )
}

const REENTRY_STRING_MAX_BYTES: usize = 4000;

fn tail_long_strings(value: &Value) -> Value {
    match value {
        Value::String(s) if s.len() > REENTRY_STRING_MAX_BYTES => {
            let start = s.ceil_char_boundary(s.len() - REENTRY_STRING_MAX_BYTES);
            Value::String(format!(
                "[{start} earlier bytes cut, the full value is in the event]\n…{}",
                &s[start..]
            ))
        }
        Value::Array(items) => Value::Array(items.iter().map(tail_long_strings).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), tail_long_strings(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

pub fn expiry_reentry_text(wait: &LiveWait, never_seen: &[String]) -> String {
    format!(
        "A subscription you registered has timed out.\n\n{}",
        expiry_text(wait, never_seen)
    )
}

#[derive(Debug, Clone)]
pub struct WaitReentry {
    /// INVARIANT: The already-persisted `PromptInjected` this re-entry anchors to.
    /// Never a fresh `MessageReceived`: nobody said anything.
    pub anchor_event_id: Uuid,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelWaitOutcome {
    Canceled,
    NotLive,
    EmitFailed,
}

#[derive(Debug, Clone)]
pub struct WaitReentryRequest {
    pub thread_id: Uuid,
    pub reentry: WaitReentry,
}

/// INVARIANT: Emit a resolution plus its **re-entry anchor**, and report what the
/// dispatcher should re-enter with.
///
/// The anchor is the second event of the pair, and there is exactly one shape:
/// a `PromptInjected` carrying the payload as prose. It is an
/// `EXCHANGE_START_TYPES` member on the frontend, so the delivery reads as the
/// new turn it genuinely is, exactly like the child-completion fan-in.
///
/// The two commit together or not at all, so no resolution stands without its
/// anchor. A crash after both is what `lost_wait_reentries` picks up, which is
/// why the anchor is one recognisable event shape.
pub async fn emit_delivery(
    bus: &crate::engine::event_bus::EventBus,
    wait: &LiveWait,
    event_id: Uuid,
    event_type: &str,
    payload: &Value,
    matched_index: usize,
) -> Result<WaitReentry, Box<dyn std::error::Error + Send + Sync>> {
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
        || delivery_reentry_text(event_type, payload, &wait.reason),
    )
    .await
}

pub async fn emit_expiry(
    bus: &crate::engine::event_bus::EventBus,
    wait: &LiveWait,
    never_seen: &[String],
) -> Result<WaitReentry, Box<dyn std::error::Error + Send + Sync>> {
    emit_resolution(
        bus,
        wait,
        ThreadEvent::EventWaitExpired {
            wait_id: wait.wait_id,
        },
        || expiry_reentry_text(wait, never_seen),
    )
    .await
}

pub async fn emit_cancel(
    bus: &crate::engine::event_bus::EventBus,
    wait: &LiveWait,
    cause: crate::engine::thread_events::EventWaitCancelCause,
    actor: Option<crate::engine::thread_events::MessageOrigin>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id: wait.thread_id,
        event: ThreadEvent::EventWaitCanceled {
            wait_id: wait.wait_id,
            cause,
            on: wait.on.clone(),
            reason: wait.reason.clone(),
        },
        meta: EventMeta::with_actor(actor),
    })
    .await?
    .ok_or("[EventWait] EventWaitCanceled was not persisted")?;
    Ok(())
}

/// INVARIANT: The shared body of [`emit_delivery`] and [`emit_expiry`]: the resolution and
/// its re-entry anchor, as one atomic emit.
///
/// One `Err` therefore means nothing was written. The persisted
/// `EventWaitStarted` is still live, so the caller MUST put the wait back in
/// the cache, or a transient write failure strands the thread.
async fn emit_resolution(
    bus: &crate::engine::event_bus::EventBus,
    wait: &LiveWait,
    resolution: ThreadEvent,
    wake_text: impl FnOnce() -> String,
) -> Result<WaitReentry, Box<dyn std::error::Error + Send + Sync>> {
    let resolution_is_delivery = matches!(resolution, ThreadEvent::EventWaitDelivered { .. });
    let resolution_id = Uuid::new_v4();
    let anchor_event_id = Uuid::new_v4();
    let text = wake_text();
    let anchor = ThreadEvent::PromptInjected {
        text: text.clone(),
        mode: crate::engine::thread_events::ActorMode::Agent,
        origin: Some(reentry_origin(wait, resolution_is_delivery)),
        injected_message_id: None,
        // INVARIANT: The prose in `text` stays exactly as the model needs it. This is
        // the same delivery addressed to the client instead, so it can name
        // the event and fold the payload away.
        delivered_event_id: resolution_is_delivery.then_some(resolution_id),
    };
    let with_id = |id| EventMeta {
        event_id: Some(id),
        ..EventMeta::NONE
    };
    bus.emit_atomically(
        wait.thread_id,
        vec![
            (resolution, with_id(resolution_id)),
            (anchor, with_id(anchor_event_id)),
        ],
    )
    .await?
    .ok_or("[EventWait] the resolution and its anchor were not persisted")?;
    Ok(WaitReentry {
        anchor_event_id,
        text,
    })
}

fn reentry_origin(wait: &LiveWait, delivered: bool) -> MessageOrigin {
    let mut watched: Vec<String> = Vec::new();
    for sub in &wait.on {
        if !watched.contains(&sub.event_type) {
            watched.push(sub.event_type.clone());
        }
    }
    MessageOrigin::engine(EngineReason::EventWait {
        outcome: if delivered {
            EventWaitOutcome::Delivered
        } else {
            EventWaitOutcome::Expired
        },
        watched,
        wait_reason: wait.reason.clone(),
    })
}

pub const DEADLINE_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// INVARIANT: Re-entries the event store says were persisted but never ran (I3b), oldest
/// first.
///
/// The selection is the whole recovery rule, so it lives here on the pool
/// rather than in the engine method that actuates it. It can then be tested
/// against a real database without standing up an engine.
///
/// A resolution's re-entry **never ran** when the very next event on its thread
/// is that resolution's own anchor and nothing follows it. Side-question events
/// are skipped both ways, since none of them is a turn. `emit_resolution`
/// writes exactly one anchor shape, so that is the whole exclusion list.
/// Anything else after the resolution means the re-entry WAS consumed, and
/// re-driving it would double-run a turn.
///
/// A legacy attached delivery anchored on the paired `ToolResult` instead, and
/// is deliberately NOT selected: an old-world re-entry has no slot in a turn to
/// be re-driven into. Discarded threads are skipped
/// too: reviving a thread the user threw away, to read an event it no longer
/// cares about, is the archive-curtain problem in another costume.
pub async fn lost_wait_reentries(
    pool: &sqlx::PgPool,
) -> Result<Vec<WaitReentryRequest>, Box<dyn std::error::Error + Send + Sync>> {
    let rows: Vec<(String, Uuid, Option<String>)> = sqlx::query_as(concat!(
        "SELECT e.aggregate_id, anchor.id, anchor.payload->>'text' \
         FROM events e \
         JOIN thread_summaries t ON t.thread_id = e.aggregate_id::uuid \
         JOIN LATERAL ( \
             SELECT a.id, a.event_type, a.payload, a.sequence FROM events a \
             WHERE a.aggregate = 'thread' \
               AND a.aggregate_id = e.aggregate_id \
               AND a.sequence > e.sequence \
               AND a.event_type <> ALL($1) \
             ORDER BY a.sequence LIMIT 1 \
         ) anchor ON TRUE \
         WHERE e.aggregate = 'thread' \
           AND e.event_type IN ('EventWaitDelivered','EventWaitExpired') \
           AND e.sequence > ",
        crate::core::snapshot::boundary::restore_floor_sql!(),
        " \
           AND t.state IS DISTINCT FROM 'discarded' \
           AND anchor.event_type = 'PromptInjected' \
           AND NOT EXISTS ( \
               SELECT 1 FROM events later \
               WHERE later.aggregate = 'thread' \
                 AND later.aggregate_id = e.aggregate_id \
                 AND later.sequence > anchor.sequence \
                 AND later.event_type <> ALL($1) \
           ) \
         ORDER BY e.sequence"
    ))
    .bind(crate::engine::thread_events::ThreadEvent::SIDE_QUESTION_EVENT_TYPES)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(thread_str, anchor_id, anchor_text)| {
            let thread_id = thread_str
                .parse::<Uuid>()
                .inspect_err(|_| {
                    crate::log_legacy!(
                        "[EventWait] Skipping lost re-entry on malformed thread id {:?}",
                        thread_str
                    )
                })
                .ok()?;
            Some(WaitReentryRequest {
                thread_id,
                reentry: WaitReentry {
                    anchor_event_id: anchor_id,
                    text: anchor_text.unwrap_or_default(),
                },
            })
        })
        .collect())
}

/// INVARIANT: Has a wait on `thread_id` already been delivered by one of `event_ids`?
///
/// The persisted half of the child-completion stand-down gate
/// (`LucidosEngine::child_completion_has_an_event_wait`), which is where the
/// two ids it is asked about, and the fail-open policy, are explained. A free
/// function over the pool so a test can seed a delivery and ask, without
/// standing up an engine.
///
/// An unreadable answer is `false`: the caller then wakes the thread, which
/// costs a duplicate turn rather than silence.
pub async fn wait_delivery_names_any(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    event_ids: &[String],
) -> bool {
    match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
             SELECT 1 FROM events d \
             WHERE d.aggregate = 'thread' \
               AND d.aggregate_id = $1 \
               AND d.event_type = 'EventWaitDelivered' \
               AND d.payload->>'event_id' = ANY($2) \
         )",
    )
    .bind(thread_id.to_string())
    .bind(event_ids)
    .fetch_one(pool)
    .await
    {
        Ok(delivered) => delivered,
        Err(e) => {
            crate::log_legacy!(
                "[EventWait] Delivery probe failed for thread {}: {}. Answering no, \
                 so the caller wakes the thread",
                thread_id,
                e
            );
            false
        }
    }
}

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
