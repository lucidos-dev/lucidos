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
pub(super) async fn run_scheduled_backup(engine: SharedEngine, provider_id: String) {
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
            thread_id: None,
        }))
        .await
    {
        log!("[Backup] Failed to emit failure notification: {}", e);
    }

    push::send_push_to_all(pool, BACKUP_FAILURE_TITLE, error, Some(notification_id)).await;
}
