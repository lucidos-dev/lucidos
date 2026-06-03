//! Per-trigger task runner: spawns a tokio task that wakes on cron expressions
//! and invokes the agentic loop. Lifecycle helpers (`cancel`/`detach`/health
//! check / event-driven registration) live here too so the parent
//! `SchedulerManager` only has to forward to them.

use crate::api::SharedEngine;
use crate::triggers::TriggerConfig;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{trigger_id_to_uuid, user_tasks, ACTIVE_TASK_COUNT, MISSED_TASK_GRACE_MINUTES};

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
    pool: PgPool,
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
            pool,
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
    pool: PgPool,
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
        &pool,
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

        // Read fresh config from in-memory state (event-sourced)
        let config = {
            let configs = trigger_configs.read().unwrap();
            configs.get(&trigger_id).cloned()
        };
        let config = match config {
            Some(c) if !c.paused => c,
            Some(_) => return Ok("trigger paused".to_string()),
            None => return Ok("trigger deleted".to_string()),
        };

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

        // Execute the task with the fresh data
        let active = ACTIVE_TASK_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if active > 1 {
            log!(
                "[Scheduler] Concurrent execution: {} tasks now active (starting '{}')",
                active,
                task_name
            );
        }

        let result = user_tasks::execute_user_task(
            engine.clone(),
            &pool,
            &config,
            crate::engine::thread_events::TriggerInvocation::Schedule,
            None,
            Some(cancel_token.clone()),
        )
        .await;

        ACTIVE_TASK_COUNT.fetch_sub(1, Ordering::Relaxed);

        // Record after execution so crash mid-task → catch-up re-executes.
        engine.record_trigger_executed(&trigger_id).await;

        if let Err(e) = result {
            log!("[Scheduler] Task '{}' execution failed: {}", task_name, e);
            // Continue to next occurrence rather than crashing
        }
    }
}

/// Check if we just missed a scheduled time and execute if within grace period
async fn check_and_execute_missed(
    schedules: &[cron::Schedule],
    tz: chrono_tz::Tz,
    trigger_id: &str,
    task_name: &str,
    engine: &SharedEngine,
    pool: &PgPool,
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

        // Guard: skip if this occurrence was already executed
        if let Some(last_run) = config.last_run {
            if last_run >= missed_utc {
                return Ok(());
            }
        }

        log!(
            "[Scheduler] Task '{}' missed at {} ({}s ago), executing now",
            task_name,
            missed.format("%H:%M:%S"),
            delay.num_seconds()
        );

        if let Err(e) = user_tasks::execute_user_task(
            engine.clone(),
            pool,
            &config,
            crate::engine::thread_events::TriggerInvocation::Schedule,
            None,
            None,
        )
        .await
        {
            log!(
                "[Scheduler] Task '{}' grace period execution failed: {}",
                task_name,
                e
            );
        }
        engine.record_trigger_executed(trigger_id).await;
    }

    Ok(())
}

/// Handle a trigger lifecycle event from the EventBus subscriber.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_trigger_event(
    event_type: &str,
    trigger_id: &str,
    payload: &serde_json::Value,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
    tracked_tasks: &Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    engine: &SharedEngine,
    pool: &PgPool,
    shutdown_flag: &Arc<AtomicBool>,
) {
    let task_uuid = trigger_id_to_uuid(trigger_id);

    match event_type {
        "TriggerCreated" => {
            if let Ok(config) = TriggerConfig::from_created_payload(payload) {
                let should_register = !config.paused && !config.schedule.is_empty();
                {
                    let mut configs = trigger_configs.write().unwrap();
                    configs.insert(trigger_id.to_string(), config.clone());
                }
                if should_register {
                    register_and_track(
                        &config,
                        tracked_tasks,
                        engine,
                        pool,
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
            let config_snapshot;
            {
                let mut configs = trigger_configs.write().unwrap();
                if let Some(config) = configs.get_mut(trigger_id) {
                    config.apply_update(payload);
                    config_snapshot = Some(config.clone());
                } else {
                    config_snapshot = None;
                }
            }
            if let Some(config) = config_snapshot {
                cancel_tracked_task(tracked_tasks, task_uuid).await;
                if !config.paused && !config.schedule.is_empty() {
                    register_and_track(
                        &config,
                        tracked_tasks,
                        engine,
                        pool,
                        shutdown_flag,
                        trigger_configs,
                    )
                    .await;
                }
                crate::log!("[Scheduler] Updated trigger: {}", trigger_id);
            }
        }
        "TriggerDeleted" => {
            {
                let mut configs = trigger_configs.write().unwrap();
                configs.remove(trigger_id);
            }
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
            let config_snapshot;
            {
                let mut configs = trigger_configs.write().unwrap();
                if let Some(config) = configs.get_mut(trigger_id) {
                    config.paused = false;
                    config_snapshot = Some(config.clone());
                } else {
                    config_snapshot = None;
                }
            }
            if let Some(config) = config_snapshot {
                if !config.schedule.is_empty() {
                    register_and_track(
                        &config,
                        tracked_tasks,
                        engine,
                        pool,
                        shutdown_flag,
                        trigger_configs,
                    )
                    .await;
                }
                crate::log!("[Scheduler] Resumed trigger: {}", trigger_id);
            }
        }
        "TriggerDisabled" => {
            {
                let mut configs = trigger_configs.write().unwrap();
                if let Some(config) = configs.get_mut(trigger_id) {
                    config.paused = true;
                }
            }
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
    pool: &PgPool,
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
        pool.clone(),
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
    pool: &PgPool,
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

    let next_depth = depth + 1;
    for config in matching {
        let engine = engine.clone();
        let pool = pool.clone();
        let event_type = event_type.to_string();
        let event_payload = payload.clone();

        ACTIVE_TASK_COUNT.fetch_add(1, Ordering::Relaxed);
        tokio::spawn(async move {
            crate::log!(
                "[Scheduler] Firing event trigger '{}' for event '{}'",
                config.name,
                event_type
            );

            let invocation = crate::engine::thread_events::TriggerInvocation::Event {
                event_type: event_type.clone(),
                event_id: source_event_id,
                thread_id: origin_thread_id,
            };
            let inner = user_tasks::EVENT_TRIGGER_DEPTH.scope(
                next_depth,
                user_tasks::execute_user_task(
                    engine.clone(),
                    &pool,
                    &config,
                    invocation,
                    Some(&event_payload),
                    None,
                ),
            );
            let result = match origin_thread_id {
                Some(tid) => user_tasks::ORIGIN_THREAD_ID.scope(tid, inner).await,
                None => inner.await,
            };

            engine.record_trigger_executed(&config.id).await;
            ACTIVE_TASK_COUNT.fetch_sub(1, Ordering::Relaxed);

            if let Err(e) = result {
                crate::log!("[Scheduler] Event trigger '{}' failed: {}", config.name, e);
            }
        });
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
    pool: PgPool,
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
            pool.clone(),
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
