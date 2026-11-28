//! INVARIANT: What an agent can do to its OWN subscriptions: read them, and stand them
//! down.
//!
//! `register.rs` is the arming half of the same surface. This is the other two
//! verbs, and they exist because arming was the only one there was. An agent
//! could subscribe and then had no way to answer either question the user asks
//! next:
//!
//! * **"Is that still watching?"** There was no read. On 2026-08-06 a thread
//!   told the user twice that a watch was armed when it had been dead for two
//!   hours, and the only way it could answer at all was to pull four event
//!   types out of the store and diff started against resolved by eye, across
//!   the whole store rather than the thread.
//! * **"Stop watching for that."** There was no revoke. The honest answer was
//!   that the subscription was unrevokable and would re-open the thread later
//!   regardless.
//!
//! # Scoped to the calling thread, on three legs
//!
//! Both verbs address the caller's own subscriptions and take no thread
//! argument, exactly as `await_event` registers against the calling thread.
//! That shape alone is not the guarantee, because the HTTP form the CLI calls
//! has a path segment where the argument is not:
//!
//! 1. **No argument to point elsewhere.** The LLM tools get the thread from
//!    `execute_tool`, and the CLI reads `$LUCIDOS_THREAD_ID`. Neither exposes a
//!    flag, so neither agent can even express another thread.
//! 2. **The route refuses an agent naming a different thread**
//!    (`api::threads::actions::refuse_event_waits_for_another_thread`). A
//!    subprocess carries a thread-bound origin token it cannot re-point, so
//!    substituting an id in the path is caught there rather than trusted. That
//!    leg is what makes leg 1 a guarantee instead of a convention.
//! 3. **A `wait_id` is scoped to its thread too**, via
//!    [`LiveWaits::take_on_thread`], which re-checks membership under the one
//!    lock. So even a correctly-addressed call cannot resolve an id belonging
//!    to a different thread.
//!
//! # One wording, two callers
//!
//! The text here is what both the LLM tool result and the CLI's JSON carry, for
//! the reason registration is shared: two agents reading different words for
//! the same refusal is how the two surfaces drift.

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::{describe_subscriptions, LiveWait};
use crate::engine::thread_events::EventWaitCancelCause;
use crate::engine::LucidosEngine;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EventWaitView {
    pub wait_id: Uuid,
    pub on: Vec<crate::core::event_subscription::EventSubscription>,
    pub subscription: String,
    pub reason: String,
    pub armed_at: DateTime<Utc>,
    pub armed_ago: String,
    pub expires_at: DateTime<Utc>,
    pub expires_in: String,
}

impl EventWaitView {
    fn of(wait: &LiveWait, now: DateTime<Utc>) -> Self {
        Self {
            wait_id: wait.wait_id,
            on: wait.on.clone(),
            subscription: describe_subscriptions(&wait.on),
            reason: wait.reason.clone(),
            armed_at: wait.armed_at,
            armed_ago: humanize_span(now - wait.armed_at),
            expires_at: wait.expires_at,
            expires_in: humanize_span(wait.expires_at - now),
        }
    }
}

fn humanize_span(span: Duration) -> String {
    let secs = span.num_seconds().max(0);
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h {}m", mins % 60);
    }
    format!("{}d {}h", hours / 24, hours % 24)
}

pub enum CancelEventWaitOutcome {
    Stopped(String),
    Refused(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CancelTarget {
    One(Uuid),
    On(String),
    All,
}

pub(crate) fn resolve_cancel_target(
    wait_id: Option<Uuid>,
    on: Option<&str>,
    all: bool,
) -> Result<CancelTarget, String> {
    let on = on.map(str::trim).filter(|s| !s.is_empty());
    match (wait_id, on, all) {
        (None, None, false) => Err(
            "Error: pass `wait_id` to stop one subscription, `on` to stop the ones \
             watching a given event type, or `all: true` to stop every one on this \
             thread. Call list_event_waits first if you do not have the id."
                .to_string(),
        ),
        (Some(id), None, false) => Ok(CancelTarget::One(id)),
        (None, Some(event_type), false) => Ok(CancelTarget::On(event_type.to_string())),
        (None, None, true) => Ok(CancelTarget::All),
        _ => Err(
            "Error: pass exactly one of `wait_id`, `on` or `all`, not several. They \
             address different sets, so naming more than one is ambiguous."
                .to_string(),
        ),
    }
}

pub(crate) fn render_event_wait_list(waits: &[EventWaitView]) -> String {
    if waits.is_empty() {
        return "This thread has no live subscriptions. Nothing will re-open it. \
                If you told the user you were watching for something, that is no \
                longer true: either subscribe again with await_event, or say so."
            .to_string();
    }
    let mut text = format!(
        "{} live subscription(s) on this thread. Each re-opens it once, then is spent:\n",
        waits.len()
    );
    for w in waits {
        text.push_str(&format!(
            "\n- {}\n  watching: {}\n  reason: {}\n  armed {} ago, times out in {}\n",
            w.wait_id, w.subscription, w.reason, w.armed_ago, w.expires_in,
        ));
    }
    text.push_str(
        "\nTo stop one, call cancel_event_wait with its id; to stop all of them, \
         call it with all=true.",
    );
    text
}

/// INVARIANT: What a stop covering SEVERAL subscriptions reports, from what was live
/// before it and what is still live after. `settled` is the sentence that
/// closes the success case, and it is the only thing the `all` and `on` scopes
/// say differently.
///
/// **A partial stop is a refusal, not a success with a caveat**, which is the
/// whole reason this is its own function. Such a stop is one emit per
/// subscription, so one can fail while the rest land, and a failed one is
/// re-armed and will still re-open the thread. Reporting "nothing is subscribed
/// any more" there would be precisely the lie this surface exists to stop the
/// agent telling: it would say a watch was stood down while the watch was
/// still running.
///
/// Pure over the two lists, so every arm is testable without an engine, and
/// keyed on what is STILL LIVE rather than on a success count, because the
/// cache is the thing that decides whether the thread is re-opened. Both lists are
/// scoped to what the call addressed, so an `on` stop is judged on the watches
/// for that event type and is not made to look partial by the unrelated ones it
/// deliberately left alone.
pub(crate) fn stop_outcome(
    before: &[String],
    still_live: &[String],
    settled: &str,
) -> CancelEventWaitOutcome {
    if still_live.len() == before.len() {
        return CancelEventWaitOutcome::Refused(
            "Error: could not record the stop. The subscriptions are still live and \
             will still re-open this thread, so tell the user they are still running."
                .to_string(),
        );
    }
    if !still_live.is_empty() {
        return CancelEventWaitOutcome::Refused(format!(
            "Error: stopped {} of {}, but {} could not be recorded and {} still live \
             and will still re-open this thread: {}. Tell the user which ones are still \
             running, and try again for those.",
            before.len().saturating_sub(still_live.len()),
            before.len(),
            still_live.len(),
            if still_live.len() == 1 { "is" } else { "are" },
            still_live.join("; "),
        ));
    }
    CancelEventWaitOutcome::Stopped(format!(
        "Stopped watching for {}. {settled}",
        before.join("; ")
    ))
}

fn names_watching(waits: &[LiveWait], event_type: &str) -> Vec<String> {
    waits
        .iter()
        .filter(|w| w.watches(event_type))
        .map(|w| describe_subscriptions(&w.on))
        .collect()
}

fn on_stop_settled(event_type: &str, others_still_live: usize) -> String {
    let mut settled = format!("Nothing on this thread watches {event_type} any more.");
    if others_still_live > 0 {
        settled.push_str(&format!(
            " {others_still_live} other subscription(s) on this thread {} still live.",
            if others_still_live == 1 { "is" } else { "are" },
        ));
    }
    settled
}

impl LucidosEngine {
    /// INVARIANT: The calling thread's live subscriptions, newest first.
    ///
    /// Read from the dispatcher's live cache rather than from the event store,
    /// and that is the whole point: the cache IS the set of unresolved
    /// subscriptions, rebuilt from the store at boot, so it cannot disagree
    /// with what will actually re-open the thread. Diffing `EventWaitStarted`
    /// against the three resolutions by hand is what the agent was reduced to,
    /// and it got the answer wrong.
    pub async fn list_event_waits_for_thread(&self, thread_id: Uuid) -> Vec<EventWaitView> {
        let now = Utc::now();
        let mut waits = self.live_waits.for_thread(thread_id).await;
        waits.sort_by(|a, b| b.armed_at.cmp(&a.armed_at));
        waits.iter().map(|w| EventWaitView::of(w, now)).collect()
    }

    pub async fn list_event_waits_text(&self, thread_id: Uuid) -> String {
        render_event_wait_list(&self.list_event_waits_for_thread(thread_id).await)
    }

    pub async fn list_event_waits_tool(
        &self,
        thread_id: Uuid,
        args: &serde_json::Value,
    ) -> Result<String, String> {
        match (args.get("wait_id"), args.get("after")) {
            (None, None) => Ok(self.list_event_waits_text(thread_id).await),
            (Some(id), Some(after)) => {
                let wait_id = id
                    .as_str()
                    .and_then(|id| id.parse().ok())
                    .ok_or("Pass the wait_id from the pending-card report.")?;
                let after = after
                    .as_i64()
                    .filter(|seq| *seq >= 0)
                    .ok_or("Pass the after cursor from the pending-card report.")?;
                self.pending_event_wait_cards(thread_id, wait_id, after)
                    .await
                    .map_err(|e| {
                        crate::log_legacy!(
                            "[EventWait] Pending card page for thread {thread_id} failed: {e}"
                        );
                        "Could not read the pending cards. Retry the same page.".to_string()
                    })?
                    .map(|page| page.tool_text())
                    .ok_or_else(|| {
                        "No sub-thread card subscription with that id on this thread.".into()
                    })
            }
            _ => Err("Pass both wait_id and after to read the next pending-card page.".into()),
        }
    }

    pub async fn cancel_event_waits_for_agent(
        &self,
        thread_id: Uuid,
        wait_id: Option<Uuid>,
        on: Option<&str>,
        all: bool,
    ) -> CancelEventWaitOutcome {
        match resolve_cancel_target(wait_id, on, all) {
            Err(msg) => CancelEventWaitOutcome::Refused(msg),
            Ok(CancelTarget::One(id)) => self.cancel_one_for_agent(thread_id, id).await,
            Ok(CancelTarget::On(event_type)) => {
                self.cancel_watching_for_agent(thread_id, &event_type).await
            }
            Ok(CancelTarget::All) => self.cancel_all_for_agent(thread_id).await,
        }
    }

    async fn cancel_one_for_agent(&self, thread_id: Uuid, wait_id: Uuid) -> CancelEventWaitOutcome {
        let named = self
            .live_waits
            .for_thread(thread_id)
            .await
            .into_iter()
            .find(|w| w.wait_id == wait_id)
            .map(|w| describe_subscriptions(&w.on));
        match self
            .cancel_event_wait(
                thread_id,
                wait_id,
                EventWaitCancelCause::AgentStandDown,
                None,
            )
            .await
        {
            super::CancelWaitOutcome::Canceled => CancelEventWaitOutcome::Stopped(format!(
                "Stopped watching for {}. It will not re-open this thread.",
                named.unwrap_or_else(|| wait_id.to_string()),
            )),
            // INVARIANT: A `wait_id` from another thread lands here too, and deliberately
            // reads the same: this thread has no such subscription, which is
            // the only true thing either case supports.
            super::CancelWaitOutcome::NotLive => CancelEventWaitOutcome::Refused(format!(
                "Error: no live subscription {wait_id} on this thread. It may already have \
                 fired, timed out, or been stopped. Call list_event_waits to see what is \
                 actually live."
            )),
            super::CancelWaitOutcome::EmitFailed => CancelEventWaitOutcome::Refused(format!(
                "Error: could not record the stop for {wait_id}. The subscription is still \
                 live and will still re-open this thread, so tell the user it is still \
                 running rather than that you stood it down."
            )),
        }
    }

    async fn cancel_watching_for_agent(
        &self,
        thread_id: Uuid,
        event_type: &str,
    ) -> CancelEventWaitOutcome {
        let named = names_watching(&self.live_waits.for_thread(thread_id).await, event_type);
        if named.is_empty() {
            return CancelEventWaitOutcome::Refused(format!(
                "Error: nothing on this thread is watching for {event_type}, so there was \
                 nothing to stop. Call list_event_waits to see what is actually live."
            ));
        }
        self.cancel_event_waits_watching(
            thread_id,
            event_type,
            EventWaitCancelCause::AgentStandDown,
            None,
        )
        .await;
        let after = self.live_waits.for_thread(thread_id).await;
        let still_live = names_watching(&after, event_type);
        let others = after.len() - still_live.len();
        stop_outcome(&named, &still_live, &on_stop_settled(event_type, others))
    }

    async fn cancel_all_for_agent(&self, thread_id: Uuid) -> CancelEventWaitOutcome {
        let live = self.live_waits.for_thread(thread_id).await;
        if live.is_empty() {
            return CancelEventWaitOutcome::Refused(
                "Error: this thread has no live subscriptions, so there was nothing to \
                 stop. Nothing was going to re-open it."
                    .to_string(),
            );
        }
        let named: Vec<String> = live.iter().map(|w| describe_subscriptions(&w.on)).collect();
        self.cancel_event_waits_for_thread(thread_id, EventWaitCancelCause::AgentStandDown, None)
            .await;
        // INVARIANT: What is STILL live decides the answer, not how many emits reported
        // success: the cache is the thing that will or will not re-open this
        // thread, and a cancel whose emit failed is put straight back into it.
        let still_live: Vec<String> = self
            .live_waits
            .for_thread(thread_id)
            .await
            .iter()
            .map(|w| describe_subscriptions(&w.on))
            .collect();
        stop_outcome(
            &named,
            &still_live,
            "Nothing is subscribed on this thread any more.",
        )
    }
}

#[cfg(test)]
#[path = "agent_surface_tests.rs"]
mod tests;
