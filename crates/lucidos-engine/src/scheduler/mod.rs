//! Scheduler module for Lucidos periodic triggers
//!
//! Provides two categories of triggers:
//! - **System triggers**: Silent/internal (session summaries, maintenance)
//! - **User triggers**: Visible (morning brief, scheduled research)
//!
//! ## Reliability Features
//! - JoinHandle tracking for spawned tasks
//! - Automatic crash detection and task restart
//! - Panic-safe task execution
//! - Fresh task data fetch on each execution

pub mod notifications;
pub mod push;
#[cfg(test)]
mod tasks;
pub mod user_tasks;

pub use notifications::{Notification, NotificationStore};
pub use push::{PushSubscription, PushSubscriptionStore};

use crate::api::SharedEngine;
use crate::core::PreferenceStore;
use crate::triggers::{replay_trigger_events, TriggerConfig, TriggerEventRow, TriggerRun};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_cron_scheduler::{Job, JobScheduler};
use tokio_util::sync::CancellationToken;

/// Grace period for missed task execution (applies to both startup catch-up and late wake)
const MISSED_TASK_GRACE_MINUTES: i64 = 60;

/// Namespace UUID for deriving trigger UUIDs via v5 (SHA-1).
const TRIGGER_UUID_NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0x63, 0x6f, 0x67, 0x6e, 0x6f, 0x73, 0x2d, 0x74, 0x72, 0x69, 0x67, 0x67, 0x65, 0x72, 0x2d, 0x6e,
]); // "lucidos-trigger-n"

/// Derive a deterministic UUID from a trigger ID string (uuid v5 / SHA-1).
pub(crate) fn trigger_id_to_uuid(trigger_id: &str) -> uuid::Uuid {
    uuid::Uuid::new_v5(&TRIGGER_UUID_NAMESPACE, trigger_id.as_bytes())
}

/// Tracks how many user tasks are executing concurrently
static ACTIVE_TASK_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Information about a tracked task.
///
/// `cancel_token` is observed by the task runner (between scheduled executions)
/// and by the agentic loop (between iterations). Signaling cancel lets the task
/// finish its current operation cleanly and emit terminal events; callers must
/// not abort the `JoinHandle` directly, or the thread is left without a
/// `ResponseGenerated`/`ResponseCanceled` event and shows as stuck "running".
struct TrackedTask {
    handle: JoinHandle<()>,
    task_name: String,
    cancel_token: CancellationToken,
}

/// Manages all triggers in Lucidos
pub struct SchedulerManager {
    scheduler: JobScheduler,
    engine: SharedEngine,
    pool: PgPool,
    /// Track spawned task handles for lifecycle management
    /// Key: task_id, Value: JoinHandle and metadata
    tracked_tasks: Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    /// Job UUID for the scheduled backup (so we can remove/replace it)
    backup_job_id: Option<uuid::Uuid>,
    /// Shared flag signaling task runners to stop scheduling new executions
    shutdown_flag: Arc<AtomicBool>,
    /// In-memory trigger configs, rebuilt from events on startup and kept
    /// up-to-date via EventBus subscription. Source of truth for trigger
    /// listing — replaces DB queries to trigger_crons table.
    trigger_configs: Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
}

impl SchedulerManager {
    /// Create a new scheduler manager.
    /// `trigger_configs` is a shared Arc that the engine also holds, so both
    /// the scheduler and engine tools see the same in-memory trigger state.
    pub async fn new(
        engine: SharedEngine,
        pool: PgPool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let scheduler = JobScheduler::new().await?;

        // Initialize database schemas
        NotificationStore::init_schema(&pool).await?;
        PushSubscriptionStore::init_schema(&pool).await?;
        crate::core::DeviceStore::init_schema(&pool).await?;

        // Share the same trigger_configs Arc as the engine
        let trigger_configs = engine.trigger_configs.clone();

        Ok(Self {
            scheduler,
            engine,
            pool,
            tracked_tasks: Arc::new(RwLock::new(HashMap::new())),
            backup_job_id: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            trigger_configs,
        })
    }

    /// Start the scheduler and register all tasks
    pub async fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Register system tasks (hardcoded)
        self.register_system_tasks().await?;

        // Migrate legacy trigger_crons table to events (idempotent)
        self.migrate_db_triggers_to_events().await;

        // Replay trigger lifecycle events to rebuild in-memory state
        self.replay_triggers_from_events().await;

        // Fix stale placeholder trigger prompts (one-time, idempotent)
        self.migrate_stale_trigger_prompts().await;

        // Register all enabled triggers from in-memory state
        self.register_triggers_from_configs().await?;

        // Seed default triggers shipped with the engine (idempotent — uses a
        // fixed marker preference so we never re-seed once the user deletes them).
        self.seed_default_triggers().await;

        // Subscribe to EventBus for live trigger CRUD updates
        self.start_trigger_event_subscriber();

        // Load backup schedule from preferences (if any)
        self.load_backup_schedule().await;

        // Start the scheduler
        self.scheduler.start().await?;
        log!("[Scheduler] Started");

        Ok(())
    }

    /// Register all system tasks
    async fn register_system_tasks(
        &mut self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let tracked_tasks = self.tracked_tasks.clone();
        let engine_health = self.engine.clone();
        let pool_health = self.pool.clone();
        let health_shutdown = self.shutdown_flag.clone();
        let trigger_configs = self.trigger_configs.clone();

        let health_job = Job::new_async("*/30 * * * * *", move |_uuid, _lock| {
            let tracked = tracked_tasks.clone();
            let engine = engine_health.clone();
            let pool = pool_health.clone();
            let shutdown = health_shutdown.clone();
            let configs = trigger_configs.clone();
            Box::pin(async move {
                check_task_health_and_restart(tracked, engine, pool, shutdown, configs).await;
            })
        })?;

        self.scheduler.add(health_job).await?;
        log!("[Scheduler] Registered system task: task_health_monitor");

        Ok(())
    }

    /// Migrate legacy `trigger_crons` table rows to event-sourced TriggerCreated events.
    /// Idempotent: skips if trigger events already exist or the table does not exist.
    /// After successful migration, drops the legacy table.
    async fn migrate_db_triggers_to_events(&self) {
        async fn drop_legacy_trigger_crons(pool: &PgPool) {
            if let Err(e) = sqlx::query("DROP TABLE IF EXISTS trigger_crons")
                .execute(pool)
                .await
            {
                log!(
                    "[Scheduler] Failed to drop legacy trigger_crons table: {}",
                    e
                );
            }
        }

        use crate::engine::event_bus::{BusEvent, SystemEvent};

        // Check if trigger_crons table exists
        let table_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_name = 'trigger_crons'
            )",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false);

        if !table_exists {
            return;
        }

        // Check if TriggerCreated events already exist (idempotency)
        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE event_type = 'TriggerCreated' AND aggregate = 'trigger'"
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        if event_count > 0 {
            drop_legacy_trigger_crons(&self.pool).await;
            return;
        }

        // Read all rows from legacy table
        let rows = sqlx::query_as::<_, (uuid::Uuid, String, String, Option<serde_json::Value>, serde_json::Value, String, bool, Option<chrono::DateTime<chrono::Utc>>)>(
            "SELECT id, name, skill_id, args, cron_expressions, timezone, enabled, last_run FROM trigger_crons ORDER BY created_at ASC"
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_else(|e| {
            log!("[Scheduler] Failed to read trigger_crons for migration: {}", e);
            vec![]
        });

        if rows.is_empty() {
            drop_legacy_trigger_crons(&self.pool).await;
            return;
        }

        let count = rows.len();
        for (id, name, legacy_target, _args, cron_json, timezone, enabled, _last_run) in rows {
            let schedule: Vec<String> = serde_json::from_value(cron_json).unwrap_or_default();
            let trigger_id_str = id.to_string();
            let payload = serde_json::json!({
                "trigger_id": trigger_id_str,
                "name": name,
                "schedule": schedule,
                "timezone": timezone,
                "run": serde_json::to_value(TriggerRun::Intent { intent: format!("Run trigger {}", legacy_target), knowhow: vec![] }).unwrap(),
            });

            if let Err(e) = self
                .engine
                .event_bus
                .emit(BusEvent::System(SystemEvent::TriggerCreated {
                    trigger_id: trigger_id_str.clone(),
                    payload,
                    actor: None,
                }))
                .await
            {
                log!(
                    "[Scheduler] Failed to emit TriggerCreated during migration for {}: {}",
                    trigger_id_str,
                    e
                );
            }

            // If the row was disabled, also emit TriggerDisabled
            if !enabled {
                if let Err(e) = self
                    .engine
                    .event_bus
                    .emit(BusEvent::System(SystemEvent::TriggerDisabled {
                        trigger_id: trigger_id_str.clone(),
                        payload: serde_json::json!({ "trigger_id": trigger_id_str }),
                        actor: None,
                    }))
                    .await
                {
                    log!(
                        "[Scheduler] Failed to emit TriggerDisabled during migration for {}: {}",
                        trigger_id_str,
                        e
                    );
                }
            }
        }

        log!(
            "[Scheduler] Migrated {} trigger(s) from trigger_crons to events",
            count
        );

        // Drop the legacy table
        if let Err(e) = sqlx::query("DROP TABLE IF EXISTS trigger_crons")
            .execute(&self.pool)
            .await
        {
            log!("[Scheduler] Failed to drop trigger_crons table: {}", e);
        }
    }

    /// Replay trigger lifecycle events from the events table to rebuild in-memory state.
    async fn replay_triggers_from_events(&self) {
        let rows = sqlx::query_as::<_, (String, serde_json::Value, chrono::DateTime<chrono::Utc>)>(
            "SELECT event_type, payload, created FROM events
             WHERE aggregate = 'trigger'
             ORDER BY sequence ASC",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_else(|e| {
            log!("[Scheduler] Failed to replay trigger events: {}", e);
            vec![]
        });

        let event_rows: Vec<TriggerEventRow> = rows
            .into_iter()
            .map(|(event_type, payload, created)| TriggerEventRow {
                event_type,
                payload,
                created,
            })
            .collect();

        let count = event_rows.len();
        let triggers = replay_trigger_events(event_rows);
        let active = triggers.values().filter(|t| !t.paused).count();

        let total = triggers.len();
        {
            let mut configs = self.trigger_configs.write().unwrap();
            *configs = triggers;
        }

        if count > 0 {
            log!(
                "[Scheduler] Replayed {} trigger events → {} triggers ({} active)",
                count,
                total,
                active
            );
        }
    }

    /// Migrate triggers whose prompt text is a stale placeholder like `"Run trigger ..."` or `"Run skill ..."`.
    ///
    /// The legacy DB migration set all trigger prompts to a placeholder which is
    /// meaningless. This replaces the text with actual prompt content by matching
    /// the trigger's display name to prompt file frontmatter names.
    ///
    /// Idempotent: only matches triggers whose text starts with `"Run skill "` or `"Run trigger "`.
    async fn migrate_stale_trigger_prompts(&self) {
        use crate::engine::event_bus::{BusEvent, SystemEvent};

        let stale_configs: Vec<TriggerConfig> = {
            let configs = self.trigger_configs.read().unwrap();
            configs.values()
                .filter(|c| matches!(&c.run, TriggerRun::Intent { intent, .. } if intent.starts_with("Run skill ") || intent.starts_with("Run trigger ")))
                .cloned()
                .collect()
        };

        if stale_configs.is_empty() {
            return;
        }

        let data_dir = self.engine.workspace_path().join(crate::core::DATA_DIR);
        let all_intents = crate::core::IntentStore::load_all(&data_dir);
        let intents_by_name: HashMap<&str, &crate::core::intents::Intent> =
            all_intents.iter().map(|p| (p.name.as_str(), p)).collect();

        let mut updated = 0;
        for config in &stale_configs {
            let new_run = if let Some(intent) = intents_by_name.get(config.name.as_str()) {
                Some(TriggerRun::Intent {
                    intent: intent.content.clone(),
                    knowhow: intent.knowhow.clone(),
                })
            } else {
                find_matching_script(&data_dir, &config.name)
                    .map(|path| TriggerRun::Script { path })
            };

            if let Some(new_run) = new_run {
                let payload = serde_json::json!({
                    "trigger_id": config.id,
                    "run": serde_json::to_value(&new_run).unwrap(),
                });

                // Persist TriggerUpdated event
                if let Err(e) = self
                    .engine
                    .event_bus
                    .emit(BusEvent::System(SystemEvent::TriggerUpdated {
                        trigger_id: config.id.clone(),
                        payload: payload.clone(),
                        actor: None,
                    }))
                    .await
                {
                    log!(
                        "[Scheduler] Failed to emit TriggerUpdated for {}: {}",
                        config.id,
                        e
                    );
                    continue;
                }

                // Update in-memory state directly (subscriber not started yet)
                {
                    let mut configs = self.trigger_configs.write().unwrap();
                    if let Some(c) = configs.get_mut(&config.id) {
                        c.apply_update(&payload);
                    }
                }

                updated += 1;
                log!(
                    "[Scheduler] Fixed stale trigger prompt: {} ({})",
                    config.name,
                    config.id
                );
            } else {
                log!("[Scheduler] WARN: Trigger '{}' ({}) has stale prompt — no matching prompt or script found",
                    config.name, config.id);
            }
        }

        if updated > 0 {
            log!("[Scheduler] Migrated {} stale trigger prompt(s)", updated);
        }
    }

    /// Register all active (non-paused) triggers from in-memory configs with the cron scheduler.
    async fn register_triggers_from_configs(
        &mut self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let configs: Vec<TriggerConfig> = {
            let configs = self.trigger_configs.read().unwrap();
            configs
                .values()
                .filter(|t| !t.paused && !t.schedule.is_empty())
                .cloned()
                .collect()
        };

        for config in configs {
            self.register_trigger_from_config(&config).await?;
        }

        Ok(())
    }

    /// Register a single trigger from its in-memory config.
    async fn register_trigger_from_config(
        &mut self,
        config: &TriggerConfig,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Generate a deterministic UUID from the trigger ID for tracking
        let task_id = trigger_id_to_uuid(&config.id);

        // Check if already tracked
        {
            let tracked = self.tracked_tasks.read().await;
            if tracked.contains_key(&task_id) {
                return Ok(());
            }
        }

        let (handle, cancel_token) = spawn_task_runner(
            config.id.clone(),
            config.name.clone(),
            config.schedule.clone(),
            config.timezone.clone(),
            self.engine.clone(),
            self.pool.clone(),
            self.shutdown_flag.clone(),
            self.trigger_configs.clone(),
        );

        {
            let mut tracked = self.tracked_tasks.write().await;
            tracked.insert(
                task_id,
                TrackedTask {
                    handle,
                    task_name: config.name.clone(),
                    cancel_token,
                },
            );
        }

        log!(
            "[Scheduler] Registered trigger: {} ({} in {})",
            config.name,
            config.schedule.join(", "),
            config.timezone
        );

        Ok(())
    }

    /// Start a background task that subscribes to EventBus for trigger lifecycle
    /// events AND domain events (to fire event-based triggers).
    fn start_trigger_event_subscriber(&self) {
        let mut rx = self.engine.event_bus.subscribe();
        let trigger_configs = self.trigger_configs.clone();
        let tracked_tasks = self.tracked_tasks.clone();
        let engine = self.engine.clone();
        let pool = self.pool.clone();
        let shutdown_flag = self.shutdown_flag.clone();

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(emitted) => {
                        if let crate::engine::event_bus::BusEvent::System(se) = &emitted.typed {
                            use crate::engine::event_bus::SystemEvent;
                            match se {
                                SystemEvent::TriggerCreated {
                                    trigger_id,
                                    payload,
                                    ..
                                }
                                | SystemEvent::TriggerUpdated {
                                    trigger_id,
                                    payload,
                                    ..
                                }
                                | SystemEvent::TriggerDeleted {
                                    trigger_id,
                                    payload,
                                    ..
                                }
                                | SystemEvent::TriggerEnabled {
                                    trigger_id,
                                    payload,
                                    ..
                                }
                                | SystemEvent::TriggerDisabled {
                                    trigger_id,
                                    payload,
                                    ..
                                }
                                | SystemEvent::TriggerExecuted {
                                    trigger_id,
                                    payload,
                                } => {
                                    handle_trigger_event(
                                        se.event_type(),
                                        trigger_id,
                                        payload,
                                        &trigger_configs,
                                        &tracked_tasks,
                                        &engine,
                                        &pool,
                                        &shutdown_flag,
                                    )
                                    .await;
                                }
                                SystemEvent::DomainEvent {
                                    event_type,
                                    payload,
                                    depth,
                                    ..
                                } => {
                                    handle_domain_event(
                                        event_type,
                                        payload,
                                        *depth,
                                        None,
                                        Some(emitted.event_id),
                                        &trigger_configs,
                                        &engine,
                                        &pool,
                                    )
                                    .await;
                                }
                                _ => {}
                            }
                        }
                        // Allow a curated subset of ThreadEvents to fire triggers.
                        // Today only `UserQuestionAsked` is in the allowlist (consumed
                        // by the seeded push-notification trigger). Adding more events
                        // here is the explicit opt-in needed to avoid every CC tool
                        // call invoking the trigger matcher.
                        if let crate::engine::event_bus::BusEvent::Thread {
                            thread_id, event, ..
                        } = &emitted.typed
                        {
                            use crate::engine::thread_events::EventMeta;
                            if matches!(
                                event,
                                crate::engine::thread_events::ThreadEvent::UserQuestionAsked { .. }
                            ) {
                                let payload = event.to_payload(&EventMeta::NONE);
                                handle_domain_event(
                                    event.event_type(),
                                    &payload,
                                    0, // depth: thread events are top-level — no recursion concern.
                                    Some(*thread_id),
                                    Some(emitted.event_id),
                                    &trigger_configs,
                                    &engine,
                                    &pool,
                                )
                                .await;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        crate::log!("[Scheduler] EventBus subscriber lagged by {} events", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        crate::log!("[Scheduler] EventBus closed, stopping trigger subscriber");
                        break;
                    }
                }
            }
        });
    }

    /// Default triggers ship with a fresh workspace. Each marker is recorded
    /// in the `preferences` table so deletion sticks (re-seeding never
    /// resurrects a trigger the user removed).
    async fn seed_default_triggers(&self) {
        use crate::engine::event_bus::{BusEvent, SystemEvent};

        const SEED_MARKER_PREFIX: &str = "seeded_trigger:";

        // (marker_id, name, on_event, intent_text)
        // Marker ids are stable identifiers; never change once shipped.
        const DEFAULTS: &[(&str, &str, &str, &str)] = &[(
            "cc-question-push",
            "Notify on Claude question",
            "UserQuestionAsked",
            // The triggering event payload is appended to the prompt as a JSON
            // block by build_trigger_instructions — instruct the LLM to read
            // the `question` field from there rather than relying on
            // template substitution (none exists in the engine).
            "Send a push notification. Use 'Claude is asking' as the title \
                 and the value of the triggering event's `question` field as the message.",
        )];

        let mut timezone: Option<String> = None;
        for (marker_id, name, on_event, intent_text) in DEFAULTS {
            let pref_key = format!("{}{}", SEED_MARKER_PREFIX, marker_id);
            let already_seeded = PreferenceStore::get(&self.pool, &pref_key)
                .await
                .ok()
                .flatten()
                .is_some();
            if already_seeded {
                continue;
            }

            // Read timezone lazily on the first defaulted trigger we actually seed.
            let tz = match &timezone {
                Some(t) => t.clone(),
                None => {
                    let t = PreferenceStore::get(&self.pool, "timezone")
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "UTC".to_string());
                    timezone = Some(t.clone());
                    t
                }
            };
            let trigger_id_str = uuid::Uuid::new_v4().to_string();
            let payload = serde_json::json!({
                "trigger_id": trigger_id_str,
                "name": name,
                "schedule": Vec::<String>::new(),
                "timezone": tz,
                "on": on_event,
                "run": serde_json::to_value(TriggerRun::Intent {
                    intent: intent_text.to_string(),
                    knowhow: vec![],
                }).unwrap(),
            });

            if let Err(e) = self
                .engine
                .event_bus
                .emit(BusEvent::System(SystemEvent::TriggerCreated {
                    trigger_id: trigger_id_str.clone(),
                    payload,
                    actor: None,
                }))
                .await
            {
                log!(
                    "[Scheduler] Failed to seed default trigger '{}': {}",
                    marker_id,
                    e
                );
                continue;
            }
            if let Err(e) = PreferenceStore::set(&self.pool, &pref_key, &trigger_id_str).await {
                // Marker write failed — next startup would re-seed (creating a
                // duplicate). Log loudly so the user sees this in the log.
                log!(
                    "[Scheduler] WARN: failed to record seed marker for '{}': {}. \
                      Future startups may re-create this trigger.",
                    marker_id,
                    e
                );
            } else {
                log!(
                    "[Scheduler] Seeded default trigger '{}' ({})",
                    marker_id,
                    trigger_id_str
                );
            }
        }
    }

    /// List all trigger configs from in-memory state.
    pub fn list_trigger_configs(&self) -> Vec<TriggerConfig> {
        let configs = self.trigger_configs.read().unwrap();
        configs.values().cloned().collect()
    }

    /// Get a specific trigger config by ID.
    pub fn get_trigger_config(&self, id: &str) -> Option<TriggerConfig> {
        let configs = self.trigger_configs.read().unwrap();
        configs.get(id).cloned()
    }

    /// Load backup schedule from preferences on startup
    async fn load_backup_schedule(&mut self) {
        use crate::core::backup::{is_schedule_active, PREF_BACKUP_PROVIDER, PREF_BACKUP_SCHEDULE};

        let cron = match PreferenceStore::get(&self.pool, PREF_BACKUP_SCHEDULE).await {
            Ok(Some(c)) if is_schedule_active(&c) => c,
            _ => return,
        };
        let provider = match PreferenceStore::get(&self.pool, PREF_BACKUP_PROVIDER).await {
            Ok(Some(p)) if !p.is_empty() => p,
            _ => return,
        };
        if let Err(e) = self.register_backup_job(&cron, &provider).await {
            log!("[Scheduler] Failed to load backup schedule: {}", e);
        }
    }

    /// Set or clear the automatic backup schedule.
    ///
    /// `cron`: A 6-field cron expression (e.g., "0 0 3 * * *" for daily at 3am),
    ///         or `None` to disable.
    /// `provider`: The backup provider ID (e.g., "google_drive").
    pub async fn set_backup_schedule(
        &mut self,
        cron: Option<&str>,
        provider: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::core::backup::{PREF_BACKUP_PROVIDER, PREF_BACKUP_SCHEDULE};

        // Remove existing backup job if any
        if let Some(job_id) = self.backup_job_id.take() {
            self.scheduler.remove(&job_id).await?;
            log!("[Scheduler] Removed previous backup job");
        }

        match cron {
            Some(expr) => {
                // Validate cron expression before persisting
                crate::engine::tools::scheduler::parse_standard_cron(expr)
                    .map_err(|e| format!("Invalid cron expression '{}': {}", expr, e))?;

                PreferenceStore::set(&self.pool, PREF_BACKUP_SCHEDULE, expr).await?;
                PreferenceStore::set(&self.pool, PREF_BACKUP_PROVIDER, provider).await?;

                self.register_backup_job(expr, provider).await?;
            }
            None => {
                // Disable schedule
                PreferenceStore::set(&self.pool, PREF_BACKUP_SCHEDULE, "off").await?;
                log!("[Scheduler] Backup schedule disabled");
            }
        }

        Ok(())
    }

    /// Register the backup cron job with the scheduler.
    async fn register_backup_job(
        &mut self,
        cron_expr: &str,
        provider_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let engine = self.engine.clone();
        let provider_id = provider_id.to_string();
        let provider_for_job = provider_id.clone();

        let job = Job::new_async(cron_expr, move |_uuid, _lock| {
            let engine = engine.clone();
            let prov = provider_for_job.clone();
            Box::pin(async move {
                run_scheduled_backup(engine, prov).await;
            })
        })?;

        let job_id = self.scheduler.add(job).await?;
        self.backup_job_id = Some(job_id);
        log!(
            "[Scheduler] Registered backup job: {} (provider: {})",
            cron_expr,
            provider_id
        );

        Ok(())
    }

    /// Shutdown the scheduler gracefully.
    ///
    /// 1. Signals all task runners to stop scheduling new executions
    /// 2. Waits up to 60s for in-flight task executions to complete
    /// 3. Aborts any remaining task handles
    pub async fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        log!("[Scheduler] Shutting down...");

        // Signal all task runners to stop — they'll exit after their current execution finishes
        self.shutdown_flag.store(true, Ordering::SeqCst);

        // Wait for in-flight task executions to complete (up to 60 seconds)
        let active = ACTIVE_TASK_COUNT.load(Ordering::Relaxed);
        if active > 0 {
            // Log which tasks are still running
            {
                let tracked = self.tracked_tasks.read().await;
                let running: Vec<_> = tracked
                    .values()
                    .filter(|t| !t.handle.is_finished())
                    .map(|t| t.task_name.as_str())
                    .collect();
                log!(
                    "[Scheduler] {} task(s) still executing ({}), waiting for completion...",
                    active,
                    running.join(", ")
                );
            }
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                if ACTIVE_TASK_COUNT.load(Ordering::Relaxed) == 0 {
                    log!("[Scheduler] All in-flight tasks completed");
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    let remaining = ACTIVE_TASK_COUNT.load(Ordering::Relaxed);
                    log!("[Scheduler] Timeout waiting for {} task(s), aborting", remaining);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }

        // Abort all tracked task handles (runners that are sleeping between executions)
        {
            let mut tracked = self.tracked_tasks.write().await;
            for (_, task) in tracked.drain() {
                task.handle.abort();
            }
        }

        self.scheduler.shutdown().await?;
        log!("[Scheduler] Shutdown complete");
        Ok(())
    }

}

/// Spawn a task runner that executes on schedule.
///
/// Returns the `JoinHandle` (for liveness checks) and a `CancellationToken` the
/// task observes between executions and inside the agentic loop. Cancel the
/// token instead of aborting the handle to let the task emit its terminal
/// events before exiting.
#[allow(clippy::too_many_arguments)]
fn spawn_task_runner(
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
            log!("[Scheduler] Task '{}' grace period execution failed: {}", task_name, e);
        }
        engine.record_trigger_executed(trigger_id).await;
    }

    Ok(())
}

/// Handle a trigger lifecycle event from the EventBus subscriber.
#[allow(clippy::too_many_arguments)]
async fn handle_trigger_event(
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
async fn handle_domain_event(
    event_type: &str,
    payload: &serde_json::Value,
    depth: u32,
    origin_thread_id: Option<uuid::Uuid>,
    source_event_id: Option<uuid::Uuid>,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
    engine: &SharedEngine,
    pool: &PgPool,
) {
    if depth >= MAX_EVENT_TRIGGER_DEPTH {
        crate::log!(
            "[Scheduler] Event '{}' at depth {} — skipping triggers to prevent recursion",
            event_type,
            depth
        );
        return;
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
    let _ = tracked.remove(&task_id);
}

/// Check health of tracked tasks and restart any that have crashed
async fn check_task_health_and_restart(
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

/// RAII guard for `engine.backup_in_progress`. Acquired atomically so two
/// concurrent backup attempts can't both pass the check; cleared on drop so
/// a panic mid-backup doesn't permanently strand the flag.
pub(crate) struct BackupGuard(SharedEngine);

impl BackupGuard {
    /// Returns `Some` when the caller has exclusive ownership of the backup
    /// slot, `None` if another backup is already running.
    pub(crate) fn try_acquire(engine: &SharedEngine) -> Option<Self> {
        engine
            .backup_in_progress
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .ok()
            .map(|_| Self(engine.clone()))
    }
}

impl Drop for BackupGuard {
    fn drop(&mut self) {
        self.0
            .backup_in_progress
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Run the backup pipeline and emit terminal SSE events. Caller must hold a
/// `BackupGuard`. Used by both the manual API handler and the scheduled cron.
pub(crate) async fn run_backup(
    engine: &SharedEngine,
    pool: &sqlx::PgPool,
    workspace: &std::path::Path,
    database_url: &str,
    key: &[u8],
    provider: &dyn crate::core::backup::BackupProvider,
) {
    use crate::core::backup;
    use crate::engine::event_bus::{BusEvent, SystemEvent};

    let progress = crate::api::backup::progress_sender(engine.event_bus.sender());

    match backup::create_backup(workspace, database_url, key, provider, progress).await {
        Ok(entry) => {
            log!(
                "[Backup] Completed: {} ({:.1} MB)",
                entry.filename,
                entry.size_bytes as f64 / 1024.0 / 1024.0
            );
            engine
                .event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::BackupCompleted {
                        filename: entry.filename.clone(),
                        size_bytes: entry.size_bytes,
                    }),
                    "[Backup] BackupCompleted",
                )
                .await;
            let keep = backup::get_retention_count(pool).await;
            if let Err(e) = backup::prune_old_backups(provider, keep).await {
                log!("[Backup] Pruning failed (non-fatal): {}", e);
            }
        }
        Err(e) => {
            let msg = e.to_string();
            log!("[Backup] Failed: {}", msg);
            engine
                .event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::BackupFailed { error: msg.clone() }),
                    "[Backup] BackupFailed",
                )
                .await;
            notify_backup_failure(engine, &msg).await;
        }
    }
}

/// Execute a scheduled backup. Called by the cron job.
async fn run_scheduled_backup(engine: SharedEngine, provider_id: String) {
    use crate::core::backup::{self, crypto};

    let Some(_guard) = BackupGuard::try_acquire(&engine) else {
        log!("[Backup] Skipping scheduled backup — another backup is already running");
        return;
    };

    log!(
        "[Backup] Starting scheduled backup (provider: {})",
        provider_id
    );

    let pool = engine.pool();
    let workspace = engine.workspace_path().to_path_buf();

    let provider = match backup::get_provider(&provider_id, pool) {
        Ok(p) => p,
        Err(e) => {
            log!("[Backup] {}, skipping", e);
            notify_backup_failure(&engine, &e.to_string()).await;
            return;
        }
    };

    let key_path = backup::key_file_path(&workspace);
    let key = match crypto::load_key_file(&key_path) {
        Ok(Some(k)) => k,
        Ok(None) => {
            log!("[Backup] No backup key found, skipping scheduled backup");
            notify_backup_failure(
                &engine,
                "No backup encryption key found. Go to Settings > Backup to set up backup.",
            )
            .await;
            return;
        }
        Err(e) => {
            log!("[Backup] Failed to load key: {}", e);
            notify_backup_failure(&engine, &format!("Failed to load backup key: {}", e)).await;
            return;
        }
    };

    let database_url = crate::core::database_url();
    run_backup(
        &engine,
        pool,
        &workspace,
        &database_url,
        &key,
        provider.as_ref(),
    )
    .await;
}

const BACKUP_FAILURE_TITLE: &str = "Backup failed";
const BACKUP_FAILURE_DEDUP_MINUTES: i64 = 30;

/// Deduplicates backup failure notifications (max 1 per 30 minutes).
pub(crate) async fn notify_backup_failure(engine: &SharedEngine, error: &str) {
    use crate::engine::event_bus::{BusEvent, SystemEvent};

    let pool = engine.pool();
    let cutoff = chrono::Utc::now() - chrono::Duration::minutes(BACKUP_FAILURE_DEDUP_MINUTES);
    let recent: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM notifications WHERE title = $1 AND created_at > $2)",
    )
    .bind(BACKUP_FAILURE_TITLE)
    .bind(cutoff)
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    if recent {
        return;
    }

    let notification_id = uuid::Uuid::new_v4();

    if let Err(e) = engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::NotificationCreated {
            id: notification_id.to_string(),
            title: BACKUP_FAILURE_TITLE.to_string(),
            message: error.to_string(),
            task_id: None,
            app_id: None,
        }))
        .await
    {
        log!("[Backup] Failed to emit failure notification: {}", e);
    }

    push::send_push_to_all(pool, BACKUP_FAILURE_TITLE, error, Some(notification_id)).await;
}

/// Try to find a script matching a trigger name by keyword overlap.
///
/// Searches `data/triggers/*/scripts/` and `data/apps/*/scripts/` for `run.py`.
/// Returns the data-relative script path (e.g. `triggers/oura-import/scripts/run.py`).
fn find_matching_script(data_dir: &std::path::Path, trigger_name: &str) -> Option<String> {
    let name_lower = trigger_name.to_lowercase();
    for (subdir, prefix) in &[("triggers", "triggers"), ("apps", "apps")] {
        let search_dir = data_dir.join(subdir);
        if let Ok(entries) = std::fs::read_dir(&search_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !path.join("scripts").join("run.py").exists() {
                    continue;
                }
                let dir_keywords: Vec<&str> = dir_name.split('-').collect();
                if dir_keywords.iter().all(|kw| name_lower.contains(kw)) {
                    return Some(format!("{}/{}/scripts/run.py", prefix, dir_name));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_matching_script_in_triggers() {
        let dir = std::env::temp_dir().join("lucidos_test_find_script_v2");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("triggers/oura-import/scripts")).unwrap();
        std::fs::write(dir.join("triggers/oura-import/scripts/run.py"), "# oura").unwrap();
        std::fs::create_dir_all(dir.join("triggers/google-calendar-sync/scripts")).unwrap();
        std::fs::write(
            dir.join("triggers/google-calendar-sync/scripts/run.py"),
            "# cal",
        )
        .unwrap();

        assert_eq!(
            find_matching_script(&dir, "Oura Data Import"),
            Some("triggers/oura-import/scripts/run.py".to_string())
        );
        assert_eq!(
            find_matching_script(&dir, "Google Calendar sync (script, dynamisk)"),
            Some("triggers/google-calendar-sync/scripts/run.py".to_string())
        );
        // No match — "google" not in trigger name
        assert_eq!(
            find_matching_script(&dir, "Kalender: 30 min påminnelse (script, dynamisk)"),
            None
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_matching_script_no_run_py() {
        let dir = std::env::temp_dir().join("lucidos_test_find_script_no_py");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("oura-import")).unwrap();
        // No run.py in the dir

        assert_eq!(find_matching_script(&dir, "Oura Data Import"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

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
}
