use crate::engine::types::SharedEngine;
use crate::triggers::TriggerConfig;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{trigger_id_to_uuid, MISSED_TASK_GRACE_MINUTES};

/// INVARIANT: Information about a tracked task.
///
/// `cancel_token` governs scheduling only: the runner observes it between
/// fires and while it waits on one. It never reaches a run, so stopping a
/// runner leaves a run already executing alone. Callers must not abort the
/// `JoinHandle` directly: the runner then cannot drop its queued fire.
pub(super) struct TrackedTask {
    pub(super) handle: JoinHandle<()>,
    pub(super) task_name: String,
    pub(super) cancel_token: CancellationToken,
    /// INVARIANT: The cron expressions and timezone this runner was spawned with. The
    /// runner never re-reads them, so [`rearm_after_lag`] compares these with
    /// the registry to find a runner an unseen update left on its old schedule.
    pub(super) schedule: Vec<String>,
    pub(super) timezone: String,
}

fn track(tracked: &mut HashMap<uuid::Uuid, TrackedTask>, task_id: uuid::Uuid, task: TrackedTask) {
    if let Some(displaced) = tracked.insert(task_id, task) {
        displaced.cancel_token.cancel();
    }
}

/// INVARIANT: Spawn a task runner that executes on schedule.
///
/// Returns the `JoinHandle` (for liveness checks) and a `CancellationToken` the
/// runner observes between fires. Cancel the token instead of aborting the
/// handle, so a still-queued fire is dropped before the runner exits.
fn spawn_task_runner(
    trigger_id: String,
    task_name: String,
    cron_expressions: Vec<String>,
    timezone: String,
    engine: SharedEngine,
    trigger_configs: Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
) -> (JoinHandle<()>, CancellationToken) {
    let cancel_token = CancellationToken::new();
    let task_cancel = cancel_token.clone();
    let handle = tokio::spawn(async move {
        // INVARIANT: Nothing here catches a panic. An `Err` is a loop that gave up, say
        // on a corrupt cron expression, and it is logged below. A panic ends
        // the task, and `check_task_health_and_restart` re-arms it.
        let result = run_task_loop(
            trigger_id,
            task_name.clone(),
            cron_expressions,
            timezone,
            engine,
            trigger_configs,
            task_cancel,
        )
        .await;

        match result {
            Ok(reason) => {
                log_legacy!("[Scheduler] Task '{}' exited: {}", task_name, reason);
            }
            Err(e) => {
                log_legacy!("[Scheduler] Task '{}' crashed: {}", task_name, e);
            }
        }
    });
    (handle, cancel_token)
}

async fn run_task_loop(
    trigger_id: String,
    task_name: String,
    cron_expressions: Vec<String>,
    timezone: String,
    engine: SharedEngine,
    trigger_configs: Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
    cancel_token: CancellationToken,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use crate::scheduler::cron_rules::next_occurrence_multi;
    let schedules: Vec<cron::Schedule> = cron_expressions
        .iter()
        .map(|expr| {
            crate::scheduler::cron_rules::parse_standard_cron(expr).map_err(|e| {
                format!(
                    "Invalid cron expression '{}' for task {}: {}",
                    expr, task_name, e
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    if schedules.is_empty() {
        return Err("No cron expressions for task".into());
    }

    let tz: chrono_tz::Tz = timezone.parse().unwrap_or_else(|_| {
        log_legacy!(
            "[Scheduler] Invalid timezone '{}' for task {}, using UTC",
            timezone,
            task_name
        );
        chrono_tz::UTC
    });

    settle_missed_slots(
        None,
        &schedules,
        tz,
        &trigger_id,
        &task_name,
        &engine,
        &trigger_configs,
    )
    .await?;

    loop {
        if engine.is_shutting_down() {
            return Ok(FireRefusal::ShuttingDown.as_str().to_string());
        }
        if cancel_token.is_cancelled() {
            return Ok("cancelled".to_string());
        }
        let next: chrono::DateTime<chrono_tz::Tz> = match next_occurrence_multi(&schedules, tz) {
            Some(t) => t,
            None => {
                return Ok("no more occurrences".to_string());
            }
        };

        let next_utc = next.with_timezone(&chrono::Utc);
        let now_utc = chrono::Utc::now();

        if next_utc > now_utc {
            let wait_secs = (next_utc - now_utc).num_seconds();
            if wait_secs > 3600 {
                log_legacy!(
                    "[Scheduler] Task '{}' waiting until {} ({:.1} hours)",
                    task_name,
                    next.format("%Y-%m-%d %H:%M:%S %Z"),
                    wait_secs as f64 / 3600.0
                );
            }
        }

        // INVARIANT: Poll with short sleeps until the scheduled time arrives.
        // This ensures we wake up promptly after macOS system sleep,
        // where monotonic timers (tokio::time::sleep) don't advance.
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
        loop {
            if engine.is_shutting_down() {
                return Ok(FireRefusal::ShuttingDown.as_str().to_string());
            }
            let now = chrono::Utc::now();
            if now >= next_utc {
                break;
            }
            let remaining = (next_utc - now)
                .to_std()
                .unwrap_or(std::time::Duration::from_secs(1));
            tokio::select! {
                _ = tokio::time::sleep(remaining.min(POLL_INTERVAL)) => {}
                _ = cancel_token.cancelled() => {
                    return Ok("cancelled".to_string());
                }
            }
        }

        let paused = {
            let configs = trigger_configs.read().unwrap();
            configs.get(&trigger_id).map(|c| c.paused)
        };
        if let Some(refusal) = scheduled_fire_refusal(engine.is_shutting_down(), paused) {
            return Ok(refusal.as_str().to_string());
        }

        let actual_now = chrono::Utc::now();
        let delay = actual_now - next_utc;
        if delay > chrono::Duration::minutes(MISSED_TASK_GRACE_MINUTES) {
            log_legacy!("[Scheduler] Task '{}' woke up {} minutes late (scheduled {}, actual {}), skipping this occurrence",
                task_name,
                delay.num_minutes(),
                next.format("%H:%M:%S"),
                actual_now.format("%H:%M:%S")
            );
            settle_missed_slots(
                Some(next_utc - chrono::Duration::nanoseconds(1)),
                &schedules,
                tz,
                &trigger_id,
                &task_name,
                &engine,
                &trigger_configs,
            )
            .await?;
            continue;
        }

        if delay.num_seconds() > 5 {
            log_legacy!(
                "[Scheduler] Task '{}' executing {}s after scheduled time",
                task_name,
                delay.num_seconds()
            );
        }

        if fire_scheduled_slot(&engine.thread_queue, &trigger_id, &task_name, &cancel_token).await
            == SlotEnd::RunnerStopped
        {
            return Ok("cancelled".to_string());
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SlotEnd {
    Finished,
    RunnerStopped,
}

/// INVARIANT: Submit one on-time cron fire through the Thread Queue and wait for it.
///
/// Admission control may hold the fire until capacity frees. Awaiting
/// completion preserves the loop's semantics: the next occurrence is computed
/// after the run ends, so saturation back-pressures this trigger's schedule
/// instead of stacking unbounded concurrent runs. Execution bookkeeping
/// (`ACTIVE_TASK_COUNT`, `record_trigger_executed`, failure logging) lives in
/// the queue executor.
async fn fire_scheduled_slot(
    queue: &Arc<crate::engine::thread_queue::ThreadQueue>,
    trigger_id: &str,
    task_name: &str,
    cancel_token: &CancellationToken,
) -> SlotEnd {
    let outcome = queue
        .submit(
            crate::engine::thread_queue::ThreadQueueRequest::Cron {
                retry_of: None,
                trigger_id: trigger_id.to_string(),
            },
            None,
        )
        .await;
    if !outcome.admitted {
        log_legacy!(
            "[Scheduler] Task '{}' queued at position {} (system at capacity)",
            task_name,
            outcome.position
        );
    }
    tokio::select! {
        _ = outcome.completion => SlotEnd::Finished,
        _ = cancel_token.cancelled() => {
            if let Err(e) = queue
                .drop_entry(outcome.entry_id, "trigger cancelled", None)
                .await
            {
                log_legacy!(
                    "[Scheduler] Runner for task '{}' stopped; admitted run left alone (entry already running or gone): {}",
                    task_name,
                    e
                );
            }
            SlotEnd::RunnerStopped
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FireRefusal {
    /// INVARIANT: Engine shutdown has begun, so the API the run may call back into is
    /// closing. The slot is left unrun and records no `TriggerExecuted`, so the
    /// next engine's missed-slot catch-up fires it.
    ShuttingDown,
    Paused,
    Deleted,
}

impl FireRefusal {
    fn as_str(self) -> &'static str {
        match self {
            Self::ShuttingDown => "engine shutting down",
            Self::Paused => "trigger paused",
            Self::Deleted => "trigger deleted",
        }
    }
}

fn scheduled_fire_refusal(shutting_down: bool, paused: Option<bool>) -> Option<FireRefusal> {
    if shutting_down {
        return Some(FireRefusal::ShuttingDown);
    }
    match paused {
        Some(false) => None,
        Some(true) => Some(FireRefusal::Paused),
        None => Some(FireRefusal::Deleted),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatchUp {
    Fire,
    Skip(SkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipReason {
    /// INVARIANT: A run is recorded at or after the slot. A late fire counts: the 07:49 run
    /// of an 07:45 slot means the 07:45 slot has run.
    AlreadyRan,
    SlotPredatesTrigger,
    /// INVARIANT: The event store could not be read. Fail closed: we cannot prove the slot
    /// hasn't run, and a duplicate push notification is worse than a skipped
    /// catch-up.
    HistoryUnavailable,
}

impl SkipReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyRan => "already ran for this slot",
            Self::SlotPredatesTrigger => "slot predates the trigger",
            Self::HistoryUnavailable => "run history unavailable",
        }
    }
}

/// INVARIANT: Decide whether a missed cron slot is still due.
///
/// Fails closed by construction: the only path to [`CatchUp::Fire`] is a
/// positive showing that the slot is due. A `None` `last_run` is *not* evidence
/// of "never ran" — before this function was extracted the guard was
/// `if let Some(last_run)`, so an absent value skipped the check entirely and
/// fired.
///
/// `in_memory_last_run` is the replayed [`TriggerConfig::last_run`] and
/// `history` is an independent read of the same durable truth. The decision
/// takes the later of the two
/// ([`TriggerRunHistory::effective_last_run`](crate::triggers::TriggerRunHistory::effective_last_run)).
fn catch_up_decision(
    slot_utc: chrono::DateTime<chrono::Utc>,
    in_memory_last_run: Option<chrono::DateTime<chrono::Utc>>,
    history: Result<crate::triggers::TriggerRunHistory, sqlx::Error>,
) -> CatchUp {
    let Ok(history) = history else {
        return CatchUp::Skip(SkipReason::HistoryUnavailable);
    };

    if history
        .effective_last_run(in_memory_last_run)
        .is_some_and(|last_run| last_run >= slot_utc)
    {
        return CatchUp::Skip(SkipReason::AlreadyRan);
    }
    if history.created_at.is_some_and(|created| created > slot_utc) {
        return CatchUp::Skip(SkipReason::SlotPredatesTrigger);
    }
    CatchUp::Fire
}

async fn settle_missed_slots(
    missed_after: Option<chrono::DateTime<chrono::Utc>>,
    schedules: &[cron::Schedule],
    tz: chrono_tz::Tz,
    trigger_id: &str,
    task_name: &str,
    engine: &SharedEngine,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use super::skipped_slots::{
        load_slot_floors, plan_missed_slots, report_skipped_slots, SlotPlan,
    };

    let config = {
        let configs = trigger_configs.read().unwrap();
        configs.get(trigger_id).cloned()
    };
    let config = match (
        scheduled_fire_refusal(engine.is_shutting_down(), config.as_ref().map(|c| c.paused)),
        config,
    ) {
        (None, Some(config)) => config,
        (refusal, _) => {
            let reason = refusal.map_or(FireRefusal::Deleted.as_str(), FireRefusal::as_str);
            log_legacy!(
                "[Scheduler] Task '{}' leaves its missed slots alone: {}",
                task_name,
                reason
            );
            return Ok(());
        }
    };

    let floors = match load_slot_floors(engine.pool(), trigger_id, config.last_run).await {
        Ok(floors) => floors,
        Err(e) => {
            log_legacy!(
                "[Scheduler] Task '{}' missed-slot history unavailable: {}",
                task_name,
                e
            );
            return Ok(());
        }
    };
    let now = chrono::Utc::now();
    let grace = chrono::Duration::minutes(MISSED_TASK_GRACE_MINUTES);
    match plan_missed_slots(schedules, tz, floors.floor(missed_after, now), now, grace) {
        SlotPlan::Nothing => Ok(()),
        SlotPlan::Report(slots) => {
            log_legacy!(
                "[Scheduler] Task '{}' skipped {} slot(s) from {} to {}",
                task_name,
                slots.count,
                slots.first,
                slots.last
            );
            match report_skipped_slots(&engine.event_bus, trigger_id, &config.name, tz, slots).await
            {
                Ok(notice) => notice.send_push(engine),
                Err(e) => log_legacy!(
                    "[Scheduler] Task '{}' skipped-slot report failed: {}",
                    task_name,
                    e
                ),
            }
            Ok(())
        }
        SlotPlan::CatchUp(slot) => {
            catch_up_missed_slot(slot, tz, trigger_id, task_name, engine, trigger_configs).await
        }
    }
}

async fn catch_up_missed_slot(
    missed_utc: chrono::DateTime<chrono::Utc>,
    tz: chrono_tz::Tz,
    trigger_id: &str,
    task_name: &str,
    engine: &SharedEngine,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let missed = missed_utc.with_timezone(&tz);
    let config = {
        let configs = trigger_configs.read().unwrap();
        configs.get(trigger_id).cloned()
    };
    let paused = config.as_ref().map(|c| c.paused);

    // INVARIANT: Re-read after the floors load: the trigger may have paused meanwhile.
    let skip = match scheduled_fire_refusal(engine.is_shutting_down(), paused) {
        Some(refusal) => Some(refusal.as_str()),
        None => {
            match crate::core::snapshot::boundary::permits_slot(engine.pool(), missed_utc).await {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(_) => return Err("snapshot_restore_boundary_unavailable".into()),
            }
            let history =
                crate::triggers::load_trigger_run_history(engine.pool(), trigger_id).await;
            if let Err(e) = &history {
                log_legacy!(
                    "[Scheduler] Task '{}' run-history lookup failed: {}",
                    task_name,
                    e
                );
            }
            let in_memory_last_run = config.and_then(|c| c.last_run);
            match catch_up_decision(missed_utc, in_memory_last_run, history) {
                CatchUp::Skip(reason) => Some(reason.as_str()),
                CatchUp::Fire => None,
            }
        }
    };
    if let Some(reason) = skip {
        log_legacy!(
            "[Scheduler] Task '{}' missed at {}, not executing: {}",
            task_name,
            missed.format("%H:%M:%S"),
            reason
        );
        return Ok(());
    }

    log_legacy!(
        "[Scheduler] Task '{}' missed at {} ({}s ago), executing now",
        task_name,
        missed.format("%H:%M:%S"),
        (chrono::Utc::now() - missed_utc).num_seconds()
    );

    let outcome = engine
        .thread_queue
        .submit(
            crate::engine::thread_queue::ThreadQueueRequest::Cron {
                retry_of: None,
                trigger_id: trigger_id.to_string(),
            },
            None,
        )
        .await;
    if outcome.completion.await.is_err() {
        log_legacy!(
            "[Scheduler] Task '{}' grace-period completion channel dropped",
            task_name
        );
    }
    Ok(())
}

/// INVARIANT: React to a trigger lifecycle event: arm or disarm its cron job.
///
/// **This function does not write the registry.** The write chokepoint
/// (`engine::trigger_writes`) already materialized the event before the writing
/// caller regained control; applying it a second time here would not be the
/// harmless redundancy it looks like. A `TriggerCreated` re-apply rebuilds the
/// config from its original payload, so a create immediately followed by a
/// pause would be transiently un-paused again when this loop reached the older
/// event, and the gap to the following `TriggerUpdated` spans real async work.
/// One applier, ordered by the chokepoint's write lock, is the whole point.
///
/// So this reads the registry rather than the event, and reads it *after*
/// taking `trigger_write_lock`. Both halves matter. The lock is what makes the
/// read safe at all: `EventBus::emit` broadcasts from inside the chokepoint's
/// locked span, so this handler can be woken before the apply has run, and
/// blocking on the same lock is what guarantees it has. Reading current state
/// rather than this event's state is then a feature: when several writes land
/// in a burst, every pass arms the job from the newest config instead of
/// re-deriving a stale one.
///
/// What only this function can do is the arming itself, because `tracked_tasks`
/// belongs to the `SchedulerManager`, not the engine.
pub(super) async fn handle_trigger_event(
    event_type: &str,
    trigger_id: &str,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
    tracked_tasks: &Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    engine: &SharedEngine,
) {
    if !crate::triggers::registry::TRIGGER_LIFECYCLE_EVENTS.contains(&event_type) {
        return;
    }
    let task_uuid = trigger_id_to_uuid(trigger_id);
    // INVARIANT: Guard dropped before any side effect below: `thread_queue.drain()` can
    // itself perform a trigger write, which takes this same lock.
    let config = {
        let _applied = engine.trigger_write_lock.lock().await;
        let snapshot = trigger_configs.read().unwrap().get(trigger_id).cloned();
        snapshot
    };

    match event_type {
        "TriggerCreated" => {
            if let Some(config) = config.as_ref() {
                if !config.paused && !config.schedule.is_empty() {
                    register_and_track(config, tracked_tasks, engine, trigger_configs).await;
                    crate::log_legacy!(
                        "[Scheduler] Registered new trigger: {} ({})",
                        config.name,
                        trigger_id
                    );
                }
            }
        }
        "TriggerUpdated" => {
            if let Some(config) = config.as_ref() {
                cancel_tracked_task(tracked_tasks, task_uuid).await;
                if !config.paused && !config.schedule.is_empty() {
                    register_and_track(config, tracked_tasks, engine, trigger_configs).await;
                }
                crate::log_legacy!("[Scheduler] Updated trigger: {}", trigger_id);
                engine.thread_queue.drain().await;
            }
        }
        "TriggerDeleted" => {
            // INVARIANT: Unconditional: the chokepoint may already have removed the
            // registry entry, but the cron job is still armed until we say so.
            cancel_tracked_task(tracked_tasks, task_uuid).await;
            crate::log_legacy!("[Scheduler] Deleted trigger: {}", trigger_id);
        }
        "TriggerEnabled" => {
            if let Some(config) = config.as_ref() {
                if !config.schedule.is_empty() {
                    register_and_track(config, tracked_tasks, engine, trigger_configs).await;
                }
                crate::log_legacy!("[Scheduler] Resumed trigger: {}", trigger_id);
                engine.thread_queue.drain().await;
            }
        }
        "TriggerDisabled" => {
            cancel_tracked_task(tracked_tasks, task_uuid).await;
            crate::log_legacy!("[Scheduler] Paused trigger: {}", trigger_id);
        }
        _ => {}
    }
}

pub(super) async fn register_and_track(
    config: &TriggerConfig,
    tracked_tasks: &Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    engine: &SharedEngine,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
) {
    let task_uuid = trigger_id_to_uuid(&config.id);
    let (handle, cancel_token) = spawn_task_runner(
        config.id.clone(),
        config.name.clone(),
        config.schedule.clone(),
        config.timezone.clone(),
        engine.clone(),
        trigger_configs.clone(),
    );
    let mut tracked = tracked_tasks.write().await;
    track(
        &mut tracked,
        task_uuid,
        TrackedTask {
            handle,
            task_name: config.name.clone(),
            cancel_token,
            schedule: config.schedule.clone(),
            timezone: config.timezone.clone(),
        },
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LagRepair {
    Leave,
    Arm,
    Rearm,
}

/// INVARIANT: Decide the repair for `config`, given the runner tracked for it, if any.
///
/// A paused or deleted trigger needs none: its runner reads the registry
/// before every fire and exits by itself. A finished runner belongs to the
/// health monitor, and a dead schedule was untracked by it on purpose.
fn lag_repair(config: &TriggerConfig, tracked: Option<&TrackedTask>) -> LagRepair {
    if config.paused || config.schedule.is_empty() || config.schedule_error().is_some() {
        return LagRepair::Leave;
    }
    match tracked {
        None => LagRepair::Arm,
        Some(task) if task.handle.is_finished() => LagRepair::Leave,
        Some(task) if task.schedule == config.schedule && task.timezone == config.timezone => {
            LagRepair::Leave
        }
        Some(_) => LagRepair::Rearm,
    }
}

pub(super) async fn rearm_after_lag(
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
    tracked_tasks: &Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    engine: &SharedEngine,
) {
    let configs: Vec<TriggerConfig> = {
        let _applied = engine.trigger_write_lock.lock().await;
        let snapshot: Vec<TriggerConfig> =
            trigger_configs.read().unwrap().values().cloned().collect();
        snapshot
    };
    let mut repaired = 0usize;
    for config in &configs {
        let task_uuid = trigger_id_to_uuid(&config.id);
        let repair = lag_repair(config, tracked_tasks.read().await.get(&task_uuid));
        match repair {
            LagRepair::Leave => continue,
            LagRepair::Arm => {}
            LagRepair::Rearm => cancel_tracked_task(tracked_tasks, task_uuid).await,
        }
        register_and_track(config, tracked_tasks, engine, trigger_configs).await;
        repaired += 1;
        crate::log_legacy!(
            "[Scheduler] Lag repair: {:?} trigger '{}' ({})",
            repair,
            config.name,
            config.id
        );
    }
    crate::log_legacy!(
        "[Scheduler] Lag repair checked {} trigger(s), re-armed {}",
        configs.len(),
        repaired
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventTriggerSkip {
    ShuttingDown,
    MaxDepth,
}

fn event_trigger_skip_reason(
    is_shutting_down: bool,
    depth: u32,
    ceiling: u32,
) -> Option<EventTriggerSkip> {
    if is_shutting_down {
        Some(EventTriggerSkip::ShuttingDown)
    } else if depth >= ceiling {
        Some(EventTriggerSkip::MaxDepth)
    } else {
        None
    }
}

const CAP_REPORT_EVENT_TYPE: &str = "NotificationCreated";

const CAP_NOTIFY_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10 * 60);

static CAP_NOTIFIED: std::sync::LazyLock<std::sync::Mutex<HashMap<String, std::time::Instant>>> =
    std::sync::LazyLock::new(Default::default);

fn cap_notification_due(trigger_id: &str) -> bool {
    let Ok(mut seen) = CAP_NOTIFIED.lock() else {
        // INVARIANT: A poisoned cooldown map must not swallow the report: the user losing
        // a notification is the failure this whole surface exists to prevent.
        return true;
    };
    let due = seen
        .get(trigger_id)
        .is_none_or(|t| t.elapsed() >= CAP_NOTIFY_COOLDOWN);
    if due {
        seen.insert(trigger_id.to_string(), std::time::Instant::now());
    }
    due
}

/// INVARIANT: Tell the user which triggers the depth cap just stopped, and why.
///
/// **A log line is not enough.** While the depth stopped at the first
/// `tokio::spawn`, a chain routed through one was uncounted and ran forever.
/// Counting it means a legitimate long chain can now hit the ceiling. A
/// verification step that silently stops running is worse than one that runs
/// too often, so the cap announces itself and names the knob.
///
/// Two things keep the report from feeding itself. It is only sent when a
/// trigger really would have fired, so a deep event nobody subscribes to stays
/// silent. And it is emitted at the capped depth, so the notification is itself
/// at or past the ceiling: a trigger subscribed to it is skipped, and that skip
/// is not reported (see [`CAP_REPORT_EVENT_TYPE`]).
async fn report_depth_cap(
    engine: &SharedEngine,
    event_type: &str,
    depth: u32,
    ceiling: u32,
    stopped: &[TriggerConfig],
) {
    if event_type == CAP_REPORT_EVENT_TYPE {
        return;
    }
    for config in stopped {
        if !cap_notification_due(&config.id) {
            continue;
        }
        let title = format!("{} did not fire", config.name);
        let message = format!(
            "The event chain reached its limit of {ceiling} trigger fires, so \
             \"{}\" was not fired by {event_type} (chain depth {depth}). Raise \
             max_event_trigger_depth in the capacity policy if this chain is \
             meant to be longer.",
            config.name
        );
        let notification_id = uuid::Uuid::new_v4();
        if let Err(e) = engine
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::NotificationCreated {
                    id: notification_id.to_string(),
                    title: title.clone(),
                    message: message.clone(),
                    task_id: Some(config.id.clone()),
                    app_id: None,
                    thread_id: None,
                    event_id: None,
                    tap: crate::scheduler::notifications::Tap::Modal,
                    actor: None,
                    audience: None,
                    push_group: None,
                },
            ))
            .await
        {
            crate::log_legacy!(
                "[Scheduler] Failed to emit depth-cap notification for '{}': {}",
                config.name,
                e
            );
        }
        crate::scheduler::push::send_push_to_all(engine, &title, &message, Some(notification_id));
    }
}

pub(super) async fn fire_matching_triggers(
    dispatch: &super::TriggerDispatch,
    source_event_id: uuid::Uuid,
    excluded: &HashSet<String>,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
    engine: &SharedEngine,
) {
    let super::TriggerDispatch {
        event_type,
        payload,
        depth,
        emitting_trigger_id,
        origin_thread_id,
    } = dispatch;
    let (depth, origin_thread_id) = (*depth, *origin_thread_id);
    let matching = || {
        let configs = trigger_configs.read().unwrap();
        let mut matched = crate::triggers::find_matching_event_triggers(
            &configs,
            event_type,
            payload,
            emitting_trigger_id.as_deref(),
        );
        matched.retain(|t| !excluded.contains(&t.id));
        matched
    };

    let ceiling = engine.thread_queue.policy().await.max_event_trigger_depth;
    match event_trigger_skip_reason(engine.is_shutting_down(), depth, ceiling) {
        Some(EventTriggerSkip::ShuttingDown) => {
            crate::log_legacy!(
                "[Scheduler] Engine shutting down, not firing triggers for event '{}'",
                event_type
            );
            return;
        }
        Some(EventTriggerSkip::MaxDepth) => {
            let stopped = matching();
            if stopped.is_empty() {
                return;
            }
            crate::log_legacy!(
                "[Scheduler] Event '{}' at depth {}: skipping {} trigger(s) to prevent recursion",
                event_type,
                depth,
                stopped.len()
            );
            // INVARIANT: At the capped depth, so the notification is itself past the
            // ceiling and cannot start a new chain. See `report_depth_cap`.
            crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH
                .scope(
                    depth,
                    report_depth_cap(engine, event_type, depth, ceiling, &stopped),
                )
                .await;
            return;
        }
        None => {}
    }

    let matching = matching();
    if matching.is_empty() {
        return;
    }

    crate::log_legacy!(
        "[Scheduler] Event '{}' matched {} trigger(s)",
        event_type,
        matching.len()
    );

    let next_depth = depth + 1;
    for config in matching {
        let outcome = engine
            .thread_queue
            .submit(
                crate::engine::thread_queue::ThreadQueueRequest::EventTrigger {
                    retry_of: None,
                    trigger_id: config.id.clone(),
                    event_type: event_type.to_string(),
                    event_payload: payload.clone(),
                    depth: next_depth,
                    origin_thread_id,
                    source_event_id: Some(source_event_id),
                },
                None,
            )
            .await;
        if !outcome.admitted {
            crate::log_legacy!(
                "[Scheduler] Event trigger '{}' queued at position {} (system at capacity)",
                config.name,
                outcome.position
            );
        }
    }
}

/// INVARIANT: Signal a tracked runner to stop scheduling, then drop its handle.
///
/// A run already executing is not touched: a save, pause or delete applies to
/// the next fire. Cancelling the token, rather than aborting the `JoinHandle`,
/// lets the runner drop a fire that is still queued.
async fn cancel_tracked_task(
    tracked_tasks: &Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    task_id: uuid::Uuid,
) {
    let mut tracked = tracked_tasks.write().await;
    if let Some(task) = tracked.remove(&task_id) {
        task.cancel_token.cancel();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FinishedRunner {
    Restart,
    /// INVARIANT: Drop the tracked entry, naming the problem. A runner whose cron can
    /// never fire exits in microseconds, so restarting it logged a crash and
    /// spawned a replacement twice a minute, forever.
    Drop(String),
    Leave,
}

fn finished_runner_action(config: &TriggerConfig) -> FinishedRunner {
    if let Some(problem) = config.schedule_error() {
        return FinishedRunner::Drop(problem);
    }
    if config.paused || config.schedule.is_empty() {
        return FinishedRunner::Leave;
    }
    FinishedRunner::Restart
}

pub(super) async fn check_task_health_and_restart(
    tracked: Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    engine: SharedEngine,
    trigger_configs: Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
) {
    if engine.is_shutting_down() {
        return;
    }

    let mut to_restart: Vec<(uuid::Uuid, String, String, Vec<String>, String)> = Vec::new();
    let mut to_drop: Vec<uuid::Uuid> = Vec::new();

    {
        let tracked_read = tracked.read().await;
        let configs = trigger_configs.read().unwrap();
        for (task_id, task_info) in tracked_read.iter() {
            if !task_info.handle.is_finished() {
                continue;
            }
            let matching = configs
                .values()
                .find(|c| trigger_id_to_uuid(&c.id) == *task_id);
            let Some(config) = matching else {
                continue;
            };
            match finished_runner_action(config) {
                FinishedRunner::Restart => {
                    log_legacy!(
                        "[Scheduler] Task '{}' crashed or exited unexpectedly, will restart",
                        task_info.task_name
                    );
                    to_restart.push((
                        *task_id,
                        config.id.clone(),
                        config.name.clone(),
                        config.schedule.clone(),
                        config.timezone.clone(),
                    ));
                }
                FinishedRunner::Drop(problem) => {
                    log_legacy!(
                        "[Scheduler] Task '{}' exited because its schedule can never fire: {}. Not restarting it; fix the cron to re-arm it.",
                        task_info.task_name,
                        problem
                    );
                    to_drop.push(*task_id);
                }
                FinishedRunner::Leave => {}
            }
        }
    }

    if !to_drop.is_empty() {
        let mut tracked_write = tracked.write().await;
        for task_id in to_drop {
            tracked_write.remove(&task_id);
        }
    }

    for (task_id, trigger_id, task_name, schedule, timezone) in to_restart {
        {
            let mut tracked_write = tracked.write().await;
            tracked_write.remove(&task_id);
        }

        let (handle, cancel_token) = spawn_task_runner(
            trigger_id,
            task_name.clone(),
            schedule.clone(),
            timezone.clone(),
            engine.clone(),
            trigger_configs.clone(),
        );

        {
            let mut tracked_write = tracked.write().await;
            track(
                &mut tracked_write,
                task_id,
                TrackedTask {
                    handle,
                    task_name: task_name.clone(),
                    cancel_token,
                    schedule,
                    timezone,
                },
            );
        }

        log_legacy!("[Scheduler] Restarted task '{}'", task_name);
    }
}

/// INVARIANT: An hourly cron whose latest slot falls 90 to 150 seconds before
/// `restored_at`, so no slot fires between the restore and the assertion.
/// The restore floor drops that slot; the catch-up grace keeps `permits_slot`
/// as the backstop should the floor regress.
#[cfg(any(test, feature = "test-support"))]
fn pre_restore_hourly_cron(restored_at: chrono::DateTime<chrono::Utc>) -> String {
    use chrono::Timelike;
    format!(
        "0 {} * * * *",
        (restored_at - chrono::Duration::seconds(90)).minute()
    )
}

#[cfg(feature = "test-support")]
pub async fn assert_snapshot_catch_up_excluded(engine: &SharedEngine) {
    let id = "snapshot-combined-cron";
    let restored_at = crate::core::snapshot::boundary::read(engine.pool())
        .await
        .unwrap()
        .expect("restore boundary installed")
        .restored_at;
    let expression = pre_restore_hourly_cron(restored_at);
    let config = TriggerConfig::from_created_payload(&serde_json::json!({
        "trigger_id": id, "name": "Synthetic cron", "schedule": [expression],
        "timezone": "UTC", "run": {"type":"intent", "intent":"Synthetic fixture"}
    }))
    .unwrap();
    let configs = Arc::new(std::sync::RwLock::new(HashMap::from([(id.into(), config)])));
    let schedule = crate::scheduler::cron_rules::parse_standard_cron(&expression).unwrap();
    settle_missed_slots(
        None,
        &[schedule],
        chrono_tz::UTC,
        id,
        "Synthetic cron",
        engine,
        &configs,
    )
    .await
    .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'TriggerStarted' AND payload->>'trigger_id' = $1")
        .bind(id).fetch_one(engine.pool()).await.unwrap();
    assert_eq!(count, 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn latest_slot(
        expression: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> chrono::DateTime<chrono::Utc> {
        let schedule = crate::scheduler::cron_rules::parse_standard_cron(expression).unwrap();
        schedule
            .after(&(now - chrono::Duration::hours(2)))
            .take_while(|slot| *slot <= now)
            .last()
            .expect("a slot inside two hours")
    }

    #[test]
    fn snapshot_fixture_cron_never_ticks_between_restore_and_assertion() {
        use chrono::TimeZone;
        let base = chrono::Utc.with_ymd_and_hms(2026, 11, 8, 9, 59, 0).unwrap();
        let assertion_delay = chrono::Duration::seconds(30);
        for offset_ms in (0..120_000).step_by(500) {
            let restored_at = base + chrono::Duration::milliseconds(offset_ms);
            let now = restored_at + assertion_delay;
            let slot = latest_slot(&pre_restore_hourly_cron(restored_at), now);
            assert!(
                slot < restored_at,
                "slot {slot} fires after restore {restored_at}"
            );
            assert!(
                now - slot < chrono::Duration::minutes(super::MISSED_TASK_GRACE_MINUTES),
                "slot {slot} is past catch-up grace at {now}"
            );
        }
        let restored_at = base + chrono::Duration::seconds(59);
        assert!(latest_slot("0 * * * * *", restored_at + assertion_delay) > restored_at);
    }

    async fn insert_observed_task(
        tracked: &Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
        task_name: &str,
    ) -> (uuid::Uuid, Arc<AtomicBool>) {
        let task_id = uuid::Uuid::new_v4();
        let cancel_token = CancellationToken::new();
        let observed = Arc::new(AtomicBool::new(false));

        let observed_clone = observed.clone();
        let token_clone = cancel_token.clone();
        let handle = tokio::spawn(async move {
            token_clone.cancelled().await;
            observed_clone.store(true, Ordering::SeqCst);
        });

        let mut tasks = tracked.write().await;
        tasks.insert(
            task_id,
            TrackedTask {
                handle,
                task_name: task_name.to_string(),
                cancel_token,
                schedule: vec![],
                timezone: String::new(),
            },
        );
        (task_id, observed)
    }

    #[tokio::test]
    async fn cancel_tracked_task_signals_cancel_and_removes_entry() {
        let tracked = Arc::new(RwLock::new(HashMap::new()));
        let (task_id, observed) = insert_observed_task(&tracked, "test-cancel").await;

        cancel_tracked_task(&tracked, task_id).await;

        tokio::time::timeout(std::time::Duration::from_millis(200), async {
            while !observed.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("task should observe cancel signal within 200ms");

        assert!(
            tracked.read().await.get(&task_id).is_none(),
            "tracked entry should be removed"
        );
    }

    #[tokio::test]
    async fn cancel_tracked_task_is_noop_for_unknown_id() {
        let tracked = Arc::new(RwLock::new(HashMap::new()));
        cancel_tracked_task(&tracked, uuid::Uuid::new_v4()).await;
        assert!(tracked.read().await.is_empty());
    }

    fn scheduled_trigger(schedule: &[&str], paused: bool) -> TriggerConfig {
        let mut config = TriggerConfig::from_created_payload(&serde_json::json!({
            "trigger_id": "t-1",
            "name": "Nightly",
            "schedule": schedule,
            "timezone": "UTC",
            "run": { "type": "intent", "intent": "go" },
        }))
        .expect("the payload parses");
        config.paused = paused;
        config
    }

    #[test]
    fn a_dead_schedule_is_dropped_rather_than_restarted() {
        let config = scheduled_trigger(&["0 0 9 31 2 *"], false);
        let problem = config.schedule_error().expect("February 31 never fires");
        let action = finished_runner_action(&config);
        assert_eq!(action, FinishedRunner::Drop(problem));
    }

    #[test]
    fn a_live_schedule_is_still_restarted() {
        assert_eq!(
            finished_runner_action(&scheduled_trigger(&["0 0 8 * * *"], false)),
            FinishedRunner::Restart
        );
    }

    #[test]
    fn a_paused_or_schedule_less_trigger_is_left_alone() {
        assert_eq!(
            finished_runner_action(&scheduled_trigger(&["0 0 8 * * *"], true)),
            FinishedRunner::Leave
        );
        assert_eq!(
            finished_runner_action(&scheduled_trigger(&[], false)),
            FinishedRunner::Leave
        );
    }

    async fn runner_on(schedule: &[&str], finished: bool) -> TrackedTask {
        let handle = if finished {
            let handle = tokio::spawn(async {});
            while !handle.is_finished() {
                tokio::task::yield_now().await;
            }
            handle
        } else {
            tokio::spawn(std::future::pending::<()>())
        };
        TrackedTask {
            handle,
            task_name: "Nightly".to_string(),
            cancel_token: CancellationToken::new(),
            schedule: schedule.iter().map(|s| s.to_string()).collect(),
            timezone: "UTC".to_string(),
        }
    }

    #[tokio::test]
    async fn a_live_trigger_with_no_runner_is_armed() {
        let config = scheduled_trigger(&["0 0 8 * * *"], false);
        assert_eq!(lag_repair(&config, None), LagRepair::Arm);
    }

    #[tokio::test]
    async fn a_runner_on_a_replaced_schedule_is_rearmed() {
        let config = scheduled_trigger(&["0 0 9 * * *"], false);
        let runner = runner_on(&["0 0 8 * * *"], false).await;
        assert_eq!(lag_repair(&config, Some(&runner)), LagRepair::Rearm);
        runner.handle.abort();
    }

    #[tokio::test]
    async fn a_runner_on_the_current_schedule_is_left_running() {
        let config = scheduled_trigger(&["0 0 8 * * *"], false);
        let runner = runner_on(&["0 0 8 * * *"], false).await;
        assert_eq!(lag_repair(&config, Some(&runner)), LagRepair::Leave);
        runner.handle.abort();
    }

    #[tokio::test]
    async fn the_repair_leaves_what_others_own() {
        let paused = scheduled_trigger(&["0 0 8 * * *"], true);
        assert_eq!(lag_repair(&paused, None), LagRepair::Leave);
        let dead = scheduled_trigger(&["0 0 9 31 2 *"], false);
        assert_eq!(lag_repair(&dead, None), LagRepair::Leave);
        let finished = runner_on(&["0 0 8 * * *"], true).await;
        let moved = scheduled_trigger(&["0 0 9 * * *"], false);
        assert_eq!(lag_repair(&moved, Some(&finished)), LagRepair::Leave);
    }

    #[tokio::test]
    async fn tracking_over_a_runner_cancels_it() {
        let mut tracked = HashMap::new();
        let id = uuid::Uuid::new_v4();
        let first = runner_on(&["0 0 8 * * *"], false).await;
        let first_token = first.cancel_token.clone();
        track(&mut tracked, id, first);
        track(&mut tracked, id, runner_on(&["0 0 8 * * *"], false).await);
        assert!(first_token.is_cancelled(), "the displaced runner must stop");
        for (_, task) in tracked.drain() {
            task.handle.abort();
        }
    }

    fn utc(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn history(last_run: Option<&str>) -> Result<crate::triggers::TriggerRunHistory, sqlx::Error> {
        Ok(crate::triggers::TriggerRunHistory {
            last_run: last_run.map(utc),
            created_at: Some(utc("2026-04-08T07:00:52Z")),
        })
    }

    const SLOT: &str = "2026-07-29T05:45:00Z";

    #[test]
    fn catch_up_skips_a_slot_that_already_ran_late() {
        assert_eq!(
            catch_up_decision(
                utc(SLOT),
                Some(utc("2026-07-29T05:49:15Z")),
                history(Some("2026-07-29T05:49:15Z")),
            ),
            CatchUp::Skip(SkipReason::AlreadyRan)
        );
    }

    #[test]
    fn catch_up_skips_when_only_the_event_store_knows_the_true_run_time() {
        assert_eq!(
            catch_up_decision(
                utc(SLOT),
                Some(utc("2026-07-29T05:44:35Z")),
                history(Some("2026-07-29T05:49:15Z")),
            ),
            CatchUp::Skip(SkipReason::AlreadyRan)
        );
    }

    #[test]
    fn catch_up_fires_a_genuinely_missed_slot() {
        assert_eq!(
            catch_up_decision(
                utc(SLOT),
                Some(utc("2026-07-28T05:45:08Z")),
                history(Some("2026-07-28T05:45:08Z")),
            ),
            CatchUp::Fire
        );
    }

    #[test]
    fn catch_up_fires_when_the_trigger_has_never_run() {
        assert_eq!(
            catch_up_decision(utc(SLOT), None, history(None)),
            CatchUp::Fire
        );
    }

    #[test]
    fn catch_up_skips_when_last_run_is_none_but_the_event_store_says_it_ran() {
        assert_eq!(
            catch_up_decision(utc(SLOT), None, history(Some("2026-07-29T05:49:15Z"))),
            CatchUp::Skip(SkipReason::AlreadyRan)
        );
    }

    #[test]
    fn catch_up_skips_when_the_run_history_is_unavailable() {
        assert_eq!(
            catch_up_decision(utc(SLOT), None, Err(sqlx::Error::PoolClosed)),
            CatchUp::Skip(SkipReason::HistoryUnavailable)
        );
        assert_eq!(
            catch_up_decision(
                utc(SLOT),
                Some(utc("2026-07-28T05:45:08Z")),
                Err(sqlx::Error::PoolClosed),
            ),
            CatchUp::Skip(SkipReason::HistoryUnavailable)
        );
    }

    #[test]
    fn catch_up_skips_a_slot_older_than_the_trigger_itself() {
        assert_eq!(
            catch_up_decision(
                utc(SLOT),
                None,
                Ok(crate::triggers::TriggerRunHistory {
                    last_run: None,
                    created_at: Some(utc("2026-07-29T05:50:00Z")),
                }),
            ),
            CatchUp::Skip(SkipReason::SlotPredatesTrigger)
        );
        assert_eq!(
            catch_up_decision(
                utc(SLOT),
                None,
                Ok(crate::triggers::TriggerRunHistory {
                    last_run: None,
                    created_at: Some(utc("2026-07-29T05:40:00Z")),
                }),
            ),
            CatchUp::Fire
        );
    }

    #[test]
    fn catch_up_fires_when_the_creation_time_is_unknown_but_no_run_is_recorded() {
        assert_eq!(
            catch_up_decision(
                utc(SLOT),
                None,
                Ok(crate::triggers::TriggerRunHistory {
                    last_run: None,
                    created_at: None,
                }),
            ),
            CatchUp::Fire
        );
    }

    #[test]
    fn a_cron_fire_due_after_shutdown_began_is_not_started() {
        assert_eq!(
            scheduled_fire_refusal(true, Some(false)),
            Some(FireRefusal::ShuttingDown)
        );
    }

    #[test]
    fn shutdown_is_named_over_a_pause_or_a_delete() {
        assert_eq!(
            scheduled_fire_refusal(true, Some(true)),
            Some(FireRefusal::ShuttingDown)
        );
        assert_eq!(
            scheduled_fire_refusal(true, None),
            Some(FireRefusal::ShuttingDown)
        );
    }

    #[test]
    fn a_live_trigger_on_a_running_engine_fires() {
        assert_eq!(scheduled_fire_refusal(false, Some(false)), None);
        assert_eq!(
            scheduled_fire_refusal(false, Some(true)),
            Some(FireRefusal::Paused)
        );
        assert_eq!(
            scheduled_fire_refusal(false, None),
            Some(FireRefusal::Deleted)
        );
    }

    async fn seed_trigger_that_ran_yesterday(pool: &sqlx::PgPool, trigger_id: &str) {
        use crate::test_support::seed_trigger_event;
        seed_trigger_event(
            pool,
            trigger_id,
            "TriggerCreated",
            "2026-10-01T09:00:00Z",
            serde_json::json!({ "trigger_id": trigger_id, "name": "Thread cost snapshot" }),
        )
        .await;
        seed_trigger_event(
            pool,
            trigger_id,
            "TriggerExecuted",
            "2026-10-10T16:10:04Z",
            serde_json::json!({
                "trigger_id": trigger_id,
                "last_run": "2026-10-10T16:10:04Z",
                "status": "ok",
            }),
        )
        .await;
    }

    async fn next_engine_decision(
        pool: &sqlx::PgPool,
        trigger_id: &str,
        slot: chrono::DateTime<chrono::Utc>,
    ) -> CatchUp {
        let history = crate::triggers::load_trigger_run_history(pool, trigger_id).await;
        let replayed_last_run = history.as_ref().ok().and_then(|h| h.last_run);
        catch_up_decision(slot, replayed_last_run, history)
    }

    #[tokio::test]
    async fn the_next_engine_catches_up_only_the_slot_shutdown_refused() {
        use crate::test_support::{seed_trigger_event, setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let slot = utc("2026-10-11T16:10:00Z");

        let refused = "66666666-6666-6666-6666-666666666666";
        seed_trigger_that_ran_yesterday(&pool, refused).await;
        assert_eq!(
            scheduled_fire_refusal(true, Some(false)),
            Some(FireRefusal::ShuttingDown)
        );
        assert_eq!(
            next_engine_decision(&pool, refused, slot).await,
            CatchUp::Fire,
            "a slot the shutdown refused is caught up by the next engine"
        );

        for (trigger_id, status) in [
            ("77777777-7777-7777-7777-777777777777", "ok"),
            ("88888888-8888-8888-8888-888888888888", "failed"),
        ] {
            seed_trigger_that_ran_yesterday(&pool, trigger_id).await;
            seed_trigger_event(
                &pool,
                trigger_id,
                "TriggerExecuted",
                "2026-10-11T16:10:09Z",
                serde_json::json!({
                    "trigger_id": trigger_id,
                    "last_run": "2026-10-11T16:10:09Z",
                    "status": status,
                }),
            )
            .await;
            assert_eq!(
                next_engine_decision(&pool, trigger_id, slot).await,
                CatchUp::Skip(SkipReason::AlreadyRan),
                "a slot that ran {status} on a healthy engine is not fired again"
            );
        }

        teardown_test_db(&db).await;
    }

    const CEILING: u32 = crate::engine::thread_queue::DEFAULT_MAX_EVENT_TRIGGER_DEPTH;

    #[test]
    fn skip_reason_shutting_down_takes_precedence() {
        assert_eq!(
            event_trigger_skip_reason(true, 0, CEILING),
            Some(EventTriggerSkip::ShuttingDown)
        );
        assert_eq!(
            event_trigger_skip_reason(true, CEILING + 5, CEILING),
            Some(EventTriggerSkip::ShuttingDown),
            "shutdown wins over the depth cap so the log names the real reason"
        );
    }

    #[tokio::test]
    async fn a_chain_across_triggers_stops_at_the_depth_cap() {
        use crate::scheduler::user_tasks::{current_event_trigger_depth, EVENT_TRIGGER_DEPTH};

        let mut observed_depth = current_event_trigger_depth();
        let mut fires = 0u32;

        while event_trigger_skip_reason(false, observed_depth, CEILING).is_none() {
            fires += 1;
            assert!(
                fires <= CEILING + 1,
                "the chain must terminate, not run away"
            );
            let next_depth = observed_depth + 1;
            observed_depth = EVENT_TRIGGER_DEPTH
                .scope(next_depth, async { current_event_trigger_depth() })
                .await;
        }

        assert_eq!(
            fires, CEILING,
            "a chain across triggers gets {CEILING} fires and then stops"
        );
    }

    #[test]
    fn skip_reason_depth_cap_when_not_shutting_down() {
        assert_eq!(
            event_trigger_skip_reason(false, CEILING, CEILING),
            Some(EventTriggerSkip::MaxDepth)
        );
        assert_eq!(
            event_trigger_skip_reason(false, CEILING - 1, CEILING),
            None,
            "below the cap and not shutting down → dispatch proceeds"
        );
        assert_eq!(
            event_trigger_skip_reason(false, 0, CEILING),
            None,
            "normal operation → dispatch proceeds"
        );
    }

    #[test]
    fn a_raised_ceiling_admits_a_fire_the_default_would_cap() {
        assert_eq!(
            event_trigger_skip_reason(false, CEILING, CEILING),
            Some(EventTriggerSkip::MaxDepth)
        );
        assert_eq!(
            event_trigger_skip_reason(false, CEILING, CEILING + 2),
            None,
            "the same event, under a policy that allows a longer chain"
        );
    }

    #[tokio::test]
    async fn a_chain_through_a_spawn_stops_at_the_depth_cap() {
        use crate::scheduler::user_tasks::{current_event_trigger_depth, EVENT_TRIGGER_DEPTH};

        let mut observed_depth = current_event_trigger_depth();
        let mut fires = 0u32;

        while event_trigger_skip_reason(false, observed_depth, CEILING).is_none() {
            fires += 1;
            assert!(
                fires <= CEILING + 1,
                "the chain must terminate even when every hop crosses a spawn"
            );
            let carried = observed_depth + 1;
            observed_depth = tokio::spawn(
                EVENT_TRIGGER_DEPTH.scope(carried, async { current_event_trigger_depth() }),
            )
            .await
            .expect("the probe task runs");
        }

        assert_eq!(
            fires, CEILING,
            "a loop through spawned work gets {CEILING} fires and then stops"
        );
    }

    #[tokio::test]
    async fn a_trigger_waiting_on_the_coding_agent_it_started_still_fires() {
        use crate::scheduler::user_tasks::{current_event_trigger_depth, EVENT_TRIGGER_DEPTH};

        let session_depth =
            tokio::spawn(EVENT_TRIGGER_DEPTH.scope(1, async { current_event_trigger_depth() }))
                .await
                .expect("the session task runs");
        assert_eq!(session_depth, 1, "the session inherits the fire's depth");

        assert_eq!(
            event_trigger_skip_reason(false, session_depth, CEILING),
            None,
            "the trigger waiting on its own coding agent must still fire"
        );

        assert_eq!(
            event_trigger_skip_reason(false, session_depth + 1, CEILING),
            None
        );
    }

    #[test]
    fn a_three_hop_release_chain_fires_every_link_under_the_default() {
        for emitting_depth in 0..3 {
            assert_eq!(
                event_trigger_skip_reason(false, emitting_depth, CEILING),
                None,
                "hop {} of the release chain must fire",
                emitting_depth + 1
            );
        }
    }

    #[test]
    fn the_cap_report_is_never_reported_for_its_own_carrier() {
        assert_eq!(
            CAP_REPORT_EVENT_TYPE, "NotificationCreated",
            "the guard has to name the event `report_depth_cap` actually emits"
        );
    }

    #[test]
    fn the_cap_notification_cooldown_suppresses_a_repeat() {
        let trigger = format!("cooldown-probe-{}", uuid::Uuid::new_v4());
        assert!(
            cap_notification_due(&trigger),
            "the first suppression for a trigger is always reported"
        );
        assert!(
            !cap_notification_due(&trigger),
            "an immediate repeat is inside the cooldown"
        );
    }

    #[tokio::test]
    async fn backdated_cut_failure_skips_its_slot_and_preserves_a_later_missed_slot() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let slot = utc("2026-10-11T16:10:00Z");
        seed_trigger_that_ran_yesterday(&pool, "cut-fire").await;
        bus.record_trigger_executed_at("cut-fire", crate::triggers::TriggerRunStatus::Failed, slot)
            .await
            .unwrap();
        assert_eq!(
            next_engine_decision(&pool, "cut-fire", slot).await,
            CatchUp::Skip(SkipReason::AlreadyRan)
        );
        assert_eq!(
            next_engine_decision(&pool, "cut-fire", slot + chrono::Duration::minutes(15)).await,
            CatchUp::Fire
        );
        pool.close().await;
        teardown_test_db(&db).await;
    }

    struct ParkedRunExecutor {
        gate: tokio::sync::Semaphore,
        entered: tokio::sync::mpsc::UnboundedSender<()>,
        finished: tokio::sync::mpsc::UnboundedSender<()>,
    }

    #[async_trait::async_trait]
    impl crate::engine::thread_queue::ThreadQueueExecutor for ParkedRunExecutor {
        async fn execute(
            &self,
            _entry: crate::engine::thread_queue::ExecutableEntry,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let _ = self.entered.send(());
            self.gate.acquire().await.expect("gate open").forget();
            let _ = self.finished.send(());
            Ok(())
        }
    }

    #[tokio::test]
    async fn stopping_runners_drops_a_queued_fire_and_leaves_an_admitted_run_alone() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let workspace = tempfile::tempdir().expect("temp workspace");
        let (entered_tx, mut entered) = tokio::sync::mpsc::unbounded_channel();
        let (finished_tx, mut finished) = tokio::sync::mpsc::unbounded_channel();
        let executor = Arc::new(ParkedRunExecutor {
            gate: tokio::sync::Semaphore::new(0),
            entered: entered_tx,
            finished: finished_tx,
        });
        let queue = Arc::new(crate::engine::thread_queue::ThreadQueue::new(
            pool.clone(),
            bus,
            Arc::new(std::sync::RwLock::new(HashMap::new())),
            workspace.path().to_path_buf(),
            Arc::new(tokio::sync::Mutex::new(())),
            crate::engine::thread_queue::CapacityPolicy {
                max_concurrent_total: 1,
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        ));
        queue.set_executor(executor.clone());

        let runner_token = CancellationToken::new();
        let (slot_tx, slot) = tokio::sync::oneshot::channel();
        let slot_handle = tokio::spawn({
            let (queue, token) = (queue.clone(), runner_token.clone());
            async move {
                slot_tx
                    .send(fire_scheduled_slot(&queue, "save-mid-run", "Save mid run", &token).await)
                    .unwrap();
            }
        });
        entered.recv().await.expect("the run was admitted");

        let queued_token = CancellationToken::new();
        let (queued_tx, queued_slot) = tokio::sync::oneshot::channel();
        let queued_handle = tokio::spawn({
            let (queue, token) = (queue.clone(), queued_token.clone());
            async move {
                queued_tx
                    .send(fire_scheduled_slot(&queue, "queued-save", "Queued save", &token).await)
                    .unwrap();
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if queue.snapshot().await.unwrap().entries.iter().any(|entry| {
                    entry.trigger_id.as_deref() == Some("queued-save") && entry.status == "queued"
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the second fire must be queued before stopping its runner");

        let tracked = Arc::new(RwLock::new(HashMap::new()));
        for (token, slot_handle) in [(runner_token, slot_handle), (queued_token, queued_handle)] {
            let task_id = uuid::Uuid::new_v4();
            tracked.write().await.insert(
                task_id,
                TrackedTask {
                    handle: slot_handle,
                    task_name: "Saved trigger".to_string(),
                    cancel_token: token,
                    schedule: vec![],
                    timezone: "UTC".to_string(),
                },
            );
            cancel_tracked_task(&tracked, task_id).await;
        }
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), queued_slot)
                .await
                .expect("the queued runner stops promptly")
                .unwrap(),
            SlotEnd::RunnerStopped
        );
        assert!(
            queue
                .snapshot()
                .await
                .unwrap()
                .entries
                .iter()
                .all(|entry| { entry.trigger_id.as_deref() != Some("queued-save") }),
            "stopping the runner must drop its queued scheduled fire"
        );

        let slot_end = tokio::time::timeout(std::time::Duration::from_secs(5), slot)
            .await
            .expect("the runner stops waiting promptly")
            .unwrap();
        assert_eq!(slot_end, SlotEnd::RunnerStopped);

        assert!(
            finished.try_recv().is_err(),
            "the run is still in flight after its runner stopped"
        );
        executor.gate.add_permits(1);
        finished.recv().await.expect("the run finished");

        pool.close().await;
        teardown_test_db(&db).await;
    }
}
