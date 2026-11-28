use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::register::MAX_LIVE_WAITS_PER_THREAD;
use super::{LiveWait, LiveWaits};
use crate::core::event_subscription::EventSubscription;
use crate::engine::bash_background::{BackgroundBashRegistry, RunningTaskHandle, UnreadResult};
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::{BackgroundWaitTiming, EventWaitCancelCause};
use crate::engine::LucidosEngine;

const BACKGROUND_BASH_COMPLETED: &str = "BackgroundBashCompleted";

/// INVARIANT: Added to the latest watchdog deadline when sizing the wait.
///
/// The watchdog kills the child at its deadline, and the completion event is
/// emitted after that, so a wait expiring exactly at the deadline could lose
/// the race with the work it exists to observe. Generous on purpose: expiring
/// late costs one turn, expiring early costs the whole wait.
const DEADLINE_MARGIN: Duration = Duration::minutes(5);

const ENGINE_TOOL_USE_PREFIX: &str = "engine:bg-task-wait";

const LABEL_MAX_CHARS: usize = 80;

const LABELS_SHOWN: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerTurn {
    /// INVARIANT: Mid-turn, as at the coding-agent `background-tasks` route. Polled tasks
    /// stay uncovered, or one completion would reach the agent twice.
    Running,
    Ended,
}

impl LucidosEngine {
    pub async fn arm_wait_for_running_background_tasks(
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

    /// INVARIANT: A wait of `thread_id` is delivering this event. When it is a background
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

#[derive(Debug, Default)]
pub(super) struct Armed {
    pub(super) covered: Vec<String>,
    pub(super) wait: Option<LiveWait>,
}

pub(super) async fn arm_wait_for_running_tasks(
    registry: &BackgroundBashRegistry,
    bus: &EventBus,
    live_waits: &LiveWaits,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    turn: OwnerTurn,
) -> Armed {
    let unread_results = || match turn {
        OwnerTurn::Ended => registry.unread_results_for_thread(thread_id),
        OwnerTurn::Running => Vec::new(),
    };
    if !registry.has_running_for_thread(thread_id).await && unread_results().is_empty() {
        return Armed::default();
    }

    // INVARIANT: Now the watermark, and BEFORE the authoritative registry read below.
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
            crate::log_legacy!(
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
    // INVARIANT: Engine-armed waits COUNT toward the recent-subscription cap,
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
            crate::log_legacy!(
                "[EventWait] Subscription-count read failed for thread {thread_id}: {e}."
            );
        })
        .ok();

    let plan = plan_wait(&running, &unread, &live, recent, thread_id, turn);
    let uncovered = match &plan {
        ArmingPlan::Arm(uncovered) => uncovered,
        ArmingPlan::NothingUncovered => return unarmed,
        // INVARIANT: Every refusal is a real regression back to the stall this
        // prevents, so it says so rather than reading as a routine skip.
        ArmingPlan::Refused(why) => {
            crate::log_legacy!(
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

    // INVARIANT: The watermark read at the top, NOT a fresh one: every running task in
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
            crate::log_legacy!(
                "[EventWait] Could not find the unread completions of thread {thread_id}: {e}. \
                 Its background work will finish unwatched."
            );
            return unarmed;
        }
    };
    let background_timing = background_wait_timing(&uncovered.running);
    let mut wait = super::register::build_wait_at(
        thread_id,
        &engine_tool_use_id(),
        on,
        &background_wait_reason(background_timing, &uncovered.labels()),
        timeout_secs,
        watermark,
    );
    wait.background_timing = background_timing;
    wait.expires_at =
        deadline_for(&uncovered.running, wait.armed_at).max(wait.armed_at + Duration::seconds(1));
    if let Err(e) = super::register::persist_wait(bus, live_waits, &wait).await {
        crate::log_legacy!(
            "[EventWait] Could not arm a background-task wait for thread {thread_id}: {e}. \
             Its background work will finish unwatched."
        );
        return unarmed;
    }
    crate::log_legacy!(
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

#[derive(Debug, PartialEq)]
pub(super) enum ArmingPlan<'a> {
    Arm(Uncovered<'a>),
    NothingUncovered,
    Refused(String),
}

#[derive(Debug, PartialEq)]
pub(super) struct Uncovered<'a> {
    pub(super) running: Vec<&'a RunningTaskHandle>,
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

/// INVARIANT: Decide whether to arm, and over which tasks.
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

fn wait_covers_task(on: &[EventSubscription], task_id: &str, owner: Option<Uuid>) -> bool {
    EventSubscription::any_matches(
        on,
        BACKGROUND_BASH_COMPLETED,
        &completion_payload(task_id, owner),
    )
}

fn completion_payload(task_id: &str, owner: Option<Uuid>) -> serde_json::Value {
    crate::core::event_subscription::matchable_payload(
        BACKGROUND_BASH_COMPLETED,
        serde_json::json!({ "task_id": task_id }),
        owner,
    )
}

/// INVARIANT: The matchable view of the completion a stop produces: killed by us, so
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

fn engine_tool_use_id() -> String {
    format!("{ENGINE_TOOL_USE_PREFIX}:{}", Uuid::new_v4())
}

fn is_engine_armed(wait: &LiveWait) -> bool {
    wait.tool_use_id
        .strip_prefix(ENGINE_TOOL_USE_PREFIX)
        .is_some_and(|rest| rest.starts_with(':'))
}

#[derive(Debug, PartialEq)]
pub(super) enum StandDown {
    End,
    EndWithOthers,
    Narrow(Vec<EventSubscription>),
}

/// INVARIANT: Decide what a stop of `task_id` does to one wait on the stopping thread.
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

#[derive(Debug, Clone, PartialEq)]
pub struct StoppedTask {
    pub task_id: String,
    pub label: String,
    pub ended_with_others: Vec<String>,
}

#[derive(Debug)]
pub(super) struct StopOutcome {
    pub(super) stopped: StoppedTask,
    /// INVARIANT: Engine waits armed in place of ones that also covered other tasks.
    /// Each still owes its catch-up scan, which needs the engine.
    pub(super) replacements: Vec<LiveWait>,
}

/// INVARIANT: Stop one background task on behalf of `issuer`, standing down the issuer's
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
        // INVARIANT: Out of the cache first, so it cannot resolve on anything while the
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
                if let Err(e) = super::register::persist_wait(bus, live_waits, &replacement).await {
                    crate::log_legacy!(
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
            // INVARIANT: Nothing was written, so the wait is still live in the store and
            // goes back in the cache. The stopped task will then reach it.
            crate::log_legacy!(
                "[EventWait] Stand-down of wait {} on thread {issuer} failed: {e}. Re-arming it.",
                wait.wait_id,
            );
            // INVARIANT: The old wait covers the remaining tasks again, so its
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
        crate::log_legacy!(
            "[EventWait] Could not withdraw replacement wait {wait_id}: {e}. It and the \
             original both stay live, so a remaining task may be delivered twice.",
        );
        live_waits.insert(replacement).await;
    }
}

/// INVARIANT: The engine wait that replaces `old` once one of its tasks was stopped.
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
    let still: Vec<_> = running
        .iter()
        .filter(|h| wait_covers_task(&on, &h.task_id, owner))
        .collect();
    let background_timing = background_wait_timing(&still);
    let reason = if still.is_empty() {
        old.reason.clone()
    } else {
        background_wait_reason(background_timing, &running_labels(&still))
    };
    LiveWait {
        wait_id: Uuid::new_v4(),
        thread_id: old.thread_id,
        tool_use_id: engine_tool_use_id(),
        on,
        reason,
        armed_at: Utc::now(),
        expires_at: old.expires_at,
        background_timing: background_timing.or(old.background_timing.filter(|_| still.is_empty())),
        watermark: old.watermark,
    }
}

impl LucidosEngine {
    pub async fn stop_background_task_as(
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

    pub async fn stop_background_tasks_of(&self, thread_id: Uuid) -> Vec<StoppedTask> {
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

fn task_label(description: Option<&str>, command: &str) -> String {
    match description {
        Some(description) => one_short_line(description),
        None => format!("the background command \"{}\"", one_short_line(command)),
    }
}

fn one_short_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= LABEL_MAX_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(LABEL_MAX_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

fn background_wait_timing(tasks: &[&RunningTaskHandle]) -> Option<BackgroundWaitTiming> {
    let deadline = tasks
        .iter()
        .filter(|task| !task.queued_for_build_slot)
        .map(|task| task.watchdog_deadline)
        .max();
    match deadline {
        Some(deadline) => Some(BackgroundWaitTiming::Running { deadline }),
        None if !tasks.is_empty() => Some(BackgroundWaitTiming::Queued),
        None => None,
    }
}

fn background_wait_reason(timing: Option<BackgroundWaitTiming>, labels: &[String]) -> String {
    if timing == Some(BackgroundWaitTiming::Queued) {
        "a build slot".to_string()
    } else {
        armed_reason(labels)
    }
}

fn running_labels(tasks: &[&RunningTaskHandle]) -> Vec<String> {
    tasks
        .iter()
        .map(|task| task_label(task.description.as_deref(), &task.command))
        .collect()
}

impl LucidosEngine {
    pub(super) async fn refresh_background_wait_timing(&self, wait: &mut LiveWait) {
        if !is_engine_armed(wait) {
            return;
        }
        let running = self
            .bash_background
            .running_for_thread(wait.thread_id)
            .await;
        let tasks: Vec<_> = running
            .iter()
            .filter(|task| wait_covers_task(&wait.on, &task.task_id, Some(wait.thread_id)))
            .collect();
        let Some(timing) = background_wait_timing(&tasks) else {
            return;
        };
        let reason = background_wait_reason(Some(timing), &running_labels(&tasks));
        let expires_at = deadline_for(&tasks, wait.armed_at);
        if Some(timing) == wait.background_timing
            && reason == wait.reason
            && expires_at == wait.expires_at
        {
            return;
        }
        if self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: wait.thread_id,
                event: crate::engine::thread_events::ThreadEvent::EventWaitTimingUpdated {
                    wait_id: wait.wait_id,
                    reason: reason.clone(),
                    expires_at,
                    background_timing: timing,
                },
                meta: crate::engine::thread_events::EventMeta::NONE,
            })
            .await
            .is_ok()
        {
            self.live_waits
                .update_timing(wait.wait_id, reason.clone(), expires_at, timing)
                .await;
        }
        // INVARIANT: Even if the write failed, a live watchdog can still prove this
        // snapshot should not expire. The next sweep retries the write.
        wait.reason = reason;
        wait.expires_at = expires_at;
        wait.background_timing = Some(timing);
    }
}

/// INVARIANT: Seconds until the wait should expire: past the last watchdog deadline among
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
    (deadline_for(tasks, now) - now)
        .num_seconds()
        .clamp(1, super::register::MAX_TIMEOUT_SECS)
}

fn deadline_for(tasks: &[&RunningTaskHandle], armed_at: DateTime<Utc>) -> DateTime<Utc> {
    let latest = tasks
        .iter()
        .map(|task| task.watchdog_deadline + DEADLINE_MARGIN)
        .max()
        .unwrap_or(armed_at + DEADLINE_MARGIN);
    latest.min(armed_at + Duration::seconds(super::register::MAX_TIMEOUT_SECS))
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
