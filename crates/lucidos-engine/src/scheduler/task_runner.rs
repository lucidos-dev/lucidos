//! Per-trigger task runner: spawns a tokio task that wakes on cron expressions
//! and invokes the agentic loop. Lifecycle helpers (`cancel`/`detach`/health
//! check / event-driven registration) live here too so the parent
//! `SchedulerManager` only has to forward to them.

use crate::api::SharedEngine;
use crate::triggers::TriggerConfig;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{trigger_id_to_uuid, MISSED_TASK_GRACE_MINUTES};

/// Information about a tracked task.
///
/// `cancel_token` is observed by the task runner (between scheduled executions)
/// and by the agentic loop (between iterations). Signaling cancel lets the task
/// finish its current operation cleanly and emit terminal events; callers must
/// not abort the `JoinHandle` directly, or the thread is left without a
/// `ResponseGenerated`/`ResponseCanceled` event and shows as stuck "running".
pub(super) struct TrackedTask {
    pub(super) handle: JoinHandle<()>,
    pub(super) task_name: String,
    pub(super) cancel_token: CancellationToken,
}

/// Spawn a task runner that executes on schedule.
///
/// Returns the `JoinHandle` (for liveness checks) and a `CancellationToken` the
/// task observes between executions and inside the agentic loop. Cancel the
/// token instead of aborting the handle to let the task emit its terminal
/// events before exiting.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_task_runner(
    trigger_id: String,
    task_name: String,
    cron_expressions: Vec<String>,
    timezone: String,
    engine: SharedEngine,
    shutdown_flag: Arc<AtomicBool>,
    trigger_configs: Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
) -> (JoinHandle<()>, CancellationToken) {
    let cancel_token = CancellationToken::new();
    let task_cancel = cancel_token.clone();
    let handle = tokio::spawn(async move {
        // Wrap the entire task in a panic catcher
        let result = run_task_loop(
            trigger_id,
            task_name.clone(),
            cron_expressions,
            timezone,
            engine,
            shutdown_flag,
            trigger_configs,
            task_cancel,
        )
        .await;

        match result {
            Ok(reason) => {
                log!("[Scheduler] Task '{}' exited: {}", task_name, reason);
            }
            Err(e) => {
                log!("[Scheduler] Task '{}' crashed: {}", task_name, e);
            }
        }
    });
    (handle, cancel_token)
}

/// The main task loop - runs until task is deleted/disabled, shutdown is requested, or an error occurs
#[allow(clippy::too_many_arguments)]
async fn run_task_loop(
    trigger_id: String,
    task_name: String,
    cron_expressions: Vec<String>,
    timezone: String,
    engine: SharedEngine,
    shutdown_flag: Arc<AtomicBool>,
    trigger_configs: Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
    cancel_token: CancellationToken,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use crate::engine::tools::scheduler::next_occurrence_multi;
    // Parse all cron expressions (translate standard dow to cron-crate convention)
    let schedules: Vec<cron::Schedule> = cron_expressions
        .iter()
        .map(|expr| {
            crate::engine::tools::scheduler::parse_standard_cron(expr).map_err(|e| {
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

    // Parse timezone
    let tz: chrono_tz::Tz = timezone.parse().unwrap_or_else(|_| {
        log!(
            "[Scheduler] Invalid timezone '{}' for task {}, using UTC",
            timezone,
            task_name
        );
        chrono_tz::UTC
    });

    // Check if we just missed a scheduled time (grace period)
    check_and_execute_missed(
        &schedules,
        tz,
        &trigger_id,
        &task_name,
        &engine,
        &trigger_configs,
    )
    .await?;

    // Main scheduling loop — exits when shutdown is signaled (between executions, not mid-execution)
    loop {
        if shutdown_flag.load(Ordering::Relaxed) {
            return Ok("shutdown requested".to_string());
        }
        if cancel_token.is_cancelled() {
            return Ok("cancelled".to_string());
        }
        // Calculate next occurrence across all schedules
        let next: chrono::DateTime<chrono_tz::Tz> = match next_occurrence_multi(&schedules, tz) {
            Some(t) => t,
            None => {
                return Ok("no more occurrences".to_string());
            }
        };

        let next_utc = next.with_timezone(&chrono::Utc);
        let now_utc = chrono::Utc::now();

        // Log when waiting for long periods
        if next_utc > now_utc {
            let wait_secs = (next_utc - now_utc).num_seconds();
            if wait_secs > 3600 {
                log!(
                    "[Scheduler] Task '{}' waiting until {} ({:.1} hours)",
                    task_name,
                    next.format("%Y-%m-%d %H:%M:%S %Z"),
                    wait_secs as f64 / 3600.0
                );
            }
        }

        // Poll with short sleeps until the scheduled time arrives.
        // This ensures we wake up promptly after macOS system sleep,
        // where monotonic timers (tokio::time::sleep) don't advance.
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
        loop {
            if shutdown_flag.load(Ordering::Relaxed) {
                return Ok("shutdown requested".to_string());
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

        // Read fresh state (event-sourced) — exit the loop on pause/delete.
        // The Thread Queue executor re-reads the full config at run time.
        let paused = {
            let configs = trigger_configs.read().unwrap();
            configs.get(&trigger_id).map(|c| c.paused)
        };
        match paused {
            Some(false) => {}
            Some(true) => return Ok("trigger paused".to_string()),
            None => return Ok("trigger deleted".to_string()),
        }

        // Validate we're not too late (past grace window)
        let actual_now = chrono::Utc::now();
        let delay = actual_now - next_utc;
        if delay > chrono::Duration::minutes(MISSED_TASK_GRACE_MINUTES) {
            log!("[Scheduler] Task '{}' woke up {} minutes late (scheduled {}, actual {}), skipping this occurrence",
                task_name,
                delay.num_minutes(),
                next.format("%H:%M:%S"),
                actual_now.format("%H:%M:%S")
            );
            // Don't execute, wait for next occurrence
            continue;
        }

        // Log execution timing
        if delay.num_seconds() > 5 {
            log!(
                "[Scheduler] Task '{}' executing {}s after scheduled time",
                task_name,
                delay.num_seconds()
            );
        }

        // Execute through the Thread Queue: admission control may hold the
        // fire until capacity frees. Awaiting completion preserves the
        // loop's semantics (next occurrence computed after the run ends) —
        // saturation back-pressures this trigger's schedule instead of
        // stacking unbounded concurrent runs. Execution bookkeeping
        // (ACTIVE_TASK_COUNT, record_trigger_executed, failure logging)
        // lives in the queue executor.
        let outcome = engine
            .thread_queue
            .submit(
                crate::engine::thread_queue::ThreadQueueRequest::Cron {
                    trigger_id: trigger_id.clone(),
                },
                None,
                Some(cancel_token.clone()),
            )
            .await;
        if !outcome.admitted {
            log!(
                "[Scheduler] Task '{}' queued at position {} (system at capacity)",
                task_name,
                outcome.position
            );
        }
        tokio::select! {
            _ = outcome.completion => {}
            _ = cancel_token.cancelled() => {
                // Trigger updated/deleted mid-wait. A still-queued entry is
                // dropped here; an admitted run observes the token itself
                // (execute_user_task carries it), so a drop failure just
                // means the run is already in flight.
                if let Err(e) = engine
                    .thread_queue
                    .drop_entry(outcome.entry_id, "trigger cancelled", None)
                    .await
                {
                    log!(
                        "[Scheduler] Task '{}' cancelled while running: {}",
                        task_name,
                        e
                    );
                }
                return Ok("cancelled".to_string());
            }
        }
    }
}

/// Whether the startup catch-up should run a missed *cron slot*, and if not, why.
///
/// Split out from [`check_and_execute_missed`] so the decision is unit-testable
/// without a scheduler, an engine, or a database — the 2026-07-29 double-fire
/// was a one-line comparison nobody could assert on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatchUp {
    /// The slot is genuinely due: the trigger existed before it and no run is
    /// recorded at or after it.
    Fire,
    Skip(SkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipReason {
    /// A run is recorded at or after the slot. A late fire counts: the 07:49 run
    /// of an 07:45 slot means the 07:45 slot has run.
    AlreadyRan,
    /// The slot is older than the trigger itself — a trigger created at 07:50
    /// must not immediately "catch up" the 07:45 slot it never existed for.
    SlotPredatesTrigger,
    /// The event store could not be read. Fail closed: we cannot prove the slot
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

/// Decide whether a missed cron slot is still due.
///
/// Fails closed by construction: the only path to [`CatchUp::Fire`] is a
/// positive showing that the slot is due. A `None` `last_run` is *not* evidence
/// of "never ran" — before this function was extracted the guard was
/// `if let Some(last_run)`, so an absent value skipped the check entirely and
/// fired.
///
/// `in_memory_last_run` is the replayed [`TriggerConfig::last_run`] and
/// `history` is an independent read of the same durable truth. Both are
/// engine-clock *recorded run times*; the catch-up takes the later of the two so
/// neither a stale config nor a lagging read can authorize a re-fire.
fn catch_up_decision(
    slot_utc: chrono::DateTime<chrono::Utc>,
    in_memory_last_run: Option<chrono::DateTime<chrono::Utc>>,
    history: Result<crate::triggers::TriggerRunHistory, sqlx::Error>,
) -> CatchUp {
    let Ok(history) = history else {
        return CatchUp::Skip(SkipReason::HistoryUnavailable);
    };

    let effective_last_run = match (in_memory_last_run, history.last_run) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };

    if effective_last_run.is_some_and(|last_run| last_run >= slot_utc) {
        return CatchUp::Skip(SkipReason::AlreadyRan);
    }
    if history.created_at.is_some_and(|created| created > slot_utc) {
        return CatchUp::Skip(SkipReason::SlotPredatesTrigger);
    }
    CatchUp::Fire
}

/// Check if we just missed a scheduled time and execute if within grace period
async fn check_and_execute_missed(
    schedules: &[cron::Schedule],
    tz: chrono_tz::Tz,
    trigger_id: &str,
    task_name: &str,
    engine: &SharedEngine,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let now_in_tz = chrono::Utc::now().with_timezone(&tz);
    let grace_period = chrono::Duration::minutes(MISSED_TASK_GRACE_MINUTES);

    // Check all schedules for missed occurrences, find the most recent missed one
    let mut best_missed: Option<chrono::DateTime<chrono_tz::Tz>> = None;

    for schedule in schedules {
        for occurrence in schedule.after(&(now_in_tz - grace_period)).take(3) {
            let occurrence_utc = occurrence.with_timezone(&chrono::Utc);
            let now_utc = chrono::Utc::now();

            if occurrence_utc < now_utc {
                let delay = now_utc - occurrence_utc;
                if delay < grace_period {
                    // This is a valid missed occurrence — keep the most recent
                    if best_missed.is_none_or(|b| occurrence > b) {
                        best_missed = Some(occurrence);
                    }
                }
            }
        }
    }

    if let Some(missed) = best_missed {
        let missed_utc = missed.with_timezone(&chrono::Utc);
        let now_utc = chrono::Utc::now();
        let delay = now_utc - missed_utc;

        // Read config from in-memory state
        let config = {
            let configs = trigger_configs.read().unwrap();
            configs.get(trigger_id).cloned()
        };
        let config = match config {
            Some(c) if !c.paused => c,
            _ => return Ok(()), // Trigger deleted or paused
        };

        // Guard: only fire a slot we can positively show is still due. Reached
        // only once a missed slot has actually been found, but every path that
        // registers a task runner — engine start, trigger create, trigger
        // update/enable, and the health monitor's crash restart — funnels its
        // catch-up through here, so this is the single choke point for all of
        // them.
        let history = crate::triggers::load_trigger_run_history(engine.pool(), trigger_id).await;
        if let Err(e) = &history {
            log!(
                "[Scheduler] Task '{}' run-history lookup failed: {}",
                task_name,
                e
            );
        }
        if let CatchUp::Skip(reason) = catch_up_decision(missed_utc, config.last_run, history) {
            log!(
                "[Scheduler] Task '{}' missed at {} — not executing: {}",
                task_name,
                missed.format("%H:%M:%S"),
                reason.as_str()
            );
            return Ok(());
        }

        log!(
            "[Scheduler] Task '{}' missed at {} ({}s ago), executing now",
            task_name,
            missed.format("%H:%M:%S"),
            delay.num_seconds()
        );

        // Same Thread Queue routing as the on-time path; the executor owns
        // record_trigger_executed and failure logging. No cancel token —
        // matches the pre-queue missed-grace call.
        let outcome = engine
            .thread_queue
            .submit(
                crate::engine::thread_queue::ThreadQueueRequest::Cron {
                    trigger_id: trigger_id.to_string(),
                },
                None,
                None,
            )
            .await;
        if outcome.completion.await.is_err() {
            log!(
                "[Scheduler] Task '{}' grace-period completion channel dropped",
                task_name
            );
        }
    }

    Ok(())
}

/// React to a trigger lifecycle event: arm or disarm its cron job.
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
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_trigger_event(
    event_type: &str,
    trigger_id: &str,
    payload: &serde_json::Value,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
    tracked_tasks: &Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    engine: &SharedEngine,
    shutdown_flag: &Arc<AtomicBool>,
) {
    // `TriggerExecuted` reaches this function too (the subscriber routes the
    // whole trigger family here) and does no work below, so it must not pay for
    // the lock on every single trigger run.
    if !crate::triggers::registry::TRIGGER_LIFECYCLE_EVENTS.contains(&event_type) {
        return;
    }
    let task_uuid = trigger_id_to_uuid(trigger_id);
    // Guard dropped before any side effect below: `thread_queue.drain()` can
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
                    register_and_track(
                        config,
                        tracked_tasks,
                        engine,
                        shutdown_flag,
                        trigger_configs,
                    )
                    .await;
                    crate::log!(
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
                    register_and_track(
                        config,
                        tracked_tasks,
                        engine,
                        shutdown_flag,
                        trigger_configs,
                    )
                    .await;
                }
                crate::log!("[Scheduler] Updated trigger: {}", trigger_id);
                // An update can unpause — queued fires for this trigger may
                // be eligible again.
                engine.thread_queue.drain().await;
            }
        }
        "TriggerDeleted" => {
            // Unconditional: the chokepoint may already have removed the
            // registry entry, but the cron job is still armed until we say so.
            let self_deleting = payload
                .get("self_deleting")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if self_deleting {
                detach_tracked_task(tracked_tasks, task_uuid).await;
                crate::log!(
                    "[Scheduler] Deleted trigger: {} (self-delete, task left running)",
                    trigger_id
                );
            } else {
                cancel_tracked_task(tracked_tasks, task_uuid).await;
                crate::log!("[Scheduler] Deleted trigger: {}", trigger_id);
            }
        }
        "TriggerEnabled" => {
            if let Some(config) = config.as_ref() {
                if !config.schedule.is_empty() {
                    register_and_track(
                        config,
                        tracked_tasks,
                        engine,
                        shutdown_flag,
                        trigger_configs,
                    )
                    .await;
                }
                crate::log!("[Scheduler] Resumed trigger: {}", trigger_id);
                // Resume unblocks the trigger's queued fires — drain so they
                // admit without waiting for the periodic sweep.
                engine.thread_queue.drain().await;
            }
        }
        "TriggerDisabled" => {
            // Unconditional for the same reason as delete: disarming the job is
            // this function's job whether or not the flag was already flipped.
            cancel_tracked_task(tracked_tasks, task_uuid).await;
            crate::log!("[Scheduler] Paused trigger: {}", trigger_id);
        }
        _ => {}
    }
}

/// Spawn a task runner for a trigger config and track its handle.
async fn register_and_track(
    config: &TriggerConfig,
    tracked_tasks: &Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    engine: &SharedEngine,
    shutdown_flag: &Arc<AtomicBool>,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
) {
    let task_uuid = trigger_id_to_uuid(&config.id);
    let (handle, cancel_token) = spawn_task_runner(
        config.id.clone(),
        config.name.clone(),
        config.schedule.clone(),
        config.timezone.clone(),
        engine.clone(),
        shutdown_flag.clone(),
        trigger_configs.clone(),
    );
    let mut tracked = tracked_tasks.write().await;
    tracked.insert(
        task_uuid,
        TrackedTask {
            handle,
            task_name: config.name.clone(),
            cancel_token,
        },
    );
}

/// Maximum depth for event-triggered chains (A→B→A…). Beyond this, events
/// are still stored but won't fire additional triggers.
const MAX_EVENT_TRIGGER_DEPTH: u32 = 3;

/// Why an incoming event must not fan out to event-triggers. Returned by
/// [`event_trigger_skip_reason`] so the decision is unit-testable without a
/// full engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventTriggerSkip {
    /// The engine is mid shutdown/restart. The terminator events emitted during
    /// cleanup (`ResponseAborted{EngineShutdown}`, `CodingAgentIdled`,
    /// `SessionEnded`) reach this dispatcher, but a trigger script's
    /// `lucidos ...` callback would hit the HTTP API being torn down →
    /// connection-refused → the script dies → spurious "<trigger> failed" push.
    ShuttingDown,
    /// Recursion cap for event chains (A→B→A…). The event is still stored, it
    /// just won't fire further triggers.
    MaxDepth,
}

/// Decide whether an event at `depth` should skip event-trigger dispatch.
/// Shutdown takes precedence over the depth cap so the log reflects the real
/// reason during a restart.
fn event_trigger_skip_reason(is_shutting_down: bool, depth: u32) -> Option<EventTriggerSkip> {
    if is_shutting_down {
        Some(EventTriggerSkip::ShuttingDown)
    } else if depth >= MAX_EVENT_TRIGGER_DEPTH {
        Some(EventTriggerSkip::MaxDepth)
    } else {
        None
    }
}

/// Handle a domain event from the EventBus — fire matching event-based triggers.
///
/// `origin_thread_id` is the thread the firing event lives in (only set for
/// thread-scoped events like `UserQuestionAsked`). It propagates via a
/// task-local so `send_notification` can deep-link the resulting push back to
/// the originating conversation instead of the trigger LLM's own thread.
///
/// `source_event_id` is the UUID of the event row that fired the trigger
/// (used by the popover panel to deep-link to the event).
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_domain_event(
    event_type: &str,
    payload: &serde_json::Value,
    depth: u32,
    origin_thread_id: Option<uuid::Uuid>,
    source_event_id: Option<uuid::Uuid>,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
    engine: &SharedEngine,
) {
    match event_trigger_skip_reason(engine.is_shutting_down(), depth) {
        Some(EventTriggerSkip::ShuttingDown) => {
            crate::log!(
                "[Scheduler] Engine shutting down — not firing triggers for event '{}'",
                event_type
            );
            return;
        }
        Some(EventTriggerSkip::MaxDepth) => {
            crate::log!(
                "[Scheduler] Event '{}' at depth {} — skipping triggers to prevent recursion",
                event_type,
                depth
            );
            return;
        }
        None => {}
    }

    let matching = {
        let configs = trigger_configs.read().unwrap();
        crate::triggers::find_matching_event_triggers(&configs, event_type, payload)
    };

    if matching.is_empty() {
        return;
    }

    crate::log!(
        "[Scheduler] Event '{}' matched {} trigger(s)",
        event_type,
        matching.len()
    );

    // Route every fire through the Thread Queue: over capacity the fire is
    // enqueued (FIFO per trigger) instead of spawning unbounded concurrent
    // executions. Execution itself — task-local scoping, ACTIVE_TASK_COUNT,
    // record_trigger_executed, failure logging — lives in the queue
    // executor (`engine::thread_queue::executor`).
    let next_depth = depth + 1;
    for config in matching {
        let outcome = engine
            .thread_queue
            .submit(
                crate::engine::thread_queue::ThreadQueueRequest::EventTrigger {
                    trigger_id: config.id.clone(),
                    event_type: event_type.to_string(),
                    event_payload: payload.clone(),
                    depth: next_depth,
                    origin_thread_id,
                    source_event_id,
                },
                None,
                None,
            )
            .await;
        if !outcome.admitted {
            crate::log!(
                "[Scheduler] Event trigger '{}' queued at position {} (system at capacity)",
                config.name,
                outcome.position
            );
        }
    }
}

/// Signal a tracked task to exit cooperatively, then drop its handle.
///
/// Aborting the `JoinHandle` would tear the agentic loop down mid-tool, leaving
/// the thread without a `ResponseGenerated`/`ResponseCanceled` event so it
/// shows as stuck "running" until engine restart. Cancelling the token instead
/// lets the loop exit cleanly between iterations and emit its terminal event.
async fn cancel_tracked_task(
    tracked_tasks: &Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    task_id: uuid::Uuid,
) {
    let mut tracked = tracked_tasks.write().await;
    if let Some(task) = tracked.remove(&task_id) {
        task.cancel_token.cancel();
    }
}

/// Drop the tracked entry without cancelling. Used for self-deletion: the
/// trigger's own LLM has called `delete_trigger` and is mid-flight; cancelling
/// would interrupt the in-progress tool call. The task's natural loop end
/// (where it re-reads the config and finds it gone) will exit it cleanly.
async fn detach_tracked_task(
    tracked_tasks: &Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    task_id: uuid::Uuid,
) {
    let mut tracked = tracked_tasks.write().await;
    tracked.remove(&task_id);
}

/// Check health of tracked tasks and restart any that have crashed
pub(super) async fn check_task_health_and_restart(
    tracked: Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    engine: SharedEngine,
    shutdown_flag: Arc<AtomicBool>,
    trigger_configs: Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
) {
    if shutdown_flag.load(Ordering::Relaxed) {
        return;
    }

    let mut to_restart: Vec<(uuid::Uuid, String, String, Vec<String>, String)> = Vec::new();

    // Check which tasks have finished (crashed or exited)
    {
        let tracked_read = tracked.read().await;
        let configs = trigger_configs.read().unwrap();
        for (task_id, task_info) in tracked_read.iter() {
            if task_info.handle.is_finished() {
                // Find matching config by deriving UUID from trigger_id
                let matching_config = configs
                    .values()
                    .find(|c| trigger_id_to_uuid(&c.id) == *task_id);
                if let Some(config) = matching_config {
                    if !config.paused && !config.schedule.is_empty() {
                        log!(
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
                }
            }
        }
    }

    // Restart crashed tasks
    for (task_id, trigger_id, task_name, schedule, timezone) in to_restart {
        // Remove old entry
        {
            let mut tracked_write = tracked.write().await;
            tracked_write.remove(&task_id);
        }

        // Spawn new task runner
        let (handle, cancel_token) = spawn_task_runner(
            trigger_id,
            task_name.clone(),
            schedule,
            timezone,
            engine.clone(),
            shutdown_flag.clone(),
            trigger_configs.clone(),
        );

        // Track the new handle
        {
            let mut tracked_write = tracked.write().await;
            tracked_write.insert(
                task_id,
                TrackedTask {
                    handle,
                    task_name: task_name.clone(),
                    cancel_token,
                },
            );
        }

        log!("[Scheduler] Restarted task '{}'", task_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Insert a tracked task that observes the cancel token and records when it
    /// was woken. Returns the task_id and a flag the task flips on cancel.
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
            },
        );
        (task_id, observed)
    }

    #[tokio::test]
    async fn cancel_tracked_task_signals_cancel_and_removes_entry() {
        // Regression: aborting the JoinHandle (the previous behavior) would
        // tear the agentic loop down mid-tool and leave the thread without a
        // terminal event, showing as stuck "running". Cooperative cancel via
        // the token gives the loop a chance to emit ResponseCanceled.
        let tracked = Arc::new(RwLock::new(HashMap::new()));
        let (task_id, observed) = insert_observed_task(&tracked, "test-cancel").await;

        cancel_tracked_task(&tracked, task_id).await;

        // Task observes the cancel signal within a short window
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
    async fn detach_tracked_task_does_not_signal_cancel() {
        // Self-deletion path: the trigger's own LLM called delete_trigger on
        // itself. Cancelling here would interrupt the in-flight tool call;
        // the natural agentic-loop completion will clean up.
        let tracked = Arc::new(RwLock::new(HashMap::new()));
        let (task_id, observed) = insert_observed_task(&tracked, "test-detach").await;

        detach_tracked_task(&tracked, task_id).await;

        // Give the cancel signal time to wrongly fire if the implementation
        // regressed.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(
            !observed.load(Ordering::SeqCst),
            "task must NOT observe cancel signal on detach"
        );
        assert!(
            tracked.read().await.get(&task_id).is_none(),
            "tracked entry should still be removed"
        );
    }

    #[tokio::test]
    async fn cancel_tracked_task_is_noop_for_unknown_id() {
        let tracked = Arc::new(RwLock::new(HashMap::new()));
        cancel_tracked_task(&tracked, uuid::Uuid::new_v4()).await;
        assert!(tracked.read().await.is_empty());
    }

    // ── Missed-slot catch-up decision ───────────────────────────────────────
    //
    // Regression suite for the 2026-07-29 double-fire: the 07:45 slot ran once
    // at 07:49 and the 08:12 restart's catch-up ran it again, sending a second
    // push notification for the same morning.

    fn utc(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    /// Run history with a creation time well before any slot under test, so a
    /// case that isn't about creation isn't accidentally decided by it.
    fn history(last_run: Option<&str>) -> Result<crate::triggers::TriggerRunHistory, sqlx::Error> {
        Ok(crate::triggers::TriggerRunHistory {
            last_run: last_run.map(utc),
            created_at: Some(utc("2026-04-08T07:00:52Z")),
        })
    }

    const SLOT: &str = "2026-07-29T05:45:00Z";

    #[test]
    fn catch_up_skips_a_slot_that_already_ran_late() {
        // The 07:45 slot fired at 07:49 (241s late, after a macOS sleep). A late
        // fire still counts as that slot having run — the restart 23 minutes
        // later must not run it again.
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
        // The exact incident shape. `events.created` (Postgres clock) was 280s
        // behind `payload.last_run` (engine clock) because the Docker VM clock
        // had not resynced after the host slept. If anything hands the catch-up
        // a stale/early in-memory value, the event store's engine-clock reading
        // must still suppress the fire — the two sources are maxed, not
        // preferred in order.
        assert_eq!(
            catch_up_decision(
                utc(SLOT),
                Some(utc("2026-07-29T05:44:35Z")), // the DB-clock value that caused the bug
                history(Some("2026-07-29T05:49:15Z")),
            ),
            CatchUp::Skip(SkipReason::AlreadyRan)
        );
    }

    #[test]
    fn catch_up_fires_a_genuinely_missed_slot() {
        // Engine was down across the scheduled time and the slot never ran. The
        // catch-up is a real feature — this is the case it exists for.
        assert_eq!(
            catch_up_decision(
                utc(SLOT),
                Some(utc("2026-07-28T05:45:08Z")), // yesterday's slot
                history(Some("2026-07-28T05:45:08Z")),
            ),
            CatchUp::Fire
        );
    }

    #[test]
    fn catch_up_fires_when_the_trigger_has_never_run() {
        // No run recorded anywhere, and the trigger predates the slot.
        assert_eq!(
            catch_up_decision(utc(SLOT), None, history(None)),
            CatchUp::Fire
        );
    }

    #[test]
    fn catch_up_skips_when_last_run_is_none_but_the_event_store_says_it_ran() {
        // A `None` `last_run` must NOT read as "safe to fire". The old guard was
        // `if let Some(last_run)`, so `None` skipped the check entirely.
        assert_eq!(
            catch_up_decision(utc(SLOT), None, history(Some("2026-07-29T05:49:15Z"))),
            CatchUp::Skip(SkipReason::AlreadyRan)
        );
    }

    #[test]
    fn catch_up_skips_when_the_run_history_is_unavailable() {
        // Fail closed: unable to prove the slot hasn't run. A skipped catch-up
        // is recoverable; a duplicate push notification is not.
        assert_eq!(
            catch_up_decision(utc(SLOT), None, Err(sqlx::Error::PoolClosed)),
            CatchUp::Skip(SkipReason::HistoryUnavailable)
        );
        // Even with an in-memory value that looks safe to fire on.
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
        // Creating a `0 45 7 * * *` trigger at 07:50 must not fire it on the
        // spot for the 07:45 slot it never existed for. `TriggerCreated` /
        // `TriggerUpdated` / `TriggerEnabled` all re-register the task runner,
        // so this path runs on every trigger edit.
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
        // Complement: created before the slot and never ran → still catches up.
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
        // A trigger whose `TriggerCreated` row is missing (legacy / migrated
        // data) must not lose its catch-up — the run history is the load-bearing
        // check, the creation time only rules out pre-existence.
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
    fn skip_reason_shutting_down_takes_precedence() {
        // Regression: during graceful shutdown / restart the engine emits
        // terminator events (ResponseAborted{EngineShutdown}, CodingAgentIdled,
        // SessionEnded) that match the notify-on-idle-and-new-changes trigger.
        // Firing it would spawn a script whose `lucidos threads count` callback
        // hits the API mid-restart and dies with a "<trigger> failed" push.
        // Shutdown must short-circuit dispatch regardless of depth.
        assert_eq!(
            event_trigger_skip_reason(true, 0),
            Some(EventTriggerSkip::ShuttingDown)
        );
        assert_eq!(
            event_trigger_skip_reason(true, MAX_EVENT_TRIGGER_DEPTH + 5),
            Some(EventTriggerSkip::ShuttingDown),
            "shutdown wins over the depth cap so the log names the real reason"
        );
    }

    /// The whole point of Bug 2's fix, walked end to end with the real pieces.
    ///
    /// A trigger subscribed to an event its own run emits is a feedback loop.
    /// `system-knowhow/triggers.md` states it as an authoring rule, and this is
    /// the engine backstop under it. Three parts have to agree: the fire runs
    /// inside `EVENT_TRIGGER_DEPTH.scope(next_depth)`, the events it emits read
    /// that scope, and the dispatcher decides on what they read.
    ///
    /// The loop below drives all three. It used to run forever, because the
    /// thread-event dispatch hardcoded zero and the scope was never read.
    #[tokio::test]
    async fn a_self_subscribed_trigger_stops_at_the_depth_cap() {
        use crate::scheduler::user_tasks::{current_event_trigger_depth, EVENT_TRIGGER_DEPTH};

        // The first event is emitted by an ordinary turn, outside any fire.
        let mut observed_depth = current_event_trigger_depth();
        let mut fires = 0u32;

        while event_trigger_skip_reason(false, observed_depth).is_none() {
            fires += 1;
            assert!(
                fires <= MAX_EVENT_TRIGGER_DEPTH + 1,
                "the chain must terminate, not run away"
            );
            // `handle_domain_event`'s `next_depth`, then the executor's scope.
            let next_depth = observed_depth + 1;
            observed_depth = EVENT_TRIGGER_DEPTH
                .scope(next_depth, async { current_event_trigger_depth() })
                .await;
        }

        assert_eq!(
            fires, MAX_EVENT_TRIGGER_DEPTH,
            "a self-subscribed trigger gets {} fires and then stops",
            MAX_EVENT_TRIGGER_DEPTH
        );
    }

    #[test]
    fn skip_reason_depth_cap_when_not_shutting_down() {
        assert_eq!(
            event_trigger_skip_reason(false, MAX_EVENT_TRIGGER_DEPTH),
            Some(EventTriggerSkip::MaxDepth)
        );
        assert_eq!(
            event_trigger_skip_reason(false, MAX_EVENT_TRIGGER_DEPTH - 1),
            None,
            "below the cap and not shutting down → dispatch proceeds"
        );
        assert_eq!(
            event_trigger_skip_reason(false, 0),
            None,
            "normal operation → dispatch proceeds"
        );
    }
}
