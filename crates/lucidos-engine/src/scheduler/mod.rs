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
pub(crate) mod plugin_updates;
pub mod push;
#[cfg(feature = "e2e-test-hooks")]
pub mod push_test_log;
#[cfg(test)]
mod tasks;
pub mod user_tasks;

pub use notifications::{Notification, NotificationStore};
pub use push::{PushSubscription, PushSubscriptionStore};

use crate::api::SharedEngine;
use crate::core::PreferenceStore;
use crate::triggers::{
    replay_trigger_events, replay_trigger_group_events, TriggerConfig, TriggerEventRow,
    TriggerGroup, TriggerGroupEventRow, TriggerRun,
};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_cron_scheduler::{Job, JobScheduler};

/// Grace period for missed task execution (applies to both startup catch-up and late wake)
const MISSED_TASK_GRACE_MINUTES: i64 = 60;

/// Days of `last_seen_at` inactivity after which a push-enabled device gets
/// auto-disabled. Tuned to skip biweekly-use devices while catching obvious
/// PWA-reinstall ghosts.
const STALE_DEVICE_DAYS: i64 = 30;

/// Namespace UUID for deriving trigger UUIDs via v5 (SHA-1). The byte
/// sequence spells the historical "cognos-trigger-n" — DO NOT change it;
/// the bytes are part of the v5 hash input, so every trigger UUID derived
/// from a `trigger_id` string depends on this exact namespace. Renaming the
/// product to Lucidos doesn't get to invalidate persisted trigger ids.
const TRIGGER_UUID_NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0x63, 0x6f, 0x67, 0x6e, 0x6f, 0x73, 0x2d, 0x74, 0x72, 0x69, 0x67, 0x67, 0x65, 0x72, 0x2d, 0x6e,
]); // "cognos-trigger-n" — frozen for v5 namespace stability; see doc above.

/// Derive a deterministic UUID from a trigger ID string (uuid v5 / SHA-1).
pub(crate) fn trigger_id_to_uuid(trigger_id: &str) -> uuid::Uuid {
    uuid::Uuid::new_v5(&TRIGGER_UUID_NAMESPACE, trigger_id.as_bytes())
}

/// Tracks how many user tasks are executing concurrently. Incremented /
/// decremented by the Thread Queue executor around each trigger run
/// (`engine::thread_queue::executor`); the scheduler reads it for the
/// shutdown drain wait. Informational — capacity enforcement lives in the
/// Thread Queue's own accounting.
pub(crate) static ACTIVE_TASK_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Manages all triggers in Lucidos
pub struct SchedulerManager {
    scheduler: JobScheduler,
    engine: SharedEngine,
    pool: PgPool,
    /// Track spawned task handles for lifecycle management
    /// Key: task_id, Value: JoinHandle and metadata
    tracked_tasks: Arc<RwLock<HashMap<uuid::Uuid, TrackedTask>>>,
    /// Job UUID for the scheduled backup (so we can remove/replace it).
    /// Shared (`Arc<Mutex>`) so the EventBus subscriber task can re-register the
    /// job — on a `timezone` change or an agent/HTTP backup-pref write — without
    /// owning `&mut self`. Only ever held briefly (take/store a Uuid), never
    /// across an `.await`.
    backup_job_id: Arc<std::sync::Mutex<Option<uuid::Uuid>>>,
    /// Shared flag signaling task runners to stop scheduling new executions
    shutdown_flag: Arc<AtomicBool>,
    /// In-memory trigger configs, rebuilt from events on startup and kept
    /// up-to-date via EventBus subscription. Source of truth for trigger
    /// listing — replaces DB queries to trigger_crons table.
    trigger_configs: Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
    /// In-memory trigger groups, rebuilt from events on startup and kept
    /// up-to-date via EventBus subscription. Source of truth for the panel's
    /// "Group" sections.
    trigger_groups: Arc<std::sync::RwLock<HashMap<String, TriggerGroup>>>,
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
        let trigger_groups = engine.trigger_groups.clone();

        Ok(Self {
            scheduler,
            engine,
            pool,
            tracked_tasks: Arc::new(RwLock::new(HashMap::new())),
            backup_job_id: Arc::new(std::sync::Mutex::new(None)),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            trigger_configs,
            trigger_groups,
        })
    }

    /// Start the scheduler and register all tasks
    pub async fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Register system tasks (hardcoded)
        self.register_system_tasks().await?;

        // Migrate legacy trigger_crons table to events (idempotent)
        self.migrate_db_triggers_to_events().await?;

        // Replay trigger lifecycle events to rebuild in-memory state
        self.replay_triggers_from_events().await;

        // Name any replayed schedule that can never fire. Create and update
        // reject those now, so these are triggers stored before the guard
        // existed: they keep their config and stay registered (the runner exits
        // cleanly on "no more occurrences"), but they would otherwise sit in the
        // panel looking healthy while doing nothing forever.
        self.warn_on_dead_schedules();

        // Rebuild the on-disk trigger.toml projection from the replayed set
        // (derived read-model — ADR 0019): ensure it's git-ignored, write every
        // current definition, and prune orphans from renames/deletes that
        // happened while the engine was down. Best-effort (logs on failure).
        {
            let ws = self.engine.workspace_path().to_path_buf();
            let configs: Vec<crate::triggers::TriggerConfig> = self
                .trigger_configs
                .read()
                .unwrap()
                .values()
                .cloned()
                .collect();
            tokio::task::spawn_blocking(move || {
                crate::triggers::definition::ensure_trigger_toml_gitignored(&ws);
                crate::triggers::definition::rebuild_trigger_definitions(&ws, &configs);
            })
            .await
            .ok();
        }

        // Replay trigger-group lifecycle events into the parallel registry.
        // Groups don't schedule anything, but the panel reads this registry to
        // render the collapsible sections.
        self.replay_trigger_groups_from_events().await;

        // Fix stale placeholder trigger prompts (one-time, idempotent)
        self.migrate_stale_trigger_prompts().await;

        // Register all enabled triggers from in-memory state
        self.register_triggers_from_configs().await?;

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
        let health_shutdown = self.shutdown_flag.clone();
        let trigger_configs = self.trigger_configs.clone();

        let health_job = Job::new_async("*/30 * * * * *", move |_uuid, _lock| {
            let tracked = tracked_tasks.clone();
            let engine = engine_health.clone();
            let shutdown = health_shutdown.clone();
            let configs = trigger_configs.clone();
            Box::pin(async move {
                check_task_health_and_restart(tracked, engine, shutdown, configs).await;
            })
        })?;

        self.scheduler.add(health_job).await?;
        log!("[Scheduler] Registered system task: task_health_monitor");

        // Daily 3am local: disable push on devices not seen in 30 days. Catches
        // phantom subscriptions left behind by PWA reinstalls whose Apple/Google
        // endpoint never returned 410 (so the per-fan-out 410 cleanup never ran).
        // Disable-not-delete: zero data loss, fully reversible from Settings, no
        // event spam beyond one DevicePushChanged per actually-flipped row.
        let engine_prune = self.engine.clone();
        let pool_prune = self.pool.clone();
        let prune_job = Job::new_async("0 0 3 * * *", move |_uuid, _lock| {
            let engine = engine_prune.clone();
            let pool = pool_prune.clone();
            Box::pin(async move {
                disable_push_on_stale_devices(engine, pool, STALE_DEVICE_DAYS).await;
            })
        })?;
        self.scheduler.add(prune_job).await?;
        log!(
            "[Scheduler] Registered system task: disable_push_on_stale_devices (daily 3am, >{}d)",
            STALE_DEVICE_DAYS
        );

        // Also run once at startup so accumulated stale rows get caught
        // immediately after deploy without waiting for the first 3am tick.
        // Awaited inline (not spawned) so a shutdown mid-prune can't emit
        // DevicePushChanged events into a tearing-down EventBus — the work
        // is bounded (one SELECT plus one UPDATE per stale row).
        disable_push_on_stale_devices(self.engine.clone(), self.pool.clone(), STALE_DEVICE_DAYS)
            .await;

        let engine_plugin_updates = self.engine.clone();
        let pool_plugin_updates = self.pool.clone();
        let plugin_update_job =
            Job::new_async(MARKETPLACE_UPDATE_CHECK_CRON, move |_uuid, _lock| {
                let engine = engine_plugin_updates.clone();
                let pool = pool_plugin_updates.clone();
                Box::pin(async move {
                    run_plugin_marketplace_update_check(engine, pool).await;
                })
            })?;
        self.scheduler.add(plugin_update_job).await?;
        log!(
            "[Scheduler] Registered system task: plugin_marketplace_update_check ({})",
            MARKETPLACE_UPDATE_CHECK_CRON
        );

        let startup_engine = self.engine.clone();
        let startup_pool = self.pool.clone();
        tokio::spawn(async move {
            run_plugin_marketplace_update_check(startup_engine, startup_pool).await;
        });

        Ok(())
    }

    /// Migrate legacy `trigger_crons` table rows to event-sourced TriggerCreated events.
    /// Idempotent: skips if trigger events already exist or the table does not exist.
    /// After successful migration, drops the legacy table.
    ///
    /// Per-query errors propagate so a DB failure aborts startup loudly
    /// instead of leaving the legacy table in place with no events emitted.
    ///
    /// DEPRECATED — landed 2026-04-06 (commit 02515f4d9). Temporary measure,
    /// registered in docs/temporary-measures.md § "Scheduler one-time startup
    /// migrations". Dead-on-arrival for any
    /// install created after that date (the `trigger_crons` table never exists).
    /// Removal blocked on confirming every live install has started up at least
    /// once since the migration shipped; once verified, drop this function, its
    /// call site in `start()`, and the `ScheduledTrigger*` event aliases in
    /// `triggers/replay.rs`. Safe target: drop after the next major release that
    /// requires a fresh install (or after telemetry confirms zero workspaces
    /// retain the legacy table).
    async fn migrate_db_triggers_to_events(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

        use crate::engine::trigger_writes::TriggerWrite;

        // Check if trigger_crons table exists
        let table_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_name = 'trigger_crons'
            )",
        )
        .fetch_one(&self.pool)
        .await?;

        if !table_exists {
            return Ok(());
        }

        // Check if TriggerCreated events already exist (idempotency)
        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE event_type = 'TriggerCreated' AND aggregate = 'trigger'"
        )
        .fetch_one(&self.pool)
        .await?;

        if event_count > 0 {
            drop_legacy_trigger_crons(&self.pool).await;
            return Ok(());
        }

        // Read all rows from legacy table
        let rows = sqlx::query_as::<_, (uuid::Uuid, String, String, Option<serde_json::Value>, serde_json::Value, String, bool, Option<chrono::DateTime<chrono::Utc>>)>(
            "SELECT id, name, skill_id, args, cron_expressions, timezone, enabled, last_run FROM trigger_crons ORDER BY created_at ASC"
        )
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            drop_legacy_trigger_crons(&self.pool).await;
            return Ok(());
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

            self.engine
                .emit_trigger_write_or_log(
                    TriggerWrite::Created,
                    &trigger_id_str,
                    payload,
                    None,
                    "[Scheduler] (legacy migration)",
                )
                .await;

            // If the row was disabled, also record the pause.
            if !enabled {
                self.engine
                    .emit_trigger_write_or_log(
                        TriggerWrite::Disabled,
                        &trigger_id_str,
                        serde_json::json!({ "trigger_id": trigger_id_str }),
                        None,
                        "[Scheduler] (legacy migration)",
                    )
                    .await;
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
        Ok(())
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

    /// Replay trigger-group lifecycle events from the events table to rebuild
    /// the in-memory registry. Symmetric with `replay_triggers_from_events`.
    async fn replay_trigger_groups_from_events(&self) {
        let rows = sqlx::query_as::<_, (String, serde_json::Value, chrono::DateTime<chrono::Utc>)>(
            "SELECT event_type, payload, created FROM events
             WHERE aggregate = 'trigger_group'
             ORDER BY sequence ASC",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_else(|e| {
            log!("[Scheduler] Failed to replay trigger_group events: {}", e);
            vec![]
        });

        let event_rows: Vec<TriggerGroupEventRow> = rows
            .into_iter()
            .map(|(event_type, payload, created)| TriggerGroupEventRow {
                event_type,
                payload,
                created,
            })
            .collect();

        let count = event_rows.len();
        let groups = replay_trigger_group_events(event_rows);
        let total = groups.len();
        {
            let mut g = self.trigger_groups.write().unwrap();
            *g = groups;
        }

        if count > 0 {
            log!(
                "[Scheduler] Replayed {} trigger_group events → {} groups",
                count,
                total
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
    ///
    /// DEPRECATED — landed 2026-04-07 (commit d32b2a518). Temporary measure,
    /// registered in docs/temporary-measures.md § "Scheduler one-time startup
    /// migrations". Dead-on-arrival for any
    /// install created after that date (no triggers ever carry the stale placeholder).
    /// Removal blocked on confirming every live install has started up at least
    /// once since the fix shipped. Safe to drop together with
    /// `migrate_db_triggers_to_events` once telemetry confirms zero workspaces
    /// still hold the placeholder prompts.
    async fn migrate_stale_trigger_prompts(&self) {
        use crate::engine::trigger_writes::TriggerWrite;

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

                // The chokepoint persists the event AND applies it to the
                // registry, which this migration needs doubly: it runs before
                // the subscriber starts, so nothing else would.
                if let Err(e) = self
                    .engine
                    .emit_trigger_write(TriggerWrite::Updated, &config.id, payload.clone(), None)
                    .await
                {
                    log!(
                        "[Scheduler] Failed to emit TriggerUpdated for {}: {}",
                        config.id,
                        e
                    );
                    continue;
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

    /// Log one warning per replayed trigger whose cron schedule can never fire.
    ///
    /// Covers paused triggers too, which is why it does not live in
    /// `register_trigger_from_config`: registration skips them, and a paused
    /// trigger with a dead schedule is exactly as broken as an active one.
    fn warn_on_dead_schedules(&self) {
        let configs = self.trigger_configs.read().unwrap();
        for config in configs.values() {
            if let Some(problem) = config.schedule_error() {
                log!(
                    "[Scheduler] Trigger '{}' ({}) has a schedule that can never fire: {}. It stays registered but will not run until its cron is fixed.",
                    config.name,
                    config.id,
                    problem
                );
            }
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
        let trigger_groups = self.trigger_groups.clone();
        let tracked_tasks = self.tracked_tasks.clone();
        let engine = self.engine.clone();
        let shutdown_flag = self.shutdown_flag.clone();
        // For re-registering the backup cron when the timezone or a backup
        // preference changes (Job::new_async_tz bakes a fixed offset, and the
        // agent/HTTP write path is event-driven — tools can't reach the
        // scheduler directly).
        let scheduler = self.scheduler.clone();
        let backup_job_id = self.backup_job_id.clone();
        let pool = self.pool.clone();

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
                                        &shutdown_flag,
                                    )
                                    .await;
                                }
                                SystemEvent::TriggerGroupCreated {
                                    group_id, payload, ..
                                }
                                | SystemEvent::TriggerGroupRenamed {
                                    group_id, payload, ..
                                }
                                | SystemEvent::TriggerGroupReordered {
                                    group_id, payload, ..
                                }
                                | SystemEvent::TriggerGroupDeleted {
                                    group_id, payload, ..
                                } => {
                                    handle_trigger_group_event(
                                        se.event_type(),
                                        group_id,
                                        payload,
                                        &trigger_groups,
                                    );
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
                                    )
                                    .await;
                                }
                                // Re-register the backup cron when the user's
                                // timezone changes — `Job::new_async_tz` bakes a
                                // FIXED offset at registration, so the schedule
                                // must be re-baked to keep firing at the
                                // configured local time across the change (incl.
                                // DST).
                                SystemEvent::TimezoneSet { .. } => {
                                    reload_backup_schedule(
                                        &scheduler,
                                        &backup_job_id,
                                        &engine,
                                        &pool,
                                    )
                                    .await;
                                }
                                // Re-register when the backup schedule/provider
                                // changes via any write path — the agent's
                                // `set_preference` (the tool layer can't reach the
                                // scheduler) or the HTTP handler. Idempotent with
                                // the handler's own direct call.
                                SystemEvent::PreferencesChanged { key, .. }
                                    if key == crate::core::backup::PREF_BACKUP_SCHEDULE
                                        || key == crate::core::backup::PREF_BACKUP_PROVIDER =>
                                {
                                    reload_backup_schedule(
                                        &scheduler,
                                        &backup_job_id,
                                        &engine,
                                        &pool,
                                    )
                                    .await;
                                }
                                _ => {}
                            }
                        }
                        // Forward ThreadEvents to the trigger matcher unless they
                        // are per-token streaming (see
                        // `ThreadEvent::is_per_token_streaming`). Blocklist
                        // semantics — a workspace can `on_event:` any persisted
                        // ThreadEvent by default; new lifecycle and per-action
                        // variants are triggerable automatically. High-cardinality
                        // per-action variants (ToolCalled, CodingAgentToolCalled, …)
                        // flow through; workspaces scope them with `condition:`
                        // filters.
                        if let crate::engine::event_bus::BusEvent::Thread {
                            thread_id, event, ..
                        } = &emitted.typed
                        {
                            use crate::engine::thread_events::EventMeta;
                            if !event.is_per_token_streaming() {
                                let payload = event.to_payload(&EventMeta::NONE);
                                handle_domain_event(
                                    event.event_type(),
                                    &payload,
                                    0, // depth: thread events are top-level — no recursion concern.
                                    Some(*thread_id),
                                    Some(emitted.event_id),
                                    &trigger_configs,
                                    &engine,
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

    /// Load backup schedule from preferences on startup. Delegates to the shared
    /// reload path so startup, a timezone change, and an agent/HTTP pref write
    /// all register the job identically (in the user's timezone).
    async fn load_backup_schedule(&mut self) {
        reload_backup_schedule(
            &self.scheduler,
            &self.backup_job_id,
            &self.engine,
            &self.pool,
        )
        .await;
    }

    /// Set or clear the automatic backup schedule.
    ///
    /// `cron`: A 6-field cron expression (e.g., "0 0 3 * * *" for daily at 3am),
    ///         or `None` to disable.
    /// `provider`: The backup provider ID (e.g., "google_drive").
    /// `actor` is stamped on the `PreferencesChanged` events the store emits
    /// for the keys this writes. Those emits are load-bearing rather than
    /// cosmetic: the scheduler's own `PreferencesChanged` subscriber
    /// re-registers the backup cron from them, and the Settings page reloads on
    /// them. They used to be hand-rolled by the one HTTP caller, which left the
    /// next caller of this method silently un-announced.
    pub async fn set_backup_schedule(
        &mut self,
        cron: Option<&str>,
        provider: &str,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::core::backup::{PREF_BACKUP_PROVIDER, PREF_BACKUP_SCHEDULE};

        match cron {
            Some(expr) => {
                // Validate cron expression before persisting
                crate::engine::tools::scheduler::parse_standard_cron(expr)
                    .map_err(|e| format!("Invalid cron expression '{}': {}", expr, e))?;

                let bus = &self.engine.event_bus;
                PreferenceStore::set(&self.pool, bus, PREF_BACKUP_SCHEDULE, expr, actor.clone())
                    .await?;
                PreferenceStore::set(&self.pool, bus, PREF_BACKUP_PROVIDER, provider, actor)
                    .await?;

                register_backup_job(
                    &self.scheduler,
                    &self.backup_job_id,
                    &self.engine,
                    expr,
                    provider,
                )
                .await?;
            }
            None => {
                // Disable schedule. The PROVIDER is still written: it is the
                // configured destination, not a property of the cron, and the
                // Backup page reads it back to know which provider it is
                // talking about. Skipping it here made the destination
                // unwritable with the schedule off, so a user who picked
                // Dropbox while backups were manual-only kept whatever the
                // preference happened to hold.
                let bus = &self.engine.event_bus;
                PreferenceStore::set(&self.pool, bus, PREF_BACKUP_SCHEDULE, "off", actor.clone())
                    .await?;
                PreferenceStore::set(&self.pool, bus, PREF_BACKUP_PROVIDER, provider, actor)
                    .await?;
                remove_backup_job(&self.scheduler, &self.backup_job_id).await?;
                log!("[Scheduler] Backup schedule disabled");
            }
        }

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

/// Shared `Arc<Mutex>` holding the registered backup job's id (or `None`). Aliased
/// for the free backup-scheduling helpers below, which the methods and the
/// EventBus subscriber both call.
type BackupJobId = Arc<std::sync::Mutex<Option<uuid::Uuid>>>;

/// Resolve the user's IANA timezone for backup scheduling, falling back to UTC.
/// Mirrors `task_runner`'s per-trigger timezone handling so the backup cron
/// fires at the configured wall-clock time in the user's timezone — not UTC.
async fn backup_timezone(engine: &SharedEngine) -> chrono_tz::Tz {
    let tz = engine.user_timezone().await;
    if tz.is_empty() {
        return chrono_tz::UTC;
    }
    tz.parse().unwrap_or_else(|_| {
        log!(
            "[Scheduler] Invalid timezone '{}' for backup schedule, using UTC",
            tz
        );
        chrono_tz::UTC
    })
}

/// Register (or replace) the backup cron job, in the user's timezone. Removes any
/// previously-registered backup job first, so it is idempotent — calling it twice
/// with the same schedule converges on exactly one job.
///
/// `tokio_cron_scheduler::Job::new_async_tz` bakes a FIXED offset at registration
/// time (it is not natively DST-aware), so this MUST be re-run whenever the
/// offset can change: at engine startup (`load_backup_schedule`) and on a
/// `TimezoneSet` / backup-pref change (the EventBus subscriber) — see
/// `reload_backup_schedule`.
async fn register_backup_job(
    scheduler: &JobScheduler,
    backup_job_id: &BackupJobId,
    engine: &SharedEngine,
    cron_expr: &str,
    provider_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Remove any existing job first (lock held only to take the Uuid, never
    // across the await).
    let prev = backup_job_id.lock().unwrap().take();
    if let Some(job_id) = prev {
        scheduler.remove(&job_id).await?;
    }

    let tz = backup_timezone(engine).await;
    let engine_for_job = engine.clone();
    let provider_for_job = provider_id.to_string();

    let job = Job::new_async_tz(cron_expr, tz, move |_uuid, _lock| {
        let engine = engine_for_job.clone();
        let prov = provider_for_job.clone();
        Box::pin(async move {
            run_scheduled_backup(engine, prov).await;
        })
    })?;

    let job_id = scheduler.add(job).await?;
    *backup_job_id.lock().unwrap() = Some(job_id);
    log!(
        "[Scheduler] Registered backup job: {} (provider: {}, tz: {})",
        cron_expr,
        provider_id,
        tz
    );
    Ok(())
}

/// Remove the backup job if one is registered (used when the schedule is
/// disabled or no longer active).
async fn remove_backup_job(
    scheduler: &JobScheduler,
    backup_job_id: &BackupJobId,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let prev = backup_job_id.lock().unwrap().take();
    if let Some(job_id) = prev {
        scheduler.remove(&job_id).await?;
        log!("[Scheduler] Removed backup job");
    }
    Ok(())
}

/// Re-read the persisted backup schedule + provider and (re-)register the cron in
/// the user's CURRENT timezone — or remove it when the schedule is disabled/unset.
/// Idempotent. This is the single registration path shared by startup
/// (`load_backup_schedule`) and the EventBus subscriber (which calls it on a
/// `TimezoneSet` or a `backup_schedule` / `backup_provider` `PreferencesChanged`,
/// so an agent or HTTP write — or a timezone change — takes effect with no engine
/// restart).
pub(crate) async fn reload_backup_schedule(
    scheduler: &JobScheduler,
    backup_job_id: &BackupJobId,
    engine: &SharedEngine,
    pool: &PgPool,
) {
    use crate::core::backup::{is_schedule_active, PREF_BACKUP_PROVIDER, PREF_BACKUP_SCHEDULE};

    let cron = match PreferenceStore::get(pool, PREF_BACKUP_SCHEDULE).await {
        Ok(Some(c)) if is_schedule_active(&c) => c,
        _ => {
            // Disabled or unset — ensure no stale job lingers.
            if let Err(e) = remove_backup_job(scheduler, backup_job_id).await {
                log!("[Scheduler] Failed to remove backup job: {}", e);
            }
            return;
        }
    };
    let provider = match PreferenceStore::get(pool, PREF_BACKUP_PROVIDER).await {
        Ok(Some(p)) if !p.is_empty() => p,
        // Schedule is active but there is no provider to upload to — drop any
        // previously-registered job rather than leave a stale one that fires
        // with a now-absent provider.
        _ => {
            if let Err(e) = remove_backup_job(scheduler, backup_job_id).await {
                log!("[Scheduler] Failed to remove backup job: {}", e);
            }
            return;
        }
    };
    if let Err(e) = register_backup_job(scheduler, backup_job_id, engine, &cron, &provider).await {
        log!("[Scheduler] Failed to reload backup schedule: {}", e);
    }
}

mod task_runner;
use task_runner::{
    check_task_health_and_restart, handle_domain_event, handle_trigger_event, spawn_task_runner,
    TrackedTask,
};

/// Apply a trigger-group lifecycle event to the in-memory registry. Groups
/// don't schedule anything, so this is just a write to the shared map. Called
/// from the EventBus subscriber so every connected SSE consumer (and the
/// engine's own registry) converges on the same state.
///
/// The `create` and `rename` helpers in
/// [`crate::engine::trigger_group_writes`] also call this synchronously,
/// under [`crate::engine::LucidosEngine::trigger_group_write_lock`], so the
/// case-insensitive unique-name invariant survives concurrent requests — the
/// broadcast subscriber's apply happens too late for that. The reorder
/// handler (`POST /trigger-groups/reorder`) also calls this synchronously
/// after each emit, because callers expect a follow-up `GET /trigger-groups`
/// to reflect the new ordering. Delete still rides only on the broadcast
/// subscriber today (no read-after-write API contract on delete); if one is
/// added, route it through this same function as well.
pub(crate) fn handle_trigger_group_event(
    event_type: &str,
    group_id: &str,
    payload: &serde_json::Value,
    trigger_groups: &Arc<std::sync::RwLock<HashMap<String, TriggerGroup>>>,
) {
    match event_type {
        "TriggerGroupCreated" => {
            if let Ok(group) = TriggerGroup::from_created_payload(payload, chrono::Utc::now()) {
                let mut g = trigger_groups.write().unwrap();
                g.insert(group_id.to_string(), group);
            }
        }
        "TriggerGroupRenamed" => {
            let mut g = trigger_groups.write().unwrap();
            if let Some(group) = g.get_mut(group_id) {
                group.apply_renamed_payload(payload);
            }
        }
        "TriggerGroupReordered" => {
            let mut g = trigger_groups.write().unwrap();
            if let Some(group) = g.get_mut(group_id) {
                group.apply_reordered_payload(payload);
            }
        }
        "TriggerGroupDeleted" => {
            let mut g = trigger_groups.write().unwrap();
            g.remove(group_id);
        }
        _ => {}
    }
}

mod backup;
use backup::run_scheduled_backup;
pub(crate) use backup::{run_backup, BackupGuard};
use plugin_updates::{run_plugin_marketplace_update_check, MARKETPLACE_UPDATE_CHECK_CRON};

/// Flip `push_enabled` to false on every device whose `last_seen_at` is older
/// than `cutoff_days`. Devices already disabled are filtered at the SELECT
/// layer so a re-run produces no events. Engine-internal: emits `actor: None`.
pub(crate) async fn disable_push_on_stale_devices(
    engine: SharedEngine,
    pool: PgPool,
    cutoff_days: i64,
) {
    let stale = match crate::core::DeviceStore::list_stale_push_enabled(&pool, cutoff_days).await {
        Ok(ids) => ids,
        Err(e) => {
            log!(
                "[Scheduler] disable_push_on_stale_devices: list failed: {}",
                e
            );
            return;
        }
    };
    if stale.is_empty() {
        return;
    }
    log!(
        "[Scheduler] Disabling push on {} stale device(s) (>{}d)",
        stale.len(),
        cutoff_days
    );

    for device_id in stale {
        // `set_push_enabled` announces `DevicePushChanged` itself, and only when
        // a row actually flipped, so this sweep cannot report a device it missed.
        match crate::core::DeviceStore::set_push_enabled(
            &pool,
            &engine.event_bus,
            &device_id,
            false,
            None,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                // Disappeared between SELECT and UPDATE — benign race.
            }
            Err(e) => {
                log!("[Scheduler] set_push_enabled({}) failed: {}", device_id, e);
            }
        }
    }
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
                // Drop empty segments so a trailing/leading/double dash (e.g.
                // "foo-" -> ["foo", ""]) doesn't contribute a "" keyword that
                // `name_lower.contains("")` matches unconditionally. An
                // all-dash dir name yields no keywords and must not match
                // everything, so require at least one keyword.
                let dir_keywords: Vec<&str> =
                    dir_name.split('-').filter(|kw| !kw.is_empty()).collect();
                if !dir_keywords.is_empty() && dir_keywords.iter().all(|kw| name_lower.contains(kw))
                {
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

    #[test]
    fn find_matching_script_empty_segments_dont_match_all() {
        // A trailing dash splits to an empty segment; it must not contribute a
        // "" keyword that matches every trigger name. The real keyword still
        // constrains, and a name without it does not match.
        let dir = std::env::temp_dir().join("lucidos_test_find_script_trailing_dash");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("triggers/weather-/scripts")).unwrap();
        std::fs::write(dir.join("triggers/weather-/scripts/run.py"), "# w").unwrap();
        assert_eq!(
            find_matching_script(&dir, "weather report"),
            Some("triggers/weather-/scripts/run.py".to_string())
        );
        assert_eq!(find_matching_script(&dir, "unrelated task"), None);
        let _ = std::fs::remove_dir_all(&dir);

        // An all-dash dir name yields no keywords and must match nothing
        // (previously ["",""] -> contains("") -> matched everything).
        let dir2 = std::env::temp_dir().join("lucidos_test_find_script_all_dash");
        let _ = std::fs::remove_dir_all(&dir2);
        std::fs::create_dir_all(dir2.join("triggers/--/scripts")).unwrap();
        std::fs::write(dir2.join("triggers/--/scripts/run.py"), "# x").unwrap();
        assert_eq!(find_matching_script(&dir2, "literally anything"), None);
        let _ = std::fs::remove_dir_all(&dir2);
    }
}
