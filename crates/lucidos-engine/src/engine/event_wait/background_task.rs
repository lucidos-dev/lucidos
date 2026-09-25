//! The wait the ENGINE arms over a thread's running background tasks.
//!
//! # Why this exists at all
//!
//! `spawn_bash_completion_watcher` (`engine/tools/bash.rs`) emits
//! `BackgroundBashCompleted` and nothing more. A thread only hears about a
//! finished task if something subscribed to that event. Two callers arm here:
//!
//! * **The chat turn tail.** Before it, a chat thread's only way back was an
//!   `await_event` the model had to remember to arm. A release thread once
//!   ended its turn with Phase A still running, and sat idle for five hours
//!   after Phase A finished with nobody subscribed.
//! * **The coding-agent `background-tasks` route**, right after it spawns. A
//!   coding agent ends its turn to wait, and its process group goes with it.
//!   The wait is the only thing left to wake it.
//!
//! # Why a subscription rather than a direct push
//!
//! A push into a parked session would be a second delivery
//! mechanism with its own one-shot, timeout and loop semantics to get right,
//! and it would be invisible: the user would see an idle thread that happens to
//! come back to life later. An *event wait* is already the engine's answer to "re-open
//! this thread when X happens". Arming one reuses its one-shot gate, its
//! explicit deadline, its caps, the waiting indicator the user can see,
//! and the boot rebuild that survives a restart. Nothing here is new machinery;
//! it is the existing machinery, armed by the engine instead of by the model.
//!
//! # The four things this gets right, none of them obvious
//!
//! * **One wait, every uncovered task.** The `on:` list carries one entry per
//!   task rather than one wait per task, so a turn that spawned three builds
//!   spends one live-wait slot and one recent-subscription count, not
//!   three. Any entry matching re-opens the thread, which is what is wanted: the
//!   re-entered turn re-runs this tail and re-arms for whatever is still going.
//! * **Conditioned on `task_id`, always.** A wait subscribes across threads (it
//!   is how a thread watches another thread's work), so an unconditioned
//!   `BackgroundBashCompleted` would re-open this thread on any background task
//!   finishing anywhere in the workspace.
//! * **Coverage, not equality.** The agent's own duplicate refusal compares
//!   `on:` lists exactly. Here that is the wrong test: a model that armed
//!   `BackgroundBashCompleted{task_id: X}` is already watching X, and arming a
//!   second wait for it would deliver one completion to the thread twice. So
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
/// the race with the work it exists to observe. Generous on purpose: expiring
/// late costs one turn, expiring early costs the whole wait.
const DEADLINE_MARGIN: Duration = Duration::minutes(5);

/// Prefix on the synthetic `tool_use_id`, so an engine-armed wait is
/// distinguishable from one a model armed and can never collide with a
/// provider-issued id (those are opaque but never carry a colon-delimited
/// namespace of ours).
const ENGINE_TOOL_USE_PREFIX: &str = "engine:bg-task-wait";

/// The longest one task's name may run in the wait's reason, in characters.
/// The reason is one line in the transcript, and a command can be kilobytes.
const LABEL_MAX_CHARS: usize = 80;

/// How many tasks the reason names before it counts the rest.
const LABELS_SHOWN: usize = 3;

impl LucidosEngine {
    /// Arm a wait covering every unfinished background task this thread owns
    /// that nothing is already watching. Called from the chat turn tail, and
    /// by the coding-agent `background-tasks` route right after it spawns.
    ///
    /// Returns the ids of the running tasks a live wait now covers, whether
    /// armed here or already there. A running task missing from the list will
    /// finish unwatched, because a cap or an error said no. A task that had
    /// already finished is missing too: it was never running to be covered.
    ///
    /// **Every refusal is silent to the user.** There is no tool call to answer
    /// and no turn left to report into, so a cap or a database error is logged
    /// and the tail moves on. That is a real regression back to the stall this
    /// prevents, which is why the log line says so explicitly rather than
    /// noting a skip.
    pub(crate) async fn arm_wait_for_running_background_tasks(
        &self,
        thread_id: Uuid,
    ) -> Vec<String> {
        // Cheapest possible early-out first: an in-memory map scan behind a
        // mutex. The overwhelming majority of turns own no background work at
        // all, and everything below this line costs at least one database
        // round trip. `MAX(sequence)` in particular is a parallel sequential
        // scan (no standalone index on the column), measured at ~300 ms over
        // 2.8M events, and paying that on every chat turn to answer a question
        // the registry answers for free would be a worse regression than the
        // stall this fixes.
        if !self.bash_background.has_running_for_thread(thread_id).await {
            return Vec::new();
        }

        // Now the watermark, and BEFORE the authoritative registry read below.
        // The catch-up scan in `commit_wait` is `sequence > watermark`, so a
        // completion landing at or below it can never match this wait: reading
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
                return Vec::new();
            }
        };

        let running = self.bash_background.running_for_thread(thread_id).await;
        if running.is_empty() {
            return Vec::new();
        }

        let live = self.live_waits.for_thread(thread_id).await;
        let already_covered: Vec<String> = running
            .iter()
            .filter(|h| {
                live.iter()
                    .any(|w| wait_covers_task(&w.on, &h.task_id, thread_id))
            })
            .map(|h| h.task_id.clone())
            .collect();
        // Engine-armed waits COUNT toward the recent-subscription cap,
        // deliberately. A turn re-entered by one of these can spawn another
        // background task and end again. When those tasks exit at once, that is
        // a hot loop, and the count stops it at the same rate the model gets.
        //
        // A cap that cannot be EVALUATED must not silently become no cap, the
        // same call `event_wait_caps_refusal` makes, so an unreadable count is
        // passed on as such rather than as a zero.
        let recent = super::register::recent_subscriptions(&self.pool, thread_id)
            .await
            .inspect_err(|e| {
                crate::log!(
                    "[EventWait] Subscription-count read failed for thread {thread_id}: {e}."
                );
            })
            .ok();

        let plan = plan_wait(&running, &live, recent, thread_id);
        let uncovered = match &plan {
            ArmingPlan::Arm(tasks) => tasks,
            ArmingPlan::NothingUncovered => return already_covered,
            // Every refusal is a real regression back to the stall this
            // prevents, so it says so rather than reading as a routine skip.
            ArmingPlan::Refused(why) => {
                crate::log!(
                    "[EventWait] Thread {thread_id} has unwatched background work and \
                     nothing will re-open it when that finishes: {why}"
                );
                return already_covered;
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
            &armed_reason(uncovered),
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
            return already_covered;
        }
        crate::log!(
            "[EventWait] Armed wait {} for thread {thread_id} over {} unwatched background \
             task(s), expiring in {timeout_secs}s",
            wait.wait_id,
            uncovered.len(),
        );
        already_covered
            .into_iter()
            .chain(uncovered.iter().map(|h| h.task_id.clone()))
            .collect()
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
    /// Every running task is already watched, so arming again would deliver one
    /// completion to the thread twice. Not a refusal: the thread IS covered.
    NothingUncovered,
    /// A cap says no, and the thread will therefore go quiet with work still
    /// running. Carries the reason for the log, because this is the failure
    /// the whole mechanism exists to prevent, reappearing at a bound.
    Refused(String),
}

/// Decide whether to arm, and over which tasks.
///
/// `recent` is `None` when the count could not be read. That is a refusal,
/// not a zero: an unreadable event store is exactly when a runaway loop would
/// do the most damage, which is the same call `event_wait_caps_refusal` makes.
pub(super) fn plan_wait<'a>(
    running: &'a [RunningTaskHandle],
    live: &[super::LiveWait],
    recent: Option<i64>,
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
    // and the recent-subscription cap is the next check.
    if live.len() >= MAX_LIVE_WAITS_PER_THREAD {
        return ArmingPlan::Refused(format!(
            "it already holds {} live subscriptions, the limit, with {} task(s) unwatched",
            live.len(),
            uncovered.len(),
        ));
    }

    match recent {
        Some(n) if n >= super::MAX_RECENT_SUBSCRIPTIONS => ArmingPlan::Refused(format!(
            "it has started {n} waits in the last {} minutes that no other thread ended, \
             with no message or answer from the user, the limit, with {} task(s) unwatched",
            super::RECENT_SUBSCRIPTION_WINDOW_SECS / 60,
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

/// Whether a live wait would already fire on this task's completion.
///
/// Runs the dispatcher's own predicate against the *matchable payload* the
/// event will carry, the thread id included, so "covered" means the wait
/// genuinely fires rather than merely looking similar. An unconditioned `BackgroundBashCompleted` entry therefore
/// counts as covering every task, which is correct: it will fire on the first
/// of them.
fn wait_covers_task(on: &[EventSubscription], task_id: &str, thread_id: Uuid) -> bool {
    let payload = crate::core::event_subscription::matchable_payload(
        BACKGROUND_BASH_COMPLETED,
        serde_json::json!({ "task_id": task_id }),
        Some(thread_id),
    );
    EventSubscription::any_matches(on, BACKGROUND_BASH_COMPLETED, &payload)
}

/// What the user reads on the wait's transcript row, after `Waiting for `. So
/// it is a noun phrase naming the work, never a sentence about waiting.
///
/// The wait fires on the FIRST covered task to finish, so several tasks read as
/// "the first of", which is what actually re-opens the thread.
fn armed_reason(tasks: &[&RunningTaskHandle]) -> String {
    match tasks {
        [one] => format!("{} to finish", task_label(one)),
        many => {
            let mut names: Vec<String> = many
                .iter()
                .take(LABELS_SHOWN)
                .map(|h| task_label(h))
                .collect();
            if many.len() > LABELS_SHOWN {
                names.push(format!("{} more", many.len() - LABELS_SHOWN));
            }
            format!(
                "the first of {} background jobs to finish: {}",
                many.len(),
                names.join("; ")
            )
        }
    }
}

/// A task by the agent's own name for it, or by its command when it gave none.
fn task_label(task: &RunningTaskHandle) -> String {
    match &task.description {
        Some(description) => one_short_line(description),
        None => format!(
            "the background command \"{}\"",
            one_short_line(&task.command)
        ),
    }
}

/// Collapse every run of whitespace, newlines included, to one space, and cut
/// at [`LABEL_MAX_CHARS`] with an ellipsis.
fn one_short_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= LABEL_MAX_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(LABEL_MAX_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
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
