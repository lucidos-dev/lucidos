//! The wait the ENGINE arms over a thread's running background tasks.
//!
//! # Why this exists at all
//!
//! `spawn_bash_completion_watcher` (`engine/tools/bash.rs`) emits
//! `BackgroundBashCompleted` and nothing more. A thread only hears about a
//! finished task if something subscribed to that event. Three callers arm here:
//!
//! * **The chat turn tail.** Before it, a chat thread's only way back was an
//!   `await_event` the model had to remember to arm. A release thread once
//!   ended its turn with Phase A still running, and sat idle for five hours
//!   after Phase A finished with nobody subscribed.
//! * **The coding-agent `background-tasks` route**, right after it spawns. A
//!   coding agent ends its turn to wait, and every process it started goes
//!   with it.
//!   The wait is the only thing left to wake it.
//! * **The coding-agent turn tail**, at a clean turn end. It covers the
//!   polled tasks the route left to the agent, since an idle agent polls
//!   nothing (ADR 0619, see [`OwnerTurn`]). It also delivers a polled result
//!   that finished during the turn and that the agent never read.
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
//!
//! # The way back down: a thread's own stop is not news
//!
//! A thread that stops a task already knows it stopped. So the stop first
//! stands down the issuing thread's waits on that task, and only then signals
//! it. The killed completion then matches nothing there (ADR 0369). The
//! task's owner still hears about a stop someone else issued. Discard,
//! Archive and a timeout deliver as before.

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::register::MAX_LIVE_WAITS_PER_THREAD;
use super::{LiveWait, LiveWaits};
use crate::core::event_subscription::EventSubscription;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::EventWaitCancelCause;
use crate::engine::tools::bash_background::{
    BackgroundBashRegistry, RunningTaskHandle, UnreadResult,
};
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

/// Where the owning thread's turn stands when the arming runs.
///
/// A polled task is its owner's job only while the owner's turn runs: the
/// agent waits on it in-turn and reads the result itself. Once the turn ends,
/// nobody polls, so the engine covers it like any other task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerTurn {
    /// Mid-turn, as at the coding-agent `background-tasks` route. Polled tasks
    /// stay uncovered, or one completion would reach the agent twice.
    Running,
    /// At a clean turn end, in the chat or the coding-agent turn tail. Every
    /// running task is covered, polled or not, and so is every polled result
    /// that finished unread.
    Ended,
}

impl LucidosEngine {
    /// Arm a wait covering every unfinished background task this thread owns
    /// that nothing is already watching. See [`arm_wait_for_running_tasks`].
    ///
    /// **Every refusal is silent to the user.** There is no tool call to answer
    /// and no turn left to report into, so a cap or a database error is logged
    /// and the tail moves on. That is a real regression back to the stall this
    /// prevents, which is why the log line says so explicitly rather than
    /// noting a skip.
    pub(crate) async fn arm_wait_for_running_background_tasks(
        &self,
        thread_id: Uuid,
        turn: OwnerTurn,
    ) -> Vec<String> {
        let armed = arm_wait_for_running_tasks(
            &self.bash_background,
            &self.event_bus,
            &self.live_waits,
            &self.pool,
            thread_id,
            turn,
        )
        .await;
        if let Some(wait) = &armed.wait {
            self.catch_up_event_wait(wait).await;
        }
        armed.covered
    }

    /// A wait of `thread_id` is delivering this event. When it is a background
    /// task's completion, the thread now holds that result, so no later wait
    /// delivers it again. Runs before the delivery is emitted: a turn ending in
    /// between must not find the result still unread.
    pub(super) fn note_delivered_completion(
        &self,
        thread_id: Uuid,
        event_type: &str,
        payload: &serde_json::Value,
    ) {
        if event_type != BACKGROUND_BASH_COMPLETED {
            return;
        }
        if let Some(task_id) = payload.get("task_id").and_then(|v| v.as_str()) {
            self.bash_background.mark_result_read(thread_id, task_id);
        }
    }
}

/// What one arming pass left behind.
#[derive(Debug, Default)]
pub(super) struct Armed {
    /// The running tasks a live wait now covers, whether armed here or already
    /// there. A running task missing from the list will finish unwatched,
    /// because a cap or an error said no. A task that had already finished is
    /// missing too: it was never running to be covered.
    pub(super) covered: Vec<String>,
    /// The wait armed here, which still owes its catch-up scan. That scan
    /// needs the engine, because a hit re-enters the thread.
    pub(super) wait: Option<LiveWait>,
}

/// Arm one wait over the thread's uncovered running tasks: three reads,
/// [`plan_wait`], and the persist. A free function over the stores, as
/// [`stop_and_stand_down`] is, so it is testable without an engine.
pub(super) async fn arm_wait_for_running_tasks(
    registry: &BackgroundBashRegistry,
    bus: &EventBus,
    live_waits: &LiveWaits,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    turn: OwnerTurn,
) -> Armed {
    // Cheapest possible early-out first: an in-memory map scan behind a
    // mutex. The overwhelming majority of turns own no background work at
    // all, and everything below this line costs at least one database
    // round trip. The watermark read runs `MAX(sequence)`, a parallel
    // sequential scan (no standalone index on the column), measured at
    // ~300 ms over 2.8M events. Paying that on every chat turn, for an
    // answer the registry gives for free, would cost more than the stall.
    //
    // Only a clean turn end delivers a polled result that finished unread.
    // Mid-turn, the owner still reads it itself (ADR 0619).
    //
    // Unread AFTER running, every time. A finish writes both under the task
    // lock, so a task that left the running read is already in the unread one.
    let unread_results = || match turn {
        OwnerTurn::Ended => registry.unread_results_for_thread(thread_id),
        OwnerTurn::Running => Vec::new(),
    };
    if !registry.has_running_for_thread(thread_id).await && unread_results().is_empty() {
        return Armed::default();
    }

    // Now the watermark, and BEFORE the authoritative registry read below.
    // The catch-up scan is `sequence > watermark`, so a completion landing
    // at or below it can never match this wait. Read afterwards, the wait
    // would arm for a task and miss the very event it was armed for. The
    // thread would then sit until timeout. The gap is real: it spans the
    // subscription-count query below, a database round trip.
    //
    // The early-out above does not reopen that gap. It is only a "should we
    // bother" probe. The `on` list comes from the `running_for_thread` read
    // that follows the watermark. So a task that finished in between is
    // absent from the list, not armed for and missed.
    let watermark = match super::register::read_watermark(pool, thread_id).await {
        Ok(w) => w,
        Err(e) => {
            crate::log!(
                "[EventWait] Could not read the watermark for thread {thread_id}: {e}. \
                 Any background work it owns will finish unwatched."
            );
            return Armed::default();
        }
    };

    let running = registry.running_for_thread(thread_id).await;
    let unread = unread_results();
    if running.is_empty() && unread.is_empty() {
        return Armed::default();
    }

    let live = live_waits.for_thread(thread_id).await;
    let already_covered: Vec<String> = running
        .iter()
        .filter(|h| {
            live.iter()
                .any(|w| wait_covers_task(&w.on, &h.task_id, Some(thread_id)))
        })
        .map(|h| h.task_id.clone())
        .collect();
    let unarmed = Armed {
        covered: already_covered.clone(),
        wait: None,
    };
    // Engine-armed waits COUNT toward the recent-subscription cap,
    // deliberately. A turn re-entered by one of these can spawn another
    // background task and end again. When those tasks exit at once, that is
    // a hot loop, and the count stops it at the same rate the model gets.
    //
    // A cap that cannot be EVALUATED must not silently become no cap. So an
    // unreadable count is passed on as such rather than as a zero, the same
    // call `event_wait_caps_refusal` makes.
    let recent = super::register::recent_subscriptions(pool, thread_id)
        .await
        .inspect_err(|e| {
            crate::log!("[EventWait] Subscription-count read failed for thread {thread_id}: {e}.");
        })
        .ok();

    let plan = plan_wait(&running, &unread, &live, recent, thread_id, turn);
    let uncovered = match &plan {
        ArmingPlan::Arm(uncovered) => uncovered,
        ArmingPlan::NothingUncovered => return unarmed,
        // Every refusal is a real regression back to the stall this
        // prevents, so it says so rather than reading as a routine skip.
        ArmingPlan::Refused(why) => {
            crate::log!(
                "[EventWait] Thread {thread_id} has unwatched background work and \
                 nothing will re-open it when that finishes: {why}"
            );
            return unarmed;
        }
    };

    let task_ids = uncovered.task_ids();
    let on: Vec<EventSubscription> = task_ids
        .iter()
        .map(|task_id| EventSubscription {
            event_type: BACKGROUND_BASH_COMPLETED.to_string(),
            condition: Some(serde_json::json!({ "task_id": task_id })),
        })
        .collect();
    let timeout_secs = timeout_for(&uncovered.running, Utc::now());

    // The watermark read at the top, NOT a fresh one: every running task in
    // `uncovered` was unfinished after that read. Its completion is emitted
    // later, and a committed horizon is below any append that starts after
    // it, so the catch-up scan can reach the completion.
    let unread_ids: Vec<&str> = uncovered
        .unread
        .iter()
        .map(|r| r.task_id.as_str())
        .collect();
    let watermark = match below_completions(pool, thread_id, &unread_ids, watermark).await {
        Ok(w) => w,
        Err(e) => {
            crate::log!(
                "[EventWait] Could not find the unread completions of thread {thread_id}: {e}. \
                 Its background work will finish unwatched."
            );
            return unarmed;
        }
    };
    let wait = super::register::build_wait_at(
        thread_id,
        &engine_tool_use_id(),
        on,
        &armed_reason(&uncovered.labels()),
        timeout_secs,
        watermark,
    );
    // No arming lookback. It lets a MODEL that checked state before calling
    // `await_event` hear about a match that landed in between. It works by
    // reporting that match back into the same turn. There is no
    // turn here to report into. The equivalent gap is closed structurally
    // instead, by reading the watermark before the registry.
    if let Err(e) = super::register::persist_wait(bus, live_waits, &wait).await {
        crate::log!(
            "[EventWait] Could not arm a background-task wait for thread {thread_id}: {e}. \
             Its background work will finish unwatched."
        );
        return unarmed;
    }
    crate::log!(
        "[EventWait] Armed wait {} for thread {thread_id} over {} unwatched background \
         task(s), expiring in {timeout_secs}s",
        wait.wait_id,
        task_ids.len(),
    );
    Armed {
        covered: already_covered
            .into_iter()
            .chain(uncovered.running.iter().map(|h| h.task_id.clone()))
            .collect(),
        wait: Some(wait),
    }
}

/// `watermark`, lowered just below the earliest committed completion of these
/// finished tasks, so the catch-up scan finds it at once. A completion not yet
/// committed needs no lowering: it lands above any committed horizon.
async fn below_completions(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    task_ids: &[&str],
    watermark: i64,
) -> Result<i64, sqlx::Error> {
    if task_ids.is_empty() {
        return Ok(watermark);
    }
    let earliest: Option<i64> = sqlx::query_scalar(
        "SELECT MIN(sequence) FROM events \
         WHERE event_type = $1 AND aggregate_id = $2 AND payload->>'task_id' = ANY($3)",
    )
    .bind(BACKGROUND_BASH_COMPLETED)
    .bind(thread_id.to_string())
    .bind(task_ids)
    .fetch_one(pool)
    .await?;
    Ok(earliest.map_or(watermark, |seq| watermark.min(seq - 1)))
}

/// What the turn tail should do about this thread's background work.
///
/// The whole decision, as a value, so the caps and the coverage filter are
/// testable without an engine and a database behind them. The engine method is
/// then three reads, this call, and the arming.
#[derive(Debug, PartialEq)]
pub(super) enum ArmingPlan<'a> {
    /// Arm one wait over these tasks. Never empty.
    Arm(Uncovered<'a>),
    /// Every running task is already watched, or polled by its owner mid-turn,
    /// and no result is unread. Arming again would deliver one completion
    /// twice. Not a refusal: the thread IS covered.
    NothingUncovered,
    /// A cap says no, and the thread will therefore go quiet with work still
    /// running. Carries the reason for the log, because this is the failure
    /// the whole mechanism exists to prevent, reappearing at a bound.
    Refused(String),
}

/// The tasks one engine wait is about to cover.
#[derive(Debug, PartialEq)]
pub(super) struct Uncovered<'a> {
    /// Still running, and nothing watches them.
    pub(super) running: Vec<&'a RunningTaskHandle>,
    /// Finished polled tasks whose owner never read the result. Their
    /// completions are already in the store, below the usual watermark.
    pub(super) unread: Vec<&'a UnreadResult>,
}

impl Uncovered<'_> {
    fn task_ids(&self) -> Vec<&str> {
        self.running
            .iter()
            .map(|h| h.task_id.as_str())
            .chain(self.unread.iter().map(|r| r.task_id.as_str()))
            .collect()
    }

    fn labels(&self) -> Vec<String> {
        self.running
            .iter()
            .map(|h| task_label(h.description.as_deref(), &h.command))
            .chain(
                self.unread
                    .iter()
                    .map(|r| task_label(r.description.as_deref(), &r.command)),
            )
            .collect()
    }

    fn len(&self) -> usize {
        self.running.len() + self.unread.len()
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Decide whether to arm, and over which tasks.
///
/// `recent` is `None` when the count could not be read. That is a refusal,
/// not a zero: an unreadable event store is exactly when a runaway loop would
/// do the most damage, which is the same call `event_wait_caps_refusal` makes.
///
/// Only a live engine-armed wait covers an unread result. Its watermark sits
/// below the completion, so its delivery is already on the way. A model's
/// wait may have armed above it and never fire.
pub(super) fn plan_wait<'a>(
    running: &'a [RunningTaskHandle],
    unread: &'a [UnreadResult],
    live: &[super::LiveWait],
    recent: Option<i64>,
    thread_id: Uuid,
    turn: OwnerTurn,
) -> ArmingPlan<'a> {
    let uncovered = Uncovered {
        running: running
            .iter()
            .filter(|h| !h.stop_requested)
            .filter(|h| turn == OwnerTurn::Ended || !h.polled)
            .filter(|h| {
                !live
                    .iter()
                    .any(|w| wait_covers_task(&w.on, &h.task_id, Some(thread_id)))
            })
            .collect(),
        unread: unread
            .iter()
            .filter(|_| turn == OwnerTurn::Ended)
            .filter(|r| {
                !live.iter().any(|w| {
                    is_engine_armed(w) && wait_covers_task(&w.on, &r.task_id, Some(thread_id))
                })
            })
            .collect(),
    };
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
/// event will carry, the owning thread's id included, so "covered" means the
/// wait genuinely fires rather than merely looking similar. An unconditioned
/// `BackgroundBashCompleted` entry therefore covers every task, which is
/// correct: it will fire on the first of them.
fn wait_covers_task(on: &[EventSubscription], task_id: &str, owner: Option<Uuid>) -> bool {
    EventSubscription::any_matches(
        on,
        BACKGROUND_BASH_COMPLETED,
        &completion_payload(task_id, owner),
    )
}

/// The matchable view of a task's eventual `BackgroundBashCompleted`, as far
/// as the coverage questions here need it: its `task_id` and its owner.
fn completion_payload(task_id: &str, owner: Option<Uuid>) -> serde_json::Value {
    crate::core::event_subscription::matchable_payload(
        BACKGROUND_BASH_COMPLETED,
        serde_json::json!({ "task_id": task_id }),
        owner,
    )
}

/// The matchable view of the completion a stop produces: killed by us, so
/// neither timed out nor abandoned. A wait conditioned on those fields fires
/// on it, so the stand-down must see them too. Exit code and signal stay out,
/// because a trap decides them.
fn stopped_completion_payload(task_id: &str, owner: Option<Uuid>) -> serde_json::Value {
    crate::core::event_subscription::matchable_payload(
        BACKGROUND_BASH_COMPLETED,
        serde_json::json!({
            "task_id": task_id,
            "killed": true,
            "timed_out": false,
            "abandoned": false,
        }),
        owner,
    )
}

/// A fresh id for a wait the engine arms. See [`ENGINE_TOOL_USE_PREFIX`].
fn engine_tool_use_id() -> String {
    format!("{ENGINE_TOOL_USE_PREFIX}:{}", Uuid::new_v4())
}

/// Whether the engine armed this wait, rather than a model.
fn is_engine_armed(wait: &LiveWait) -> bool {
    wait.tool_use_id
        .strip_prefix(ENGINE_TOOL_USE_PREFIX)
        .is_some_and(|rest| rest.starts_with(':'))
}

/// What standing a wait down over one stopped task does to it.
#[derive(Debug, PartialEq)]
pub(super) enum StandDown {
    /// Every entry fires on the stopped task, so nothing is left to watch.
    End,
    /// A wait the model armed, which also watched something else. It ends
    /// whole and the stop names it: ADR 0059 never narrows a model's wait.
    EndWithOthers,
    /// A wait the engine armed over other tasks too. It is replaced by an
    /// engine wait over these entries, the rest of its own `on` list.
    Narrow(Vec<EventSubscription>),
}

/// Decide what a stop of `task_id` does to one wait on the stopping thread.
/// `None` when the wait never fires on that task.
///
/// The engine may narrow its OWN wait because it wrote the `on` list and the
/// reason, and it re-arms that wait every turn anyway.
pub(super) fn plan_stand_down(
    wait: &LiveWait,
    task_id: &str,
    owner: Option<Uuid>,
) -> Option<StandDown> {
    let payload = stopped_completion_payload(task_id, owner);
    let (watching, rest): (Vec<EventSubscription>, Vec<EventSubscription>) = wait
        .on
        .iter()
        .cloned()
        .partition(|entry| entry.matches(BACKGROUND_BASH_COMPLETED, &payload));
    if watching.is_empty() {
        None
    } else if rest.is_empty() {
        Some(StandDown::End)
    } else if is_engine_armed(wait) {
        Some(StandDown::Narrow(rest))
    } else {
        Some(StandDown::EndWithOthers)
    }
}

/// What stopping one background task did, for the caller to report.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StoppedTask {
    pub(crate) task_id: String,
    /// The task by its description, or its command, as the waiting row named it.
    pub(crate) label: String,
    /// Reasons of model-armed waits that watched more than this task and
    /// ended whole. The agent may want to re-arm the rest.
    pub(crate) ended_with_others: Vec<String>,
}

/// A stop and its stand-down, before the engine runs the replacements'
/// catch-up scans.
#[derive(Debug)]
pub(super) struct StopOutcome {
    pub(super) stopped: StoppedTask,
    /// Engine waits armed in place of ones that also covered other tasks.
    /// Each still owes its catch-up scan, which needs the engine.
    pub(super) replacements: Vec<LiveWait>,
}

/// Stop one background task on behalf of `issuer`, standing down the issuer's
/// waits on it first. `None` when the task is not running, or a stop is
/// already on its way.
///
/// The order is the whole point: take the stop, stand down, THEN signal. A
/// completion emitted before the stand-down would wake the issuer. A free
/// function over the three stores so it is testable without an engine.
pub(super) async fn stop_and_stand_down(
    registry: &BackgroundBashRegistry,
    bus: &EventBus,
    live_waits: &LiveWaits,
    issuer: Uuid,
    task_id: &str,
) -> Option<StopOutcome> {
    let stop = registry.begin_stop(task_id).await?;
    let owner = stop.owner();
    let running = match owner {
        Some(owner) => registry.running_for_thread(owner).await,
        None => Vec::new(),
    };
    let label = running
        .iter()
        .find(|h| h.task_id == task_id)
        .map(|h| task_label(h.description.as_deref(), &h.command))
        .unwrap_or_else(|| format!("background task {task_id}"));

    let mut outcome = StopOutcome {
        stopped: StoppedTask {
            task_id: task_id.to_string(),
            label,
            ended_with_others: Vec::new(),
        },
        replacements: Vec::new(),
    };
    for live in live_waits.for_thread(issuer).await {
        let Some(plan) = plan_stand_down(&live, task_id, owner) else {
            continue;
        };
        // Out of the cache first, so it cannot resolve on anything while the
        // replacement is written. A wait that resolved meanwhile is gone.
        let Some(wait) = live_waits.take(live.wait_id).await else {
            continue;
        };
        let mut replacement_id = None;
        match plan {
            StandDown::End => {}
            StandDown::EndWithOthers => outcome.stopped.ended_with_others.push(wait.reason.clone()),
            StandDown::Narrow(rest) => {
                let replacement = narrowed(&wait, rest, &running, owner);
                // The replacement commits BEFORE the cancel, so a waiting
                // child never reads as finished in between (ADR 0254).
                if let Err(e) = super::register::persist_wait(bus, live_waits, &replacement).await {
                    crate::log!(
                        "[EventWait] Could not narrow wait {} on thread {issuer}: {e}. It stays \
                         live, so the stopped task's completion will still reach it.",
                        wait.wait_id,
                    );
                    live_waits.insert(wait).await;
                    continue;
                }
                replacement_id = Some(replacement.wait_id);
                outcome.replacements.push(replacement);
            }
        }
        if let Err(e) =
            super::emit_cancel(bus, &wait, EventWaitCancelCause::AgentStandDown, None).await
        {
            // Nothing was written, so the wait is still live in the store and
            // goes back in the cache. The stopped task will then reach it.
            crate::log!(
                "[EventWait] Stand-down of wait {} on thread {issuer} failed: {e}. Re-arming it.",
                wait.wait_id,
            );
            // The old wait covers the remaining tasks again, so its
            // replacement must go, or one completion would be delivered twice.
            if let Some(id) = replacement_id {
                outcome.replacements.retain(|r| r.wait_id != id);
                withdraw(bus, live_waits, id).await;
            }
            live_waits.insert(wait).await;
        }
    }
    stop.send();
    Some(outcome)
}

/// Cancel a replacement wait whose original could not be stood down. A
/// replacement that already resolved is gone, and so is the risk.
async fn withdraw(bus: &EventBus, live_waits: &LiveWaits, wait_id: Uuid) {
    let Some(replacement) = live_waits.take(wait_id).await else {
        return;
    };
    if let Err(e) = super::emit_cancel(
        bus,
        &replacement,
        EventWaitCancelCause::AgentStandDown,
        None,
    )
    .await
    {
        crate::log!(
            "[EventWait] Could not withdraw replacement wait {wait_id}: {e}. It and the \
             original both stay live, so a remaining task may be delivered twice.",
        );
        live_waits.insert(replacement).await;
    }
}

/// The engine wait that replaces `old` once one of its tasks was stopped.
///
/// Keeps `old`'s watermark, so its catch-up scan reaches any completion of the
/// remaining tasks since `old` was armed: nothing landing in between is lost.
/// Keeps its deadline too, which already covered the remaining tasks.
fn narrowed(
    old: &LiveWait,
    on: Vec<EventSubscription>,
    running: &[RunningTaskHandle],
    owner: Option<Uuid>,
) -> LiveWait {
    let still: Vec<String> = running
        .iter()
        .filter(|h| wait_covers_task(&on, &h.task_id, owner))
        .map(|h| task_label(h.description.as_deref(), &h.command))
        .collect();
    let reason = if still.is_empty() {
        old.reason.clone()
    } else {
        armed_reason(&still)
    };
    LiveWait {
        wait_id: Uuid::new_v4(),
        thread_id: old.thread_id,
        tool_use_id: engine_tool_use_id(),
        on,
        reason,
        armed_at: Utc::now(),
        expires_at: old.expires_at,
        watermark: old.watermark,
    }
}

impl LucidosEngine {
    /// Stop one background task for `issuer`, so that its completion does not
    /// re-open `issuer`. See [`stop_and_stand_down`].
    pub(crate) async fn stop_background_task_as(
        &self,
        issuer: Uuid,
        task_id: &str,
    ) -> Option<StoppedTask> {
        let outcome = stop_and_stand_down(
            &self.bash_background,
            &self.event_bus,
            &self.live_waits,
            issuer,
            task_id,
        )
        .await?;
        for replacement in &outcome.replacements {
            self.catch_up_event_wait(replacement).await;
        }
        Some(outcome.stopped)
    }

    /// Stop every running background task of `thread_id`, as the thread
    /// itself. See [`stop_all_and_stand_down`].
    pub(crate) async fn stop_background_tasks_of(&self, thread_id: Uuid) -> Vec<StoppedTask> {
        let outcomes = stop_all_and_stand_down(
            &self.bash_background,
            &self.event_bus,
            &self.live_waits,
            thread_id,
        )
        .await;
        let mut stopped = Vec::with_capacity(outcomes.len());
        for outcome in outcomes {
            for replacement in &outcome.replacements {
                self.catch_up_event_wait(replacement).await;
            }
            stopped.push(outcome.stopped);
        }
        stopped
    }
}

/// Stop every running background task of `thread_id` that nobody is already
/// stopping, each through [`stop_and_stand_down`] as the thread itself.
///
/// For `lucidos hardened mark`. The marker certifies the branch, so a task
/// still running started before it and cannot change the verdict.
pub(super) async fn stop_all_and_stand_down(
    registry: &BackgroundBashRegistry,
    bus: &EventBus,
    live_waits: &LiveWaits,
    thread_id: Uuid,
) -> Vec<StopOutcome> {
    let mut outcomes = Vec::new();
    for task in registry.running_for_thread(thread_id).await {
        if task.stop_requested {
            continue;
        }
        if let Some(outcome) =
            stop_and_stand_down(registry, bus, live_waits, thread_id, &task.task_id).await
        {
            outcomes.push(outcome);
        }
    }
    outcomes
}

/// What the user reads on the wait's transcript row, after `Waiting for `. So
/// it is a noun phrase naming the work, never a sentence about waiting.
///
/// The wait fires on the FIRST covered task to finish, so several tasks read as
/// "the first of", which is what actually re-opens the thread. Takes each
/// task's [`task_label`].
fn armed_reason(labels: &[String]) -> String {
    match labels {
        [one] => format!("{one} to finish"),
        many => {
            let mut names: Vec<String> = many.iter().take(LABELS_SHOWN).cloned().collect();
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
fn task_label(description: Option<&str>, command: &str) -> String {
    match description {
        Some(description) => one_short_line(description),
        None => format!("the background command \"{}\"", one_short_line(command)),
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
/// With no running task, as for a wait over unread results alone, the margin
/// alone sizes it: their completions are already in the store.
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

#[cfg(test)]
#[path = "background_task_stop_tests.rs"]
mod stop_tests;

#[cfg(test)]
#[path = "background_task_arm_tests.rs"]
mod arm_tests;
