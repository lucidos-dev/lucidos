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
//!   message in between. Catches a thread awaiting an event kind its own wake
//!   emits, two threads ping-ponging, and a model simply stuck.
//! * **The live-wait cap** (S6b): 5 simultaneous waits per thread, so a
//!   subscribe / wake / subscribe cycle cannot accumulate watchers without
//!   bound.
//! * **The duplicate refusal** (S6b): the same `on` list twice on one thread.
//!   One event would then produce two wakes.

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

/// How many waits one thread may hold at once (S6b). All of them are ordinary
/// background subscriptions: none holds the thread's turn, so this bounds how
/// many watchers one thread can accumulate, nothing more.
pub(crate) const MAX_LIVE_WAITS_PER_THREAD: usize = 5;

/// What `await_event` did.
///
/// Both arms carry the tool result the model reads, and in both the turn
/// carries on: `await_event` registers a subscription and returns, it does not
/// end the turn. The wake arrives later as its own turn, so nothing is left
/// dangling here.
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

        // Read the watermark BEFORE emitting, so the catch-up scan
        // (`sequence > watermark`) covers everything from this instant on,
        // including anything that lands while the emit is still in flight. It
        // re-reads `EventWaitStarted` itself, which is harmless: that name can
        // never be one of the subscribed types (the gate above refuses it).
        let watermark = match self.latest_event_sequence().await {
            Ok(seq) => seq,
            Err(e) => {
                crate::log!("[EventWait] Watermark read failed for thread {thread_id}: {e}");
                return AwaitEventOutcome::Refused(format!(
                    "Error: could not register the wait ({e}). Try again, or fall back to \
                     checking the state yourself."
                ));
            }
        };

        let wait = LiveWait {
            wait_id: Uuid::new_v4(),
            thread_id,
            tool_use_id: tool_use_id.to_string(),
            on,
            reason: reason.to_string(),
            expires_at: Utc::now() + Duration::seconds(timeout_secs),
            watermark,
        };

        if let Err(e) = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: ThreadEvent::EventWaitStarted {
                    wait_id: wait.wait_id,
                    tool_use_id: wait.tool_use_id.clone(),
                    on: wait.on.clone(),
                    reason: wait.reason.clone(),
                    expires_at: wait.expires_at,
                    watermark: wait.watermark,
                },
                meta: EventMeta::NONE,
            })
            .await
        {
            crate::log!("[EventWait] EventWaitStarted emit failed for thread {thread_id}: {e}");
            return AwaitEventOutcome::Refused(format!(
                "Error: could not register the wait ({e}). Try again, or fall back to \
                 checking the state yourself."
            ));
        }

        crate::log!(
            "[EventWait] Thread {} subscribed to {:?} for {}s (wait {})",
            thread_id,
            wait.on.iter().map(|s| &s.event_type).collect::<Vec<_>>(),
            timeout_secs,
            wait.wait_id,
        );
        let registered = registered_tool_result_text(&wait);
        self.live_waits.insert(wait.clone()).await;
        // Same scan the boot rebuild runs, and here it closes the live race:
        // an event emitted between the watermark read and the insert above was
        // offered to a cache that did not hold this wait yet.
        //
        // It can therefore resolve the wait before this call has even returned,
        // which is fine and is why the text below is written in the future
        // tense without promising the thread is still subscribed by the time
        // the model reads it: the wake queues behind this turn either way.
        self.catch_up_event_wait(&wait).await;
        AwaitEventOutcome::Registered(registered)
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
                 still live and will wake this thread when it matches, so registering it \
                 again would wake you twice for one event. Wait for it, or watch something \
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
                 the user, which is the limit. Either you are waking yourself, or what you \
                 are waiting for is not coming. Report where things stand and let the user \
                 decide."
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
pub(super) fn registered_tool_result_text(wait: &LiveWait) -> String {
    format!(
        "Subscribed to {}. Nothing is blocking: finish this turn and end your response \
         normally. You will be woken as a NEW turn when it matches, or told it timed out \
         at the deadline you set. Do not call await_event again for this.",
        describe_subscriptions(&wait.on),
    )
}

/// `EventWaitStarted` events since the last **human** message on this thread.
/// The S8 counter, derived from events, with no new state.
///
/// Human specifically: an agent- or engine-authored `MessageReceived` (a child
/// wake, a trigger fire, an event wake) is exactly the kind of traffic a
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
             to wake on."
                .to_string(),
        );
    };
    if entries.is_empty() {
        return Err(
            "Error: `on` is empty, so nothing could ever wake you. Name at least one event."
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
             margin. You are woken with a timeout if nothing matches."
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
