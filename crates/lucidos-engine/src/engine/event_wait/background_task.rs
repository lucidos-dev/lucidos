//! The wait the ENGINE arms, for a chat turn that ends with background work
//! still running.
//!
//! # Why this exists at all
//!
//! A background task's completion reaches a **coding-agent** thread by itself:
//! `spawn_bash_completion_watcher` (`engine/tools/bash.rs`) emits
//! `BackgroundBashCompleted` and then pushes a synthetic prompt onto the parked
//! session's `msg_tx`, so the agent resumes and reads the result. That path has
//! one branch it deliberately cannot take, stated in its own comment: it skips
//! the wake when there is "a chat-mode background bash with no CC session at
//! all". A chat thread has no parked subprocess to push to.
//!
//! So a chat thread's only wake was an `await_event` the model had to remember
//! to arm, and nothing pointed at that pairing. On 2026-08-09 a release thread
//! spawned Phase A with `run_bash_background`, drained it a few times, and
//! ended its turn with a status message. Phase A finished five minutes later
//! and emitted `BackgroundBashCompleted` with nobody subscribed. The thread sat
//! idle for five hours until the user asked whether the release had happened.
//!
//! # Why a subscription rather than a second wake path
//!
//! Mirroring the coding-agent `msg_tx` push would be a second delivery
//! mechanism with its own one-shot, timeout and loop semantics to get right,
//! and it would be invisible: the user would see an idle thread that happens to
//! wake up later. An *event wait* is already the engine's answer to "re-open
//! this thread when X happens". Arming one reuses its one-shot gate, its
//! explicit deadline, its caps, the subscription indicator the user can see,
//! and the boot rebuild that survives a restart. Nothing here is new machinery;
//! it is the existing machinery, armed by the engine instead of by the model.
//!
//! # The four things this gets right, none of them obvious
//!
//! * **One wait, every uncovered task.** The `on:` list carries one entry per
//!   task rather than one wait per task, so a turn that spawned three builds
//!   spends one live-wait slot and one consecutive-subscription count, not
//!   three. Any entry matching wakes the thread, which is what is wanted: the
//!   turn that wakes re-runs this tail and re-arms for whatever is still going.
//! * **Conditioned on `task_id`, always.** A wait subscribes across threads (it
//!   is how a thread watches another thread's work), so an unconditioned
//!   `BackgroundBashCompleted` would wake this thread on any background task
//!   finishing anywhere in the workspace.
//! * **Coverage, not equality.** The agent's own duplicate refusal compares
//!   `on:` lists exactly. Here that is the wrong test: a model that armed
//!   `BackgroundBashCompleted{task_id: X}` is already watching X, and arming a
//!   second wait for it would wake the thread twice for one completion. So
//!   coverage is decided with [`EventSubscription::matches`] against the
//!   payload the event will actually carry, which is the same predicate the
//!   dispatcher will use.
//! * **The deadline comes from the task.** A wait that expires before its task
//!   can be killed is a subscription that guarantees nothing, so the expiry is
//!   the latest watchdog deadline among the covered tasks plus a margin,
//!   clamped to the ordinary ceiling.

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::register::MAX_LIVE_WAITS_PER_THREAD;
use crate::core::event_subscription::EventSubscription;
use crate::engine::tools::bash_background::RunningTaskHandle;
use crate::engine::LucidosEngine;

/// The event a background task's completion lands as.
const BACKGROUND_BASH_COMPLETED: &str = "BackgroundBashCompleted";

/// Added to the latest watchdog deadline when sizing the wait.
///
/// The watchdog kills the child at its deadline, and the completion event is
/// emitted after that, so a wait expiring exactly at the deadline could lose
/// the race with the work it exists to observe. Generous on purpose: waking
/// late costs the whole wait, waking early costs one turn.
const DEADLINE_MARGIN: Duration = Duration::minutes(5);

/// Prefix on the synthetic `tool_use_id`, so an engine-armed wait is
/// distinguishable from one a model armed and can never collide with a
/// provider-issued id (those are opaque but never carry a colon-delimited
/// namespace of ours).
const ENGINE_TOOL_USE_PREFIX: &str = "engine:bg-task-wait";

/// What the user reads in the subscription indicator. Engine-authored, so it
/// says who armed it: a subscription the user did not see the agent ask for is
/// otherwise indistinguishable from one it did.
const ARMED_REASON: &str = "Watching background work started in this thread, so the thread \
                            re-opens when it finishes";

impl LucidosEngine {
    /// Arm a wait covering every unfinished background task this thread owns
    /// that nothing is already watching. Called from the chat turn tail.
    ///
    /// Returns the number of tasks now covered by a wait armed here, which is
    /// zero in every ordinary case: no background work, or the model armed its
    /// own subscription, or a cap says no.
    ///
    /// **Every refusal is silent to the user.** There is no tool call to answer
    /// and no turn left to report into, so a cap or a database error is logged
    /// and the tail moves on. That is a real regression back to the stall this
    /// prevents, which is why the log line says so explicitly rather than
    /// noting a skip.
    pub(crate) async fn arm_wait_for_running_background_tasks(&self, thread_id: Uuid) -> usize {
        // Cheapest possible early-out first: an in-memory map scan behind a
        // mutex. The overwhelming majority of turns own no background work at
        // all, and everything below this line costs at least one database
        // round trip. `MAX(sequence)` in particular is a parallel sequential
        // scan (no standalone index on the column), measured at ~300 ms over
        // 2.8M events, and paying that on every chat turn to answer a question
        // the registry answers for free would be a worse regression than the
        // stall this fixes.
        if !self.bash_background.has_running_for_thread(thread_id).await {
            return 0;
        }

        // Now the watermark, and BEFORE the authoritative registry read below.
        // The catch-up scan in `commit_wait` is `sequence > watermark`, so a
        // completion landing at or below it can never wake this wait: reading
        // the watermark afterwards would arm for a task and then miss the very
        // event it was armed for, and the thread would sit until timeout. The
        // gap is not hypothetical, it spans the subscription-count query below,
        // which is a database round trip.
        //
        // The early-out above does not reopen that gap. It is only a "should we
        // bother" probe; every task the `on` list is actually built from comes
        // from the `running_for_thread` read that follows the watermark, so a
        // task that finished in between is simply absent from the list rather
        // than armed-for-and-missed.
        let watermark = match self.read_watermark(thread_id).await {
            Ok(w) => w,
            Err(e) => {
                crate::log!(
                    "[EventWait] Could not read the watermark for thread {thread_id}: {e}. \
                     Any background work it owns will finish unwatched."
                );
                return 0;
            }
        };

        let running = self.bash_background.running_for_thread(thread_id).await;
        if running.is_empty() {
            return 0;
        }

        let live = self.live_waits.for_thread(thread_id).await;
        // Engine-armed waits COUNT toward the consecutive cap, deliberately. A
        // turn woken by one of these can spawn another background task and end
        // again, and without the count that loop has no bound at all. Counting
        // stops it at the same ten the model gets, after which the thread goes
        // quiet, which is exactly today's behaviour and so no regression.
        //
        // A cap that cannot be EVALUATED must not silently become no cap, the
        // same call `event_wait_caps_refusal` makes, so an unreadable count is
        // passed on as such rather than as a zero.
        let consecutive = super::register::consecutive_subscriptions(&self.pool, thread_id)
            .await
            .inspect_err(|e| {
                crate::log!(
                    "[EventWait] Subscription-count read failed for thread {thread_id}: {e}."
                );
            })
            .ok();

        let plan = plan_wait(&running, &live, consecutive, thread_id);
        let uncovered = match &plan {
            ArmingPlan::Arm(tasks) => tasks,
            ArmingPlan::NothingUncovered => return 0,
            // Every refusal is a real regression back to the stall this
            // prevents, so it says so rather than reading as a routine skip.
            ArmingPlan::Refused(why) => {
                crate::log!(
                    "[EventWait] Thread {thread_id} ended a turn with unwatched background \
                     work and nothing will re-open it when that finishes: {why}"
                );
                return 0;
            }
        };

        let on: Vec<EventSubscription> = uncovered
            .iter()
            .map(|h| EventSubscription {
                event_type: BACKGROUND_BASH_COMPLETED.to_string(),
                condition: Some(serde_json::json!({ "task_id": h.task_id })),
            })
            .collect();
        let timeout_secs = timeout_for(uncovered, Utc::now());

        let tool_use_id = format!("{ENGINE_TOOL_USE_PREFIX}:{}", Uuid::new_v4());
        // The watermark read at the top, NOT a fresh one: every task in
        // `uncovered` was unfinished after that read, so each completion is
        // guaranteed to land above it and be reachable by the catch-up scan.
        let wait = self.build_wait_at(
            thread_id,
            &tool_use_id,
            on,
            ARMED_REASON,
            timeout_secs,
            watermark,
        );
        // No arming lookback. It exists so a MODEL that checked state before
        // calling `await_event` hears about a match that landed in between, and
        // it works by reporting that match back into the same turn. There is no
        // turn here to report into. The equivalent gap is closed structurally
        // instead, by reading the watermark before the registry.
        if let Err(e) = self.commit_wait(&wait).await {
            crate::log!(
                "[EventWait] Could not arm a background-task wait for thread {thread_id}: {e}. \
                 Its background work will finish unwatched."
            );
            return 0;
        }
        crate::log!(
            "[EventWait] Armed wait {} for thread {thread_id} over {} unwatched background \
             task(s), expiring in {timeout_secs}s",
            wait.wait_id,
            uncovered.len(),
        );
        uncovered.len()
    }
}

/// What the turn tail should do about this thread's background work.
///
/// The whole decision, as a value, so the caps and the coverage filter are
/// testable without an engine and a database behind them. The engine method is
/// then three reads, this call, and the arming.
#[derive(Debug, PartialEq)]
pub(super) enum ArmingPlan<'a> {
    /// Arm one wait over these tasks. Never empty.
    Arm(Vec<&'a RunningTaskHandle>),
    /// Every running task is already watched, so arming again would wake the
    /// thread twice for one completion. Not a refusal: the thread IS covered.
    NothingUncovered,
    /// A cap says no, and the thread will therefore go quiet with work still
    /// running. Carries the reason for the log, because this is the failure
    /// the whole mechanism exists to prevent, reappearing at a bound.
    Refused(String),
}

/// Decide whether to arm, and over which tasks.
///
/// `consecutive` is `None` when the count could not be read. That is a refusal,
/// not a zero: an unreadable event store is exactly when a runaway loop would
/// do the most damage, which is the same call `event_wait_caps_refusal` makes.
pub(super) fn plan_wait<'a>(
    running: &'a [RunningTaskHandle],
    live: &[super::LiveWait],
    consecutive: Option<i64>,
    thread_id: Uuid,
) -> ArmingPlan<'a> {
    let uncovered: Vec<&RunningTaskHandle> = running
        .iter()
        .filter(|h| {
            !live
                .iter()
                .any(|w| wait_covers_task(&w.on, &h.task_id, thread_id))
        })
        .collect();
    if uncovered.is_empty() {
        return ArmingPlan::NothingUncovered;
    }

    // The live-wait cap. Only this one of `event_wait_caps_refusal`'s three
    // arms applies: the duplicate arm is replaced by the coverage filter above,
    // and the consecutive cap is the next check.
    if live.len() >= MAX_LIVE_WAITS_PER_THREAD {
        return ArmingPlan::Refused(format!(
            "it already holds {} live subscriptions, the limit, with {} task(s) unwatched",
            live.len(),
            uncovered.len(),
        ));
    }

    match consecutive {
        Some(n) if n >= super::MAX_CONSECUTIVE_SUBSCRIPTIONS => ArmingPlan::Refused(format!(
            "it has subscribed {n} times with no message from the user, the limit, with {} \
             task(s) unwatched",
            uncovered.len(),
        )),
        Some(_) => ArmingPlan::Arm(uncovered),
        None => ArmingPlan::Refused(
            "the subscription count could not be read, and an uncheckable cap must not \
             become no cap"
                .to_string(),
        ),
    }
}

/// Whether a live wait would already wake on this task's completion.
///
/// Runs the dispatcher's own predicate against the *matchable payload* the
/// event will carry, the thread id included, so "covered" means the wait
/// genuinely fires rather than merely looking similar. An unconditioned `BackgroundBashCompleted` entry therefore
/// counts as covering every task, which is correct: it will wake on the first
/// of them.
fn wait_covers_task(on: &[EventSubscription], task_id: &str, thread_id: Uuid) -> bool {
    let payload = crate::core::event_subscription::matchable_payload(
        serde_json::json!({ "task_id": task_id }),
        Some(thread_id),
    );
    EventSubscription::any_matches(on, BACKGROUND_BASH_COMPLETED, &payload)
}

/// Seconds until the wait should expire: past the last watchdog deadline among
/// the covered tasks, plus [`DEADLINE_MARGIN`], clamped to the ordinary ceiling
/// and floored at one second.
///
/// A deadline already in the past yields the floor rather than a negative
/// number. That happens when the watchdog is late (a child ignoring SIGTERM,
/// a saturated host), and the right answer is a wait that gives up almost
/// immediately rather than one that never expires.
fn timeout_for(tasks: &[&RunningTaskHandle], now: DateTime<Utc>) -> i64 {
    let latest = tasks
        .iter()
        .map(|h| h.watchdog_deadline)
        .max()
        .unwrap_or(now);
    ((latest + DEADLINE_MARGIN) - now)
        .num_seconds()
        .clamp(1, super::register::MAX_TIMEOUT_SECS)
}

#[cfg(test)]
#[path = "background_task_tests.rs"]
mod tests;
