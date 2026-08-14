//! Registration: what happens when the model calls `await_event`.
//!
//! Everything here runs synchronously inside the tool call, which is the whole
//! reason a refusal is worth having. A trigger that subscribes to `TextStreamed`
//! validates, persists and then never fires, and the user finds out days later;
//! `await_event` hands the model an error in the same turn and it picks another
//! event.
//!
//! # Four refusals, three of them caps
//!
//! * **The subscribability gate** (S3), via `validate_awaitable_event_type`.
//! * **The consecutive-subscription cap** (S8): 10 registrations with no human
//!   message in between. Catches a thread awaiting an event kind its own
//!   re-entry emits, two threads ping-ponging, and a model simply stuck.
//! * **The live-wait cap** (S6b): 25 simultaneous waits per thread. It bounds
//!   how many separate re-entries one burst of events can start on one thread,
//!   not what a sleeping subscription costs, which is nothing.
//! * **The duplicate refusal** (S6b): the same `on` list twice on one thread.
//!   One event would then be delivered twice.

use chrono::{Duration, Utc};
use serde_json::Value;
use uuid::Uuid;

use super::LiveWait;
use crate::core::event_subscription::{validate_awaitable_event_type, EventSubscription};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::LucidosEngine;

/// Ceiling on `timeout_secs`. There is no unbounded wait: a wait that outlives
/// every reason anyone had for it is indistinguishable from a stalled thread.
pub(crate) const MAX_TIMEOUT_SECS: i64 = 24 * 60 * 60;

/// How many times a thread may subscribe with no human `MessageReceived` in
/// between (S8). Mirrors `MAX_EVENT_TRIGGER_DEPTH` in intent: the events still
/// persist, the fan-out just stops.
pub(crate) const MAX_CONSECUTIVE_SUBSCRIPTIONS: i64 = 10;

/// How many waits one thread may hold at once (S6b).
///
/// **It bounds outstanding RE-ENTRIES, not accumulated watchers.** A sleeping wait
/// is not a running anything: it costs one entry in the live cache, one
/// `EventWaitStarted` row, and one name compare plus a condition eval per
/// emitted event. What the number actually limits is how many separate turns
/// one burst of events can start on a single thread, because every wait that
/// fires opens one.
///
/// It is NOT the guard against a runaway thread.
/// [`MAX_CONSECUTIVE_SUBSCRIPTIONS`] is, and that one bounds a *loop*: a thread
/// re-opening itself spends a turn per iteration with no human in it. This cap adds
/// exactly one thing that one does not. The consecutive counter resets on a
/// human message, so the standing set a thread carries ACROSS many messages is
/// bounded here and nowhere else.
///
/// Raised from 5 on 2026-08-12. Five was sized for a shape that no longer
/// exists: it arrived with the *attached wait*, where "at most one of them is
/// attached; the rest are background subscriptions that survived an
/// interruption", so the number bounded how many leftovers could pile up behind
/// the one holding the turn. ADR 0049 retired attachment and every wait became
/// an ordinary background subscription, which left the number without its
/// reason.
///
/// It was also refusing the case the mechanism exists for. "Wait until the
/// running coding agents finish" wants one wait per live session, so six
/// running agents hit the limit, and the workaround (one wait whose `on` names
/// every session) is only available because that list is itself uncapped. Which
/// is the other reason five was wrong: it never bounded what a thread WATCHES,
/// since one wait can name twenty event/condition pairs. Nor is it the lever on
/// matching cost, which is workspace-wide: the dispatcher matches every live
/// wait in the workspace against every event, and nothing bounds how many
/// threads there are.
pub(crate) const MAX_LIVE_WAITS_PER_THREAD: usize = 25;

/// How far back registration looks for a match that landed while the model was
/// still working towards this call (the **arming lookback**, see
/// `docs/plans/2026-08-06-await-event-covers-the-observe-then-arm-gap.md`).
///
/// Sized to the gap it covers, and deliberately not to any structural boundary.
/// The gap is between the model deciding to wait and the call landing: on
/// 2026-08-06 that was 84 seconds, spent composing and spawning an unrelated
/// thread. Three minutes is roughly double that.
///
/// The turn is NOT the boundary, and that is the whole reason this is a
/// constant. A turn ran from 17:39 to 19:14 that day driving a release build,
/// and a model decides to subscribe mid-turn: it can form the intent at minute
/// 88 of a ninety-minute turn, having never looked at that state before, in
/// which case events from minute 2 are archaeology rather than a missed
/// rendezvous. Tight on purpose: a slower check-then-arm falls outside and is
/// simply not reported, which is exactly today's behaviour, while a longer
/// window starts surfacing work the model did earlier in the same stretch.
pub(crate) const ARMING_LOOKBACK_SECS: i64 = 3 * 60;

/// How many lookback matches the report names before it just says there were
/// more. A tool result is read in full by the model, so a busy window must not
/// turn one registration into a wall of payloads.
pub(crate) const ARMING_LOOKBACK_MAX_REPORTED: usize = 3;

/// What `await_event` did.
///
/// Both arms carry the tool result the model reads, and in both the turn
/// carries on: `await_event` registers a subscription and returns, it does not
/// end the turn. The delivery arrives later, normally as its own turn, so
/// nothing is left dangling here.
pub(crate) enum AwaitEventOutcome {
    /// The wait is registered.
    Registered(String),
    /// Nothing was registered.
    Refused(String),
}

impl LucidosEngine {
    /// Validate an `await_event` call, and register the wait if it passes.
    pub(crate) async fn register_event_wait(
        &self,
        thread_id: Uuid,
        tool_use_id: &str,
        args: &Value,
    ) -> AwaitEventOutcome {
        let on = match parse_subscriptions(args) {
            Ok(on) => on,
            Err(msg) => return AwaitEventOutcome::Refused(msg),
        };
        let timeout_secs = match parse_timeout_secs(args) {
            Ok(secs) => secs,
            Err(msg) => return AwaitEventOutcome::Refused(msg),
        };
        let reason = args
            .get("reason")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or_default();
        if reason.is_empty() {
            return AwaitEventOutcome::Refused(
                "Error: `reason` is required. One short line saying what you are waiting \
                 for and why, in the user's language. The user reads it in the \
                 subscription indicator, and it is how they tell a sleeping thread from \
                 a stalled one."
                    .to_string(),
            );
        }

        if let Some(msg) = self.event_wait_caps_refusal(thread_id, &on).await {
            return AwaitEventOutcome::Refused(msg);
        }

        let wait = match self
            .build_wait(thread_id, tool_use_id, on, reason, timeout_secs)
            .await
        {
            Ok(w) => w,
            Err(e) => {
                return AwaitEventOutcome::Refused(format!(
                    "Error: could not register the wait ({e}). Try again, or fall back to \
                     checking the state yourself."
                ))
            }
        };

        // The arming lookback. Bounded at `sequence <= watermark`, so it cannot
        // see anything this registration is about to write and its position
        // relative to the emit below is not load-bearing. It sits here so a
        // refused call (the caps above) does no lookback work.
        let lookback = self.arming_lookback(&wait).await;

        if let Err(e) = self.commit_wait(&wait).await {
            return AwaitEventOutcome::Refused(format!(
                "Error: could not register the wait ({e}). Try again, or fall back to \
                 checking the state yourself."
            ));
        }

        AwaitEventOutcome::Registered(registered_tool_result_text(&wait, lookback.as_ref()))
    }

    /// Read the watermark and build the [`LiveWait`]. Nothing is emitted or
    /// cached yet, so a caller may still abandon it.
    ///
    /// The watermark is read BEFORE the emit in [`Self::commit_wait`], so the
    /// catch-up scan (`sequence > watermark`) covers everything from this
    /// instant on, including anything landing while the emit is in flight. It
    /// re-reads `EventWaitStarted` itself, which is harmless: that name can
    /// never be a subscribed type (the subscribability gate refuses it).
    pub(super) async fn build_wait(
        &self,
        thread_id: Uuid,
        tool_use_id: &str,
        on: Vec<EventSubscription>,
        reason: &str,
        timeout_secs: i64,
    ) -> Result<LiveWait, Box<dyn std::error::Error + Send + Sync>> {
        let watermark = self.read_watermark(thread_id).await?;
        Ok(self.build_wait_at(thread_id, tool_use_id, on, reason, timeout_secs, watermark))
    }

    /// The event store's high-water sequence, to be used as a wait's watermark.
    ///
    /// Separate from [`Self::build_wait`] for the ONE caller that must read it
    /// earlier than the rest of its own inputs: see
    /// [`Self::build_wait_at`].
    pub(super) async fn read_watermark(
        &self,
        thread_id: Uuid,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        self.latest_event_sequence().await.inspect_err(|e| {
            crate::log!("[EventWait] Watermark read failed for thread {thread_id}: {e}");
        })
    }

    /// [`Self::build_wait`] with the watermark supplied rather than read here.
    ///
    /// **The watermark must be read before whatever state decided the `on`
    /// list.** The catch-up scan is `sequence > watermark`, so any matching
    /// event that landed at or below it is invisible to this wait: it will not
    /// match, and the thread sits until the timeout. Reading the watermark
    /// afterwards leaves exactly that gap, sized by everything in between.
    ///
    /// `register_event_wait` has no gap to worry about, because the model's
    /// `on` list arrives with the tool call and the watermark is the first
    /// thing read after it (what covers the model's own observe-then-arm gap is
    /// the separate arming lookback). The engine-armed background-task wait
    /// does: it decides `on` from the task registry, and asks the database for
    /// the subscription count in between, so a task completing across those
    /// milliseconds would be armed for and then never delivered, which is the
    /// exact stall the wait exists to prevent.
    pub(super) fn build_wait_at(
        &self,
        thread_id: Uuid,
        tool_use_id: &str,
        on: Vec<EventSubscription>,
        reason: &str,
        timeout_secs: i64,
        watermark: i64,
    ) -> LiveWait {
        let armed_at = Utc::now();
        LiveWait {
            wait_id: Uuid::new_v4(),
            thread_id,
            tool_use_id: tool_use_id.to_string(),
            on,
            reason: reason.to_string(),
            armed_at,
            expires_at: armed_at + Duration::seconds(timeout_secs),
            watermark,
        }
    }

    /// Persist a built wait and make it live: emit `EventWaitStarted`, insert
    /// it into the cache, then run the catch-up scan.
    ///
    /// The scan is the same one the boot rebuild runs, and here it closes the
    /// live race: an event emitted between the watermark read and the insert
    /// was offered to a cache that did not yet hold this wait. It can therefore
    /// resolve the wait before this call returns, which is fine, and is why the
    /// caller's tool result is written in the future tense without promising
    /// the thread is still subscribed by the time the model reads it. The
    /// delivery queues behind the current turn either way.
    pub(super) async fn commit_wait(
        &self,
        wait: &LiveWait,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: wait.thread_id,
                event: ThreadEvent::EventWaitStarted {
                    wait_id: wait.wait_id,
                    tool_use_id: wait.tool_use_id.clone(),
                    on: wait.on.clone(),
                    reason: wait.reason.clone(),
                    armed_at: wait.armed_at,
                    expires_at: wait.expires_at,
                    watermark: wait.watermark,
                },
                meta: EventMeta::NONE,
            })
            .await
            .inspect_err(|e| {
                crate::log!(
                    "[EventWait] EventWaitStarted emit failed for thread {}: {e}",
                    wait.thread_id
                );
            })?;

        crate::log!(
            "[EventWait] Thread {} subscribed to {:?} for {}s (wait {})",
            wait.thread_id,
            wait.on.iter().map(|s| &s.event_type).collect::<Vec<_>>(),
            (wait.expires_at - wait.armed_at).num_seconds(),
            wait.wait_id,
        );
        self.live_waits.insert(wait.clone()).await;
        self.catch_up_event_wait(wait).await;
        Ok(())
    }

    /// The three caps, in the order that gives the model the most useful
    /// message when several would fire.
    async fn event_wait_caps_refusal(
        &self,
        thread_id: Uuid,
        on: &[EventSubscription],
    ) -> Option<String> {
        let live = self.live_waits.for_thread(thread_id).await;
        if live.iter().any(|w| w.on == on) {
            return Some(format!(
                "Error: you are already waiting on exactly this ({}). That subscription is \
                 still live and will re-open this thread when it matches, so registering it \
                 again would deliver one event to you twice. Wait for it, or watch something \
                 else.",
                describe_subscriptions(on),
            ));
        }
        if live.len() >= MAX_LIVE_WAITS_PER_THREAD {
            return Some(format!(
                "Error: this thread already holds {} live subscriptions, which is the \
                 limit. They are: {}. Let one resolve, or tell the user what you are \
                 waiting for and stop.",
                live.len(),
                live.iter()
                    .map(|w| describe_subscriptions(&w.on))
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        match consecutive_subscriptions(&self.pool, thread_id).await {
            Ok(n) if n >= MAX_CONSECUTIVE_SUBSCRIPTIONS => Some(format!(
                "Error: this thread has subscribed {n} times in a row with no message from \
                 the user, which is the limit. Either this thread keeps re-opening itself, \
                 or what you are waiting for is not coming. Report where things stand and \
                 let the user decide."
            )),
            Ok(_) => None,
            Err(e) => {
                // A cap that cannot be evaluated must not silently become no
                // cap: an unreadable event store is exactly when a runaway
                // loop would do the most damage.
                crate::log!(
                    "[EventWait] Subscription-count read failed for thread {thread_id}: {e}"
                );
                Some(format!(
                    "Error: could not check the wait limits for this thread ({e}). Report \
                     where things stand instead of waiting."
                ))
            }
        }
    }

    /// Run the **arming lookback** for a wait about to be registered.
    ///
    /// Returns `None` when there is nothing to report AND when the probe could
    /// not run. Collapsing those two is deliberate and is the fail-open half of
    /// `.claude/rules/rust.md`'s unknown-state rule: the report is advisory, so
    /// a database hiccup must cost the model a note, never the subscription it
    /// asked for. The failure is logged rather than swallowed.
    async fn arming_lookback(
        &self,
        wait: &LiveWait,
    ) -> Option<crate::engine::event_wait::ArmingLookback> {
        // Both halves are given the WINDOW, and each resolves it against the
        // database clock, because `created` is written by the database (see
        // `arming_lookback_matches`). The exclusion set is read FIRST on
        // purpose: its `now()` is therefore the earlier of the two, so its
        // window is the wider one and it cannot fail to cover an event the
        // scan below is willing to report. Reversing these two calls would
        // leave a sliver in which an already-handed event is reported again.
        let delivered =
            match delivered_event_ids(&self.pool, wait.thread_id, ARMING_LOOKBACK_SECS).await {
                Ok(ids) => ids,
                Err(e) => {
                    // Not an empty set: without the exclusion the lookback would
                    // re-report an event this thread was already handed, so an
                    // unreadable exclusion means no report at all.
                    crate::log!(
                        "[EventWait] Lookback delivered-set read failed for thread {}: {e}",
                        wait.thread_id
                    );
                    return None;
                }
            };
        match crate::engine::event_wait::arming_lookback_matches(
            &self.pool,
            &wait.on,
            wait.watermark,
            ARMING_LOOKBACK_SECS,
            &delivered,
            ARMING_LOOKBACK_MAX_REPORTED,
        )
        .await
        {
            Ok(found) if found.is_empty() => None,
            Ok(found) => Some(found),
            Err(e) => {
                crate::log!(
                    "[EventWait] Lookback scan failed for thread {}: {e}",
                    wait.thread_id
                );
                None
            }
        }
    }

    /// The event store's current high-water sequence, used as a wait's
    /// watermark.
    async fn latest_event_sequence(&self) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let seq: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM events")
            .fetch_one(&self.pool)
            .await?;
        Ok(seq)
    }

    /// Has this workspace ever emitted an event by this name? Drives the
    /// never-seen note on an unknown name (S3): accepted, because it may be a
    /// domain event nobody has emitted yet, but worth saying out loud so the
    /// model does not watch for 24 hours for a typo.
    pub(crate) async fn event_type_seen_before(&self, event_type: &str) -> bool {
        event_type_ever_emitted(&self.pool, event_type).await
    }
}

/// The `ToolResult` text a successful registration returns.
///
/// It has one job beyond confirming the subscription: telling the model that
/// nothing is blocking, so it finishes the turn instead of stalling for a
/// delivery that will arrive as its own turn much later. Naming the
/// re-registration ban here is the cheap half of the duplicate refusal in
/// `event_wait_caps_refusal`.
///
/// The **arming lookback** leads when there is one, because it is the only part
/// of this result the model has to act on within this turn: the subscription
/// watches forward, so a match from before it was armed will never be delivered
/// and reading past it is how the 2026-08-06 change went unapplied.
pub(super) fn registered_tool_result_text(
    wait: &LiveWait,
    lookback: Option<&crate::engine::event_wait::ArmingLookback>,
) -> String {
    let subscribed = format!(
        "Subscribed to {}. Nothing is blocking: finish this turn and end your response \
         normally. The match reaches you as a NEW turn, or a timeout notice does at the \
         deadline you set. Do not call await_event again for this.",
        describe_subscriptions(&wait.on),
    );
    match lookback.filter(|l| !l.is_empty()) {
        None => subscribed,
        Some(found) => format!("{}\n\n{subscribed}", arming_lookback_notice(found)),
    }
}

/// The report itself: what already happened, how long ago, and what the model
/// owes it.
///
/// Written as an instruction rather than a fact because the fact alone is what
/// the model already had and did not act on. It states the trap explicitly:
/// this subscription will not deliver these, so a turn that ends here ends with
/// the event unhandled.
fn arming_lookback_notice(found: &crate::engine::event_wait::ArmingLookback) -> String {
    let mut text = String::from(
        "ALREADY HAPPENED, before this subscription existed. Your subscription watches \
         FORWARD only, so it will NOT deliver anything below. Decide now, in this \
         turn: if one of these is what you were waiting for, act on it before you finish. \
         If you already handled it earlier in this turn, ignore it and carry on.\n",
    );
    for m in &found.matches {
        text.push_str(&format!(
            "\n{} ({} ago):\n{}\n",
            m.event_type,
            humanize_age(m.age_secs),
            capped_payload(&m.payload),
        ));
    }
    if found.more {
        text.push_str(
            "\nMore matched than are shown; these are the most recent. If that many are \
             arriving, what you want is probably a narrower `condition`, or a trigger.\n",
        );
    }
    text
}

/// One reported payload, pretty-printed and bounded.
///
/// The notice rides on a call the model made only to REGISTER, so its whole
/// budget is a couple of sentences plus enough of each payload to recognise the
/// event. Three uncapped ones would be tens of KB for a fat type (a
/// `ResponseGenerated`, a `ChangeProposed` with a long file list) spent on a
/// tool result that mostly says "subscribed". The identity and the age are what
/// the notice tells the model to act on, and both survive the cut.
///
/// Cut on a char boundary per `.claude/rules/rust.md`: a payload is arbitrary
/// user or workspace text, so a byte index lands mid-character eventually.
fn capped_payload(payload: &Value) -> String {
    const MAX_BYTES: usize = 1200;
    const MARKER: &str = "\n… (payload truncated)";

    let pretty = serde_json::to_string_pretty(payload)
        .unwrap_or_else(|_| "<unserializable payload>".to_string());
    if pretty.len() <= MAX_BYTES {
        return pretty;
    }
    let cut = pretty.floor_char_boundary(MAX_BYTES.saturating_sub(MARKER.len()));
    format!("{}{MARKER}", &pretty[..cut])
}

/// How long ago, at the granularity the lookback window makes meaningful.
///
/// Whole seconds up to a minute, then minutes and seconds. The window is a few
/// minutes wide, so anything coarser would render every match as "0m" and the
/// age is the whole basis on which the model tells a miss from its own work.
fn humanize_age(age_secs: i64) -> String {
    let secs = age_secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

/// Events this thread has already been **handed** by an earlier wait, among
/// those recent enough for the arming lookback to consider.
///
/// The lookback's one suppression, and the only sound one. A delivery names an
/// exact `event_id`, so excluding it says precisely "the thread has seen this
/// event" and nothing more. Everything coarser that was tried here suppressed
/// events nobody had reported: see [`arming_lookback_matches`] for the two
/// sequence floors that died and why.
///
/// Scoped to the same `window_secs`, which loses nothing: a delivery is always
/// at or after the event it delivers, so any delivery of an event inside the
/// window is itself inside the window. A **duration** rather than a cutoff
/// instant for the reason [`arming_lookback_matches`] spells out: `created` is
/// the database's clock, so the boundary has to be too.
///
/// A free function on the pool, matching [`consecutive_subscriptions`], so the
/// SQL can be tested against a real database without standing up an engine.
/// `aggregate = 'thread'` is the same load-bearing guard `LIVE_WAITS_SQL`
/// documents. A row whose `event_id` will not parse is skipped rather than
/// failing the read: events are append-only, so one malformed payload would
/// otherwise disable the lookback on this thread permanently.
pub(crate) async fn delivered_event_ids(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    window_secs: i64,
) -> Result<std::collections::HashSet<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    let rows: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT payload->>'event_id' FROM events \
         WHERE aggregate = 'thread' \
           AND aggregate_id = $1 \
           AND event_type = 'EventWaitDelivered' \
           AND created >= now() - make_interval(secs => $2)",
    )
    .bind(thread_id.to_string())
    .bind(window_secs)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id,)| id?.parse::<Uuid>().ok())
        .collect())
}

/// `EventWaitStarted` events since the last **human** message on this thread.
/// The S8 counter, derived from events, with no new state.
///
/// Human specifically: an agent- or engine-authored `MessageReceived` (a child
/// callback, a trigger fire, an event delivery) is exactly the kind of traffic a
/// ping-pong loop generates, so counting it would reset the very counter it
/// should be tripping.
///
/// A free function on the pool rather than a method, so the SQL that carries
/// the whole cap can be tested against a real database without standing up an
/// engine.
pub(crate) async fn consecutive_subscriptions(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events e \
         WHERE e.aggregate = 'thread' \
           AND e.aggregate_id = $1 \
           AND e.event_type = 'EventWaitStarted' \
           AND e.sequence > COALESCE(( \
               SELECT MAX(m.sequence) FROM events m \
               WHERE m.aggregate = 'thread' \
                 AND m.aggregate_id = $1 \
                 AND m.event_type = 'MessageReceived' \
                 AND m.payload->>'mode' = 'human' \
           ), 0)",
    )
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// Has this workspace ever emitted an event by this name?
///
/// **An unreadable store answers "seen", which suppresses the note.** The note
/// is advisory, and a false alarm claiming a real event type has never been
/// emitted would mislead the model worse than saying nothing.
pub(crate) async fn event_type_ever_emitted(pool: &sqlx::PgPool, event_type: &str) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM events WHERE event_type = $1)")
        .bind(event_type)
        .fetch_one(pool)
        .await
        .unwrap_or(true)
}

/// Parse and validate the `on:` array. Every entry is checked against the
/// shared subscribability gate, and the first failure refuses the whole call:
/// a partially-registered wait would watch for less than the model asked for
/// while reading as a success.
fn parse_subscriptions(args: &Value) -> Result<Vec<EventSubscription>, String> {
    let Some(entries) = args.get("on").and_then(|v| v.as_array()) else {
        return Err(
            "Error: `on` must be an array of {event_type, condition?} objects saying what \
             to watch for."
                .to_string(),
        );
    };
    if entries.is_empty() {
        return Err(
            "Error: `on` is empty, so nothing could ever match. Name at least one event."
                .to_string(),
        );
    }
    let mut subs = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(obj) = entry.as_object() else {
            return Err(format!(
                "Error: every `on` entry must be an object with an `event_type`, got {entry}."
            ));
        };
        let Some(sub) = EventSubscription::from_object_entry(obj) else {
            return Err(format!(
                "Error: `on` entry {entry} has no usable `event_type`."
            ));
        };
        validate_awaitable_event_type(&sub.event_type).map_err(|e| format!("Error: {e}"))?;
        subs.push(sub);
    }
    Ok(subs)
}

/// Parse `timeout_secs`. Required, and bounded at both ends.
fn parse_timeout_secs(args: &Value) -> Result<i64, String> {
    let Some(raw) = args.get("timeout_secs") else {
        return Err(format!(
            "Error: `timeout_secs` is required (1 to {MAX_TIMEOUT_SECS}). There is no \
             unbounded wait: pick an upper bound for the thing you are waiting on and add \
             margin. You get a timeout notice if nothing matches."
        ));
    };
    let Some(secs) = raw.as_i64() else {
        return Err(format!(
            "Error: `timeout_secs` must be a whole number of seconds, got {raw}."
        ));
    };
    if secs < 1 {
        return Err(format!(
            "Error: `timeout_secs` must be at least 1 second, got {secs}."
        ));
    }
    if secs > MAX_TIMEOUT_SECS {
        return Err(format!(
            "Error: `timeout_secs` is capped at {MAX_TIMEOUT_SECS} (24 hours), got {secs}. \
             For anything longer, a trigger is the right shape: it is a standing rule that \
             outlives this conversation."
        ));
    }
    Ok(secs)
}

/// Human-readable form of an `on:` list, for a refusal the model has to act on.
pub(crate) fn describe_subscriptions(on: &[EventSubscription]) -> String {
    on.iter()
        .map(|s| match &s.condition {
            Some(c) => format!("{} where {}", s.event_type, c),
            None => s.event_type.clone(),
        })
        .collect::<Vec<_>>()
        .join(" or ")
}

#[cfg(test)]
#[path = "register_tests.rs"]
mod tests;
