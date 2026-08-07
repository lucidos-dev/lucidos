//! What an agent can do to its OWN subscriptions: read them, and stand them
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
//!   that the subscription was unrevokable and would wake the thread later
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

/// One live subscription, as the agent reads it.
///
/// Six fields, and each answers a question the agent was asked and could not
/// answer: which one (`wait_id`, the handle the cancel takes), what it watches
/// (`on`, conditions included), why (`reason`), and how long it has been and
/// has left (`armed_at` / `expires_at`, plus the two ages spelled out because
/// a timestamp is the thing a model is worst at subtracting).
#[derive(Debug, Clone, serde::Serialize)]
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

/// A duration a model can act on without arithmetic: `18s`, `4m`, `2h 5m`,
/// `1d 3h`. Never negative, so an overdue deadline reads as `0s` rather than as
/// a minus sign the model has to interpret.
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

/// What `cancel_event_wait` did. Both arms carry the text the agent reads.
pub(crate) enum CancelEventWaitOutcome {
    Stopped(String),
    /// Nothing was stopped: a bad argument, or an id that is not live on this
    /// thread. Reported as an error so the agent corrects itself in the same
    /// turn rather than telling the user it stood down when it did not.
    Refused(String),
}

/// Which subscriptions a cancel call names.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CancelTarget {
    One(Uuid),
    All,
}

/// Read `wait_id` / `all` into a target, or refuse.
///
/// A pure seam so both refusals are testable without an engine, and so the two
/// callers (the LLM tool and the HTTP route) cannot disagree about which
/// argument combinations are legal.
///
/// The two are mutually exclusive and one is required. Neither silent default
/// is right for a destructive verb: defaulting a bare call to `all` would stop
/// four subscriptions when the agent meant one, and defaulting it to a no-op
/// would report success for nothing.
pub(crate) fn resolve_cancel_target(
    wait_id: Option<Uuid>,
    all: bool,
) -> Result<CancelTarget, String> {
    match (wait_id, all) {
        (Some(_), true) => Err(
            "Error: pass either `wait_id` or `all`, not both. `all` stops every \
             subscription on this thread, so naming one alongside it is ambiguous."
                .to_string(),
        ),
        (None, false) => Err(
            "Error: pass `wait_id` to stop one subscription, or `all: true` to stop \
             every one on this thread. Call list_event_waits first if you do not have \
             the id."
                .to_string(),
        ),
        (Some(id), false) => Ok(CancelTarget::One(id)),
        (None, true) => Ok(CancelTarget::All),
    }
}

/// The `list_event_waits` result: the live set as prose the model reads rather
/// than JSON it has to parse.
///
/// Pure over the views, so the wording is testable without a database. The
/// empty case is not a bare "none": a thread that thinks it is watching and is
/// not is the exact failure this tool exists to fix, so the text says what that
/// means and what to do about it.
pub(crate) fn render_event_wait_list(waits: &[EventWaitView]) -> String {
    if waits.is_empty() {
        return "This thread has no live subscriptions. Nothing will wake it. \
                If you told the user you were watching for something, that is no \
                longer true: either subscribe again with await_event, or say so."
            .to_string();
    }
    let mut text = format!(
        "{} live subscription(s) on this thread. Each wakes it once, then is spent:\n",
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

/// What an `all` stop reports, from what was live before it and what is still
/// live after.
///
/// **A partial stop is a refusal, not a success with a caveat**, which is the
/// whole reason this is its own function. An `all` stop is one emit per
/// subscription, so one can fail while the rest land, and a failed one is
/// re-armed and will still wake the thread. Reporting "nothing is subscribed
/// any more" there would be precisely the lie this surface exists to stop the
/// agent telling: it would say a watch was stood down while the watch was
/// still running.
///
/// Pure over the two lists, so every arm is testable without an engine, and
/// keyed on what is STILL LIVE rather than on a success count, because the
/// cache is the thing that decides whether the thread wakes.
pub(crate) fn all_stop_outcome(before: &[String], still_live: &[String]) -> CancelEventWaitOutcome {
    if still_live.len() == before.len() {
        return CancelEventWaitOutcome::Refused(
            "Error: could not record the stop. The subscriptions are still live and \
             will still wake this thread, so tell the user they are still running."
                .to_string(),
        );
    }
    if !still_live.is_empty() {
        return CancelEventWaitOutcome::Refused(format!(
            "Error: stopped {} of {}, but {} could not be recorded and {} still live \
             and will still wake this thread: {}. Tell the user which ones are still \
             running, and try again for those.",
            // Saturating because this is arithmetic on two independent reads of
            // a shared cache. Nothing can grow the set mid-call today (a thread
            // runs one turn at a time), but an underflow here would panic
            // inside a tool call rather than misreport by one.
            before.len().saturating_sub(still_live.len()),
            before.len(),
            still_live.len(),
            if still_live.len() == 1 { "is" } else { "are" },
            still_live.join("; "),
        ));
    }
    CancelEventWaitOutcome::Stopped(format!(
        "Stopped watching for {}. Nothing is subscribed on this thread any more.",
        before.join("; "),
    ))
}

impl LucidosEngine {
    /// The calling thread's live subscriptions, newest first.
    ///
    /// Read from the dispatcher's live cache rather than from the event store,
    /// and that is the whole point: the cache IS the set of unresolved
    /// subscriptions, rebuilt from the store at boot, so it cannot disagree
    /// with what will actually wake the thread. Diffing `EventWaitStarted`
    /// against the three resolutions by hand is what the agent was reduced to,
    /// and it got the answer wrong.
    pub(crate) async fn list_event_waits_for_thread(&self, thread_id: Uuid) -> Vec<EventWaitView> {
        let now = Utc::now();
        let mut waits = self.live_waits.for_thread(thread_id).await;
        waits.sort_by(|a, b| b.armed_at.cmp(&a.armed_at));
        waits.iter().map(|w| EventWaitView::of(w, now)).collect()
    }

    /// The `list_event_waits` tool result.
    pub(crate) async fn list_event_waits_text(&self, thread_id: Uuid) -> String {
        render_event_wait_list(&self.list_event_waits_for_thread(thread_id).await)
    }

    /// Stand down one of this thread's subscriptions, or all of them.
    pub(crate) async fn cancel_event_waits_for_agent(
        &self,
        thread_id: Uuid,
        wait_id: Option<Uuid>,
        all: bool,
    ) -> CancelEventWaitOutcome {
        match resolve_cancel_target(wait_id, all) {
            Err(msg) => CancelEventWaitOutcome::Refused(msg),
            Ok(CancelTarget::One(id)) => self.cancel_one_for_agent(thread_id, id).await,
            Ok(CancelTarget::All) => self.cancel_all_for_agent(thread_id).await,
        }
    }

    async fn cancel_one_for_agent(&self, thread_id: Uuid, wait_id: Uuid) -> CancelEventWaitOutcome {
        // Named before it is taken, so the result can say WHAT it stopped
        // rather than only that something was.
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
                "Stopped watching for {}. It will not wake this thread.",
                named.unwrap_or_else(|| wait_id.to_string()),
            )),
            // A `wait_id` from another thread lands here too, and deliberately
            // reads the same: this thread has no such subscription, which is
            // the only true thing either case supports.
            super::CancelWaitOutcome::NotLive => CancelEventWaitOutcome::Refused(format!(
                "Error: no live subscription {wait_id} on this thread. It may already have \
                 fired, timed out, or been stopped. Call list_event_waits to see what is \
                 actually live."
            )),
            super::CancelWaitOutcome::EmitFailed => CancelEventWaitOutcome::Refused(format!(
                "Error: could not record the stop for {wait_id}. The subscription is still \
                 live and will still wake this thread, so tell the user it is still \
                 running rather than that you stood it down."
            )),
        }
    }

    async fn cancel_all_for_agent(&self, thread_id: Uuid) -> CancelEventWaitOutcome {
        let live = self.live_waits.for_thread(thread_id).await;
        if live.is_empty() {
            return CancelEventWaitOutcome::Refused(
                "Error: this thread has no live subscriptions, so there was nothing to \
                 stop. Nothing was going to wake it."
                    .to_string(),
            );
        }
        let named: Vec<String> = live.iter().map(|w| describe_subscriptions(&w.on)).collect();
        self.cancel_event_waits_for_thread(thread_id, EventWaitCancelCause::AgentStandDown, None)
            .await;
        // What is STILL live decides the answer, not how many emits reported
        // success: the cache is the thing that will or will not wake this
        // thread, and a cancel whose emit failed is put straight back into it.
        let still_live: Vec<String> = self
            .live_waits
            .for_thread(thread_id)
            .await
            .iter()
            .map(|w| describe_subscriptions(&w.on))
            .collect();
        all_stop_outcome(&named, &still_live)
    }
}

#[cfg(test)]
#[path = "agent_surface_tests.rs"]
mod tests;
