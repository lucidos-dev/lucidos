//! Backup primitives: the RAII guard, the actual run-backup pipeline (used by
//! both the manual API handler and the scheduled cron), and the failure
//! notification dedup helper.

use crate::api::SharedEngine;

use super::push;

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

/// Run the backup pipeline and emit terminal SSE events. Takes ownership of
/// the `BackupGuard` so it can clear the in-progress flag before the terminal
/// SSE — otherwise a status refetch triggered by the SSE races the flag and
/// briefly shows "Backup in progress" after the backup already finished.
pub(crate) async fn run_backup(
    guard: BackupGuard,
    engine: &SharedEngine,
    pool: &sqlx::PgPool,
    workspace: &std::path::Path,
    database_url: &str,
    key: &[u8],
    provider: &dyn crate::core::backup::BackupProvider,
) {
    use crate::core::backup;
    use crate::engine::event_bus::{BusEvent, SystemEvent};

    let progress = crate::api::backup::progress_sender(engine.event_bus.clone());

    // Capture start/finish so the persisted terminal event + last_run record the
    // run's duration (the durable backup history — see `BackupLastRun` /
    // `load_recent_runs`).
    let started_at = chrono::Utc::now();
    let result = backup::create_backup(workspace, database_url, key, provider, progress).await;
    let finished_at = chrono::Utc::now();

    match result {
        Ok(entry) => {
            log!(
                "[Backup] Completed: {} ({:.1} MB)",
                entry.filename,
                entry.size_bytes as f64 / 1024.0 / 1024.0
            );
            // Persist outcome and clear the in-progress flag BEFORE the
            // terminal SSE so any status refetch triggered by the event
            // sees both running=false and the fresh last_run.
            persist_last_run(pool, &backup::BackupLastRun::success(&entry, started_at)).await;
            drop(guard);
            engine
                .event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::BackupCompleted {
                        filename: entry.filename.clone(),
                        size_bytes: entry.size_bytes,
                        started_at,
                        finished_at,
                    }),
                    "[Backup] BackupCompleted",
                )
                .await;
            let keep = backup::get_retention_count(pool).await;
            // Scope pruning to THIS workspace's archives so a shared cloud
            // backup folder (multiple workspaces, one account) never has one
            // workspace evict another's backups.
            let workspace_name = backup::workspace_archive_name(workspace);
            if let Err(e) = backup::prune_old_backups(provider, workspace_name, keep).await {
                log!("[Backup] Pruning failed (non-fatal): {}", e);
            }
        }
        Err(e) => {
            let msg = e.to_string();
            log!("[Backup] Failed: {}", msg);
            persist_last_run(pool, &backup::BackupLastRun::failure(&msg, started_at)).await;
            drop(guard);
            engine
                .event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::BackupFailed {
                        error: msg.clone(),
                        started_at,
                        finished_at,
                    }),
                    "[Backup] BackupFailed",
                )
                .await;
            notify_backup_failure(engine, &msg).await;
        }
    }
}

/// Persist the last-run outcome, logging (never crashing the backup) on
/// failure. Wraps `backup::persist_last_run` so the `run_backup` arms stay
/// terse and both paths get identical error handling.
async fn persist_last_run(pool: &sqlx::PgPool, run: &crate::core::backup::BackupLastRun) {
    if let Err(e) = crate::core::backup::persist_last_run(pool, run).await {
        log!("[Backup] Failed to persist last-run outcome: {}", e);
    }
}

/// Execute a scheduled backup. Called by the cron job.
pub(super) async fn run_scheduled_backup(engine: SharedEngine, provider_id: String) {
    use crate::core::backup::{self, crypto};

    let Some(guard) = BackupGuard::try_acquire(&engine) else {
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

    // Ensure an encryption key exists, generating + persisting one if absent —
    // the exact same `crypto::ensure_key` the manual / activation path uses, so
    // the two can never produce different key formats or locations. A scheduled
    // backup must never silently skip just because the user hasn't triggered a
    // manual backup first.
    let key = match crypto::ensure_key(&workspace) {
        Ok((k, is_new)) => {
            if is_new {
                log!(
                    "[Backup] No backup key found; auto-generated a new encryption key for the scheduled backup"
                );
                // The user has never seen this key — it was created unattended by
                // the cron, not by them clicking through Settings → Backup. Tell
                // them to store it safely (it can't be recovered and is required
                // to restore), deep-linking the tap to the page where they can
                // view + copy it.
                notify_backup_key_generated(&engine).await;
            }
            k
        }
        Err(e) => {
            log!("[Backup] Failed to load or generate key: {}", e);
            notify_backup_failure(
                &engine,
                &format!("Failed to load or generate backup key: {}", e),
            )
            .await;
            return;
        }
    };

    let database_url = crate::core::database_url();
    run_backup(
        guard,
        &engine,
        pool,
        &workspace,
        &database_url,
        &key,
        provider.as_ref(),
    )
    .await;
}

const BACKUP_KEY_GENERATED_TITLE: &str = "Backup key created — store it safely";

/// Emit a backup notification (DB row + SSE) and fan it out to every device via
/// web push. Shared by the failure and key-generated paths so both surface
/// identically and differ only in title / message / tap destination.
async fn emit_backup_notification(
    engine: &SharedEngine,
    title: &str,
    message: &str,
    tap: crate::scheduler::notifications::Tap,
) {
    use crate::engine::event_bus::{BusEvent, SystemEvent};

    let notification_id = uuid::Uuid::new_v4();

    if let Err(e) = engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::NotificationCreated {
            id: notification_id.to_string(),
            title: title.to_string(),
            message: message.to_string(),
            task_id: None,
            app_id: None,
            thread_id: None,
            event_id: None,
            tap,
            actor: None,
        }))
        .await
    {
        log!("[Backup] Failed to emit notification: {}", e);
    }

    push::send_push_to_all(engine, title, message, Some(notification_id)).await;
}

/// Notify the user that the scheduled backup auto-generated a fresh encryption
/// key. Unlike the manual flow, the user never saw this key as it was created,
/// so they must be told to store it safely — it cannot be recovered and is
/// required to restore. The tap deep-links to Settings → Backup, where the key
/// can be revealed and copied (same destination as the LLM's `navigate_ui`).
async fn notify_backup_key_generated(engine: &SharedEngine) {
    use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};

    const MESSAGE: &str = "Your scheduled backup created a new encryption key. \
        Store it somewhere safe — you need it to restore, and it cannot be recovered. \
        Open Settings → Backup to view and copy it.";

    emit_backup_notification(
        engine,
        BACKUP_KEY_GENERATED_TITLE,
        MESSAGE,
        Tap::Navigate {
            to: NavigateUi {
                target: NavigateTarget::Settings,
                settings_view: Some("backup".to_string()),
                ..Default::default()
            },
        },
    )
    .await;
}

const BACKUP_FAILURE_TITLE: &str = "Backup failed";
const BACKUP_FAILURE_DEDUP_MINUTES: i64 = 30;

/// Deduplicates backup failure notifications (max 1 per 30 minutes).
pub(crate) async fn notify_backup_failure(engine: &SharedEngine, error: &str) {
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

    emit_backup_notification(
        engine,
        BACKUP_FAILURE_TITLE,
        error,
        crate::scheduler::notifications::Tap::Modal,
    )
    .await;
}
