//! The **event-wait dispatcher**: the second consumer of the EventBus, beside
//! the trigger matcher. It holds the live waits, matches every subscribable
//! event against them, and drives the re-entry of the waiting thread.
//!
//! **No table.** The persisted `EventWaitStarted` IS the wait (ADR 0047).
//! [`LiveWaits`] is a cache, rebuilt at boot by [`rebuild_live_waits`], and
//! nothing here holds state that cannot be reconstructed that way.
//!
//! **The watermark closes two gaps with one mechanism.** Each wait records the
//! event `sequence` at registration, and both registration and boot run
//! [`catch_up_from_watermark`] forward from it. That covers events that landed
//! while the engine was down, and the live race between the emit and the cache
//! insert.
//!
//! **The third gap is REPORTED, not closed.** An event arriving between the
//! model deciding to wait and the call landing sits BELOW the watermark.
//! [`arming_lookback_matches`] scans backwards for it and reports what it
//! finds. It never delivers: only the model has the turn in its context
//! (ADR 0047). Every wait is detached (ADR 0049).

mod agent_surface;
mod background_task;
mod dispatcher;
mod register;

pub(crate) use agent_surface::CancelEventWaitOutcome;
/// Re-exported so the `await_event` tool description interpolates the real cap.
/// A restated number would silently drift from the refusal the model hits.
pub(crate) use register::MAX_CONSECUTIVE_SUBSCRIPTIONS;
pub(crate) use register::{describe_subscriptions, AwaitEventOutcome};

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::core::event_subscription::{is_subscribable, matchable_payload, EventSubscription};
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
    /// When the subscription was armed. Recorded on the event rather than
    /// derived from `expires_at`. The age is what makes `list_event_waits`
    /// answerable, and a derived one would drift the moment a deadline did.
    pub armed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub watermark: i64,
}

impl LiveWait {
    /// Index of the first `on:` entry matching this event, if any.
    ///
    /// Returns the index rather than a bool because the delivery names which
    /// entry fired: a wait is a rendezvous, not a stream, so a model watching
    /// several events needs to know which one delivered.
    pub fn matched_index(&self, event_type: &str, payload: &Value) -> Option<usize> {
        self.on
            .iter()
            .position(|sub| sub.matches(event_type, payload))
    }

    /// Does this subscription watch `event_type` at all?
    ///
    /// The name only, deliberately ignoring any `condition`, because this
    /// answers "could this fire on that event?" rather than "would this
    /// particular payload fire it?" ([`Self::matched_index`] is that question).
    /// It is what a stand-down by event type resolves against. "Stop telling
    /// me about X" means every watch that could fire on an X, whatever slice of
    /// it each one asked for.
    ///
    /// **`any`, so a subscription watching several event types answers yes to
    /// each, and a stand-down by one of its names ends the whole thing.** That
    /// is the intended reading. A wait is ONE rendezvous with several triggers,
    /// spent by the first match, not several independent watches sharing a row.
    /// Its later legs cannot be delivered once one is gone, and nothing in this
    /// family mutates a wait. The report names every event type it ended, so
    /// the caller reads exactly what went. ADR 0059 records the rejected
    /// re-arm.
    pub fn watches(&self, event_type: &str) -> bool {
        self.on.iter().any(|sub| sub.event_type == event_type)
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
    /// back therefore produce exactly one delivery: the second `take` finds
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

/// [`waits_matching`] for a caller holding a persisted [`crate::core::EventRow`]
/// rather than a live bus event.
///
/// It exists so such a caller cannot ask the question against the stored
/// `payload` column, which is not what any dispatch path matches. The
/// *matchable payload* adds the row's own thread, and a wait scoped with a
/// `thread_id` condition matches only that view (ADR 0062). Asking with the raw
/// column makes a thread-scoped subscription look un-subscribed, and its thread
/// is then re-entered twice for one completion.
pub fn waits_matching_row(waits: &[LiveWait], row: &crate::core::EventRow) -> Vec<(Uuid, usize)> {
    waits_matching(
        waits,
        &row.event_type,
        &matchable_payload(row.payload.clone(), row.thread_id),
    )
}

/// Whether this bus event should be offered to the wait matcher at all.
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

/// The `EventWaitStarted` rows on this thread that are still live, oldest
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
const LIVE_WAITS_SQL: &str = "\
    SELECT e.aggregate_id::uuid, e.payload, e.sequence, e.created \
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

/// Rebuild the live-waits cache from the event store (I3). Runs at boot, and is
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
/// (see `ThreadEvent::to_payload`). `sequence` and `created` are the row's own,
/// used as the fallbacks for the two fields a payload written before they
/// existed does not carry: the watermark, and the arming time.
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
    // The row's own `created` is the right fallback rather than a computed
    // `expires_at - timeout`. The event was written at registration, so the two
    // differ only by the emit. The computed one is a guess that can be wrong by
    // a whole timeout on a legacy subscription.
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
        watermark,
    })
}

/// How many rows the catch-up scan reads per round-trip.
///
/// The scan pages instead of reading everything, because a wait is a
/// rendezvous and only its FIRST match matters. A timeout runs to 24 hours, and
/// the tool positively recommends high-cardinality subscriptions. So a wait
/// registered a day before a restart can have tens of thousands of candidate
/// rows behind it, payloads and all. An unbounded read would pull every one of
/// them into memory on the startup path to look at the first.
const CATCH_UP_PAGE: i64 = 200;

/// How many pages the **arming lookback** may read before giving up.
///
/// The forward scan has no such budget and needs none. It stops at its first
/// match, and it is the mechanism that must not miss one. The lookback is
/// advisory and runs synchronously inside `await_event`. An unbounded page
/// count would add database round trips to every registration on a busy
/// workspace. Three pages is far past the point where the newest few matches
/// are what the model meant.
const ARMING_LOOKBACK_MAX_PAGES: usize = 3;

/// The first event after a wait's watermark that matches it, if any.
///
/// The catch-up scan (S7). Filters by the subscribed event types in SQL and
/// evaluates the conditions in Rust, so the condition language has exactly one
/// implementation. That split is why this cannot be a bare `LIMIT 1`. Only
/// Rust can say whether a `condition` holds, so the scan pages forward until a
/// row passes both the name filter and the condition.
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
        // `thread_id` rather than `aggregate_id`, deliberately. The question
        // is "which thread does this row belong to, or none", over a result set
        // that can include rows belonging to no thread. That is exactly what
        // the column holds, and `aggregate_id::uuid` fails to cast on a system
        // row.
        let rows: Vec<(Uuid, String, Value, Option<Uuid>, i64)> = sqlx::query_as(
            "SELECT id, event_type, payload, thread_id, sequence FROM events \
             WHERE sequence > $1 AND event_type = ANY($2) \
             ORDER BY sequence LIMIT $3",
        )
        .bind(after)
        .bind(&types)
        .bind(CATCH_UP_PAGE)
        .fetch_all(pool)
        .await?;

        let exhausted = (rows.len() as i64) < CATCH_UP_PAGE;
        if let Some(&(_, _, _, _, last_seq)) = rows.last() {
            after = last_seq;
        }
        for (id, event_type, payload, thread_id, _) in rows {
            // The same view the live dispatcher matched against, so a wait that
            // would have matched live matches on replay too.
            let payload = matchable_payload(payload, thread_id);
            if let Some(idx) = wait.matched_index(&event_type, &payload) {
                return Ok(Some((id, event_type, payload, idx)));
            }
        }
        if exhausted {
            return Ok(None);
        }
    }
}

/// One event the *arming lookback* found: something matching the subscription
/// that had already happened by the time the wait was armed.
///
/// Carries the age because the age is what makes the report usable. Only the
/// model tells a miss from something it handled a moment ago, and it tells them
/// apart by when.
///
/// An **age** rather than the row's `created`, for the reason
/// [`arming_lookback_matches`] takes a window rather than a cutoff. `created`
/// is written by the database, so subtracting it from the engine's clock is a
/// cross-clock comparison. It misreports the age whenever the two drift
/// (ADR 0053), so the subtraction happens inside the query.
#[derive(Debug, Clone)]
pub struct LookbackMatch {
    pub event_type: String,
    pub payload: Value,
    /// Whole seconds between the event and the scan, measured by the database.
    pub age_secs: i64,
}

/// What the *arming lookback* found, bounded.
#[derive(Debug, Clone, Default)]
pub struct ArmingLookback {
    /// Newest first, at most the caller's limit.
    pub matches: Vec<LookbackMatch>,
    /// There were more than the limit. Deliberately a bool rather than a total:
    /// counting them all would mean scanning the whole window, which is the
    /// unbounded read this function exists to avoid.
    pub more: bool,
}

impl ArmingLookback {
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }
}

/// **The arming lookback**: matches that landed in a short window BEFORE the
/// wait was armed, for the tool result to report.
///
/// The third gap, and the one the watermark deliberately does NOT close. A
/// match found here is reported, never delivered (ADR 0047).
///
/// `already_delivered` is the ONE thing suppressed: events this thread was
/// literally handed by an earlier wait. **Nothing coarser is sound.** "Already
/// told" is a property of an individual event under a predicate, not of a point
/// on the timeline, and only a delivery names one. So two overlapping
/// subscriptions may each report the same undelivered event.
///
/// Scans newest first and stops at `limit + 1` matches, so the caller can say
/// "and more" without the total. **Bounded in pages, not just per page**: the
/// condition is evaluated in Rust, so the row `LIMIT` bounds one round trip but
/// not their number. Registration awaits this probe inside the tool call.
///
/// `window_secs` is a duration, NOT a cutoff instant, so the database resolves
/// it against its own clock (ADR 0053). A cutoff the engine computed would
/// silently report nothing once the two clocks drift past the window.
pub async fn arming_lookback_matches(
    pool: &sqlx::PgPool,
    on: &[EventSubscription],
    watermark: i64,
    window_secs: i64,
    already_delivered: &std::collections::HashSet<Uuid>,
    limit: usize,
) -> Result<ArmingLookback, Box<dyn std::error::Error + Send + Sync>> {
    let types: Vec<String> = on.iter().map(|s| s.event_type.clone()).collect();
    if types.is_empty() {
        return Ok(ArmingLookback::default());
    }
    // One past the limit, so `more` can be set without counting the rest.
    let wanted = limit.saturating_add(1);
    let mut found: Vec<LookbackMatch> = Vec::with_capacity(wanted);
    let mut upper = watermark;
    let mut pages = 0;
    while found.len() < wanted {
        if pages == ARMING_LOOKBACK_MAX_PAGES {
            // Not silent: a budget nobody can see reads as "the window was
            // empty", which is the wrong conclusion to hand a debugger.
            crate::log!(
                "[EventWait] Arming lookback hit its {ARMING_LOOKBACK_MAX_PAGES}-page budget \
                 for {types:?}; reporting the {} match(es) found so far",
                found.len(),
            );
            break;
        }
        pages += 1;
        // `now()` on both sides of the window and of the age: the cutoff and
        // the elapsed time are the database's own arithmetic, so neither can be
        // thrown off by the engine host's clock. See the doc comment.
        // `thread_id` for the same reason as the catch-up scan above: it is the
        // column that answers "which thread, or none" across mixed rows.
        let rows: Vec<(Uuid, String, Value, Option<Uuid>, i64, i64)> = sqlx::query_as(
            "SELECT id, event_type, payload, thread_id, \
                    EXTRACT(EPOCH FROM now() - created)::bigint, sequence \
             FROM events \
             WHERE sequence <= $1 \
               AND created >= now() - make_interval(secs => $2) \
               AND event_type = ANY($3) \
             ORDER BY sequence DESC LIMIT $4",
        )
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
            // Matched against the same view as every other path, and REPORTED
            // as that view. The injected `thread_id` tells the model which
            // thread the match belongs to, which is what it needs to scope the
            // wait it arms next.
            let payload = matchable_payload(payload, thread_id);
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

/// The body of the expiry re-entry.
///
/// `never_seen` names any watched event type this workspace has **never**
/// emitted. That is the one moment the fact is worth stating. `await_event`
/// deliberately accepts an unknown name, since it may be a domain event nobody
/// has emitted yet. Registration confirms only that the subscription was
/// accepted, so a typo is invisible until exactly here. Naming it turns a
/// baffling timeout into an obvious one.
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

/// The text a delivery carries. Closes with [`WAIT_SPENT_NOTICE`], which this
/// needs more than the old attached tool result did: it normally starts a fresh
/// turn, so the model has no open call to remind it that it was ever watching.
pub fn delivery_reentry_text(event_type: &str, payload: &Value, reason: &str) -> String {
    format!(
        "An event you subscribed to has arrived (you were waiting because: {reason}).\n\n\
         {event_type}:\n{}{WAIT_SPENT_NOTICE}",
        serde_json::to_string_pretty(payload)
            .unwrap_or_else(|_| "<unserializable payload>".to_string()),
    )
}

/// The text an expiry re-entry carries.
pub fn expiry_reentry_text(wait: &LiveWait, never_seen: &[String]) -> String {
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
pub struct WaitReentry {
    /// The already-persisted `UserPromptInjected` this re-entry anchors to.
    /// Never a fresh `MessageReceived`: nobody said anything.
    pub anchor_event_id: Uuid,
    /// The prompt for the re-entry, the same prose the anchor carries.
    pub text: String,
}

/// What **Stop waiting** did to one wait.
///
/// Three states rather than a bool, because the two failures are not the same
/// thing to a user. A wait that already resolved is gone and the button was
/// stale. A wait whose cancel could not be written is still there and still
/// cancellable. Collapsing them tells the user "already resolved" about a wait
/// that is about to keep waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelWaitOutcome {
    Canceled,
    /// No live wait with that id on that thread.
    NotLive,
    /// The wait is live; persisting the cancel failed.
    EmitFailed,
}

/// One re-entry in flight, as it travels over the engine's re-entry channel.
///
/// Plain data on purpose. Handing the turn to a consumer task, rather than
/// awaiting it in place, keeps `run_agentic_loop`'s future from containing
/// itself. Registration can resolve a wait inline through its catch-up scan.
/// See `WAIT_REENTRY_RX` for the full argument.
#[derive(Debug, Clone)]
pub struct WaitReentryRequest {
    pub thread_id: Uuid,
    pub reentry: WaitReentry,
}

/// Emit a resolution plus its **re-entry anchor**, and report what the
/// dispatcher should re-enter with.
///
/// The anchor is the second event of the pair, and there is exactly one shape:
/// a `UserPromptInjected` carrying the payload as prose. It is an
/// `EXCHANGE_START_TYPES` member on the frontend, so the delivery reads as the
/// new turn it genuinely is, exactly like the child-completion fan-in.
///
/// Emission order matters. The resolution goes first, so a crash between the
/// two leaves a *resolved* wait rather than an anchor for a wait that still
/// looks live. A crash after both is what `lost_wait_reentries` picks up, which
/// is why the anchor is one recognisable event shape.
pub async fn emit_delivery(
    bus: &crate::engine::event_bus::EventBus,
    wait: &LiveWait,
    event_id: Uuid,
    event_type: &str,
    payload: &Value,
    matched_index: usize,
) -> Result<WaitReentry, ResolutionEmitError> {
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

/// Emit the resolution event for an expired wait plus its re-entry anchor. Same
/// shape and same ordering rule as [`emit_delivery`].
pub async fn emit_expiry(
    bus: &crate::engine::event_bus::EventBus,
    wait: &LiveWait,
    never_seen: &[String],
) -> Result<WaitReentry, ResolutionEmitError> {
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

/// Emit the resolution event for a **canceled** wait.
///
/// The one resolution that re-enters nothing, and therefore has no anchor. The
/// user ended the subscription, so the thread is left exactly as it was. There
/// is nothing to close, because `await_event` paired its own call at
/// registration.
///
/// `actor` is the device that ended it, and this is the one resolution that can
/// have one. A delivery and an expiry are the engine acting on its own, while
/// every cancel cause traces back to a person. Stamped per
/// `.claude/rules/rust.md` so the timeline can say who. `None` for the
/// engine-internal causes, whose actor is already on their own event.
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
            // Self-contained, like a delivery: a cancel renders at its own
            // place in the transcript and the `EventWaitStarted` it resolves is
            // routinely outside the loaded window by then.
            on: wait.on.clone(),
            reason: wait.reason.clone(),
        },
        EventMeta::with_actor(actor),
    )
    .await
    .map_err(ResolutionEmitError::Unresolved)?;
    Ok(())
}

/// Why a resolution emit failed, and therefore what the caller owes the wait.
///
/// The two arms are NOT the same recovery. That is the structural
/// justification `.claude/rules/rust.md` asks for a typed error: the caller
/// branches on it rather than formatting it.
///
/// * `Unresolved` means nothing was written. The persisted `EventWaitStarted`
///   is still live, so the caller MUST put the wait back in the cache. Without
///   that, a transient write failure strands the thread. It is gone from the
///   live set, so nothing can match or expire it. It is still live in the event
///   store, so only the next boot would re-arm it.
/// * `AnchorMissing` means the resolution landed but its re-entry anchor did not.
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
            Self::AnchorMissing(e) => write!(f, "the re-entry anchor was not persisted: {e}"),
        }
    }
}

impl std::error::Error for ResolutionEmitError {}

/// The shared body of [`emit_delivery`] and [`emit_expiry`]: resolution first,
/// then its re-entry anchor.
async fn emit_resolution(
    bus: &crate::engine::event_bus::EventBus,
    wait: &LiveWait,
    resolution: ThreadEvent,
    wake_text: impl FnOnce() -> String,
) -> Result<WaitReentry, ResolutionEmitError> {
    // Whether the anchor can point back at this resolution for a structured
    // render. Only a DELIVERY carries the matched event's name and payload as
    // fields. An expiry has nothing but the words its own text already says, so
    // linking one would buy the client nothing. Read before the emit moves it.
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
            // The prose in `text` stays exactly as the model needs it. This is
            // the same delivery addressed to the client instead, so it can name
            // the event and fold the payload away.
            delivered_event_id: resolution_is_delivery.then_some(resolution_id),
        },
    )
    .await
    .map_err(ResolutionEmitError::AnchorMissing)?;
    Ok(WaitReentry {
        anchor_event_id,
        text,
    })
}

/// Emit one persisted thread event and return its row id.
///
/// Errors rather than defaulting when the bus reports no `EmitResult`. Every
/// event here is persisted, so `None` means the write did not happen. A
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
/// far below what anyone waiting tens of minutes will notice. It is cheap too,
/// because the sweep scans an in-memory map holding a handful of entries per
/// subscribed thread.
pub const DEADLINE_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Re-entries the event store says were persisted but never ran (I3b), oldest
/// first.
///
/// The selection is the whole recovery rule, so it lives here on the pool
/// rather than in the engine method that actuates it. It can then be tested
/// against a real database without standing up an engine.
///
/// A resolution's re-entry **never ran** when the very next event on its thread
/// is that resolution's own anchor and nothing follows it. `emit_resolution`
/// writes exactly one anchor shape, so that is the whole exclusion list.
/// Anything else after the resolution means the re-entry WAS consumed, and
/// re-driving it would double-run a turn.
///
/// A legacy attached delivery anchored on the paired `ToolResult` instead, and
/// is deliberately NOT selected. `settle_legacy_attached_event_waits` closes
/// that shape out once, so an old-world re-entry is settled rather than
/// re-driven into a turn with no slot for it. Discarded threads are skipped
/// too: reviving a thread the user threw away, to read an event it no longer
/// cares about, is the archive-curtain problem in another costume.
pub async fn lost_wait_reentries(
    pool: &sqlx::PgPool,
) -> Result<Vec<WaitReentryRequest>, Box<dyn std::error::Error + Send + Sync>> {
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

/// One-off boot sweep for threads caught mid-**attached** wait by the upgrade
/// that removed that shape (ADR 0049).
///
/// Such a thread has an `await_event` `ToolCalled` with no `ToolResult`, which
/// is a provider 400 on its very next turn. Closing the pair is all that is
/// owed. `rebuild_live_waits` re-arms any unresolved wait as an ordinary
/// subscription, and any resolved one has its payload sitting in the events the
/// thread will read anyway.
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
                 hold a turn open: any subscription of yours that is still live will re-open \
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
