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
use tokio_cron_scheduler::{Job, JobScheduler};

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
                "run": serde_json::to_value(TriggerRun::Intent { intent: format!("Run trigger {}", legacy_target) }).unwrap(),
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
            // The triggering event payload arrives appended to the user
            // message under a `## Triggering Event` JSON block (built by
            // `build_trigger_user_message`) — instruct the LLM to read the
            // `question` field from there rather than relying on template
            // substitution (none exists in the engine).
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
                    log!(
                        "[Scheduler] Timeout waiting for {} task(s), aborting",
                        remaining
                    );
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

mod task_runner;
use task_runner::{
    check_task_health_and_restart, handle_domain_event, handle_trigger_event, spawn_task_runner,
    TrackedTask,
};

mod backup;
use backup::run_scheduled_backup;
pub(crate) use backup::{run_backup, BackupGuard};

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
}
