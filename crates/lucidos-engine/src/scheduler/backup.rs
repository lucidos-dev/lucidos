use crate::core::backup::crypto::SealingKey;
use crate::core::backup::recipient;
use crate::core::keep_awake::{self, AwakeHold, Work};
use crate::core::snapshot::{ArchiveJobGuard, ArchiveJobSlot, Busy};
use crate::engine::types::SharedEngine;
use crate::scheduler::notifications::SettingsPage;
use tokio_util::sync::CancellationToken;

use super::push;

pub struct BackupGuard {
    _slot: ArchiveJobGuard,
    _awake: AwakeHold,
}

impl BackupGuard {
    pub fn try_acquire(engine: &SharedEngine) -> Option<Self> {
        engine
            .archive_job_slot
            .try_acquire(crate::core::snapshot::ArchiveJobKind::Backup)
            .map(Self::holding)
    }

    fn holding(slot: ArchiveJobGuard) -> Self {
        Self {
            _slot: slot,
            _awake: keep_awake::hold(Work::Backup, "backup"),
        }
    }
}

const SNAPSHOT_WAIT: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

async fn slot_for_schedule(
    slot: &ArchiveJobSlot,
    limit: std::time::Duration,
    cancel: &CancellationToken,
) -> Option<Result<ArchiveJobGuard, Busy>> {
    tokio::select! {
        acquired = slot.acquire_for_backup(limit) => (!cancel.is_cancelled()).then_some(acquired),
        _ = cancel.cancelled() => None,
    }
}

/// INVARIANT: Run the backup pipeline and emit terminal SSE events. Takes ownership of
/// the `BackupGuard` so it can clear the in-progress flag before the terminal
/// SSE — otherwise a status refetch triggered by the SSE races the flag and
/// briefly shows "Backup in progress" after the backup already finished.
pub async fn run_backup(
    guard: BackupGuard,
    engine: &SharedEngine,
    pool: &sqlx::PgPool,
    workspace: &std::path::Path,
    database_url: &str,
    key: SealingKey,
    provider: &dyn crate::core::backup::BackupProvider,
) {
    use crate::core::backup;
    use crate::engine::event_bus::{BusEvent, SystemEvent};

    let progress = progress_sender(engine.event_bus.clone());

    let started_at = chrono::Utc::now();
    let monotonic_start = std::time::Instant::now();
    let result =
        backup::create_backup(pool, workspace, database_url, key, provider, progress).await;
    let finished_at = chrono::Utc::now();

    match result {
        Ok((entry, origin)) => {
            log_legacy!(
                "[Backup] Completed: {} ({:.1} MB)",
                entry.filename,
                entry.size_bytes as f64 / 1024.0 / 1024.0
            );
            // INVARIANT: Persist outcome and clear the in-progress flag BEFORE the
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
            // INVARIANT: A retention count we could not READ must not authorize deleting
            // anything: the old default-on-error pruned down to 5 on a pool
            // timeout, discarding up to 45 archives of a workspace configured
            // for 50. Skipping costs one un-pruned run, which the next
            // successful backup cleans up.
            match backup::get_retention_count(pool).await {
                Ok(keep) => {
                    let workspace_name = backup::workspace_archive_name(workspace);
                    if let Err(e) =
                        backup::prune_old_backups(provider, workspace_name, &origin, keep).await
                    {
                        log_legacy!("[Backup] Pruning failed (non-fatal): {}", e);
                    }
                }
                Err(e) => log_legacy!(
                    "[Backup] Could not read the retention count ({}); skipping pruning this run rather than defaulting to {}",
                    e,
                    crate::core::prefs::BACKUP_RETENTION.default_number()
                ),
            }
        }
        Err(e) => {
            let slept = time_asleep(finished_at - started_at, monotonic_start.elapsed());
            let msg = describe_backup_failure(&e.to_string(), slept);
            log_legacy!("[Backup] Failed: {}", msg);
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
            notify_backup_failure(engine, provider.id(), &msg).await;
        }
    }
}

const SLEEP_NOTE_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(60);

fn time_asleep(
    wall: chrono::Duration,
    monotonic: std::time::Duration,
) -> Option<std::time::Duration> {
    let wall = wall.to_std().ok()?;
    wall.checked_sub(monotonic)
        .filter(|gap| *gap >= SLEEP_NOTE_THRESHOLD)
}

fn describe_backup_failure(error: &str, slept: Option<std::time::Duration>) -> String {
    match slept {
        Some(gap) => format!(
            "The computer slept for {} during this backup. {error}",
            spoken_duration(gap)
        ),
        None => error.to_string(),
    }
}

fn spoken_duration(d: std::time::Duration) -> String {
    let total = ((d.as_secs() + 30) / 60).max(1);
    let unit = |n: u64, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    let (hours, minutes) = (total / 60, total % 60);
    match (hours, minutes) {
        (0, m) => unit(m, "minute"),
        (h, 0) => unit(h, "hour"),
        (h, m) => format!("{} {}", unit(h, "hour"), unit(m, "minute")),
    }
}

async fn persist_last_run(pool: &sqlx::PgPool, run: &crate::core::backup::BackupLastRun) {
    if let Err(e) = crate::core::backup::persist_last_run(pool, run).await {
        log_legacy!("[Backup] Failed to persist last-run outcome: {}", e);
    }
}

pub fn backups_not_set_up_message() -> String {
    format!(
        "Backups are not set up yet. Open {} and create a backup key.",
        SettingsPage::BACKUP.link()
    )
}

pub(super) async fn run_scheduled_backup(
    engine: SharedEngine,
    provider_id: String,
    cancel: &CancellationToken,
) {
    use crate::core::backup;

    let Some(acquired) = slot_for_schedule(&engine.archive_job_slot, SNAPSHOT_WAIT, cancel).await
    else {
        return;
    };
    if engine.is_shutting_down() {
        return;
    }
    let guard = match acquired {
        Ok(slot) => BackupGuard::holding(slot),
        Err(Busy::Backup) => {
            log_legacy!("[Backup] Skipping scheduled backup: another backup is already running");
            return;
        }
        Err(Busy::Snapshot) => {
            let msg = "A workspace snapshot job held the archive slot for six hours, so the scheduled backup did not run.";
            log_legacy!("[Backup] {}", msg);
            notify_backup_failure(&engine, &provider_id, msg).await;
            return;
        }
    };

    log_legacy!(
        "[Backup] Starting scheduled backup (provider: {})",
        provider_id
    );

    let pool = engine.pool();
    let workspace = engine.workspace_path().to_path_buf();

    let provider = match backup::get_provider(&provider_id, pool) {
        Ok(p) => p,
        Err(e) => {
            log_legacy!("[Backup] {}, skipping", e);
            notify_backup_failure(&engine, &provider_id, &e.to_string()).await;
            return;
        }
    };

    let state = match recipient::load(pool).await {
        Ok(state) => state,
        Err(e) => {
            let msg = format!("Cannot read the backup recipient: {e}");
            log_legacy!("[Backup] {}", msg);
            notify_backup_failure(&engine, &provider_id, &msg).await;
            return;
        }
    };
    let key = match recipient::sealing_key(pool, &workspace).await {
        Ok(Some(key)) => key,
        Ok(None) => {
            log_legacy!("[Backup] Skipping scheduled backup: backups are not set up");
            notify_once(
                &engine,
                BACKUPS_NOT_SET_UP_TITLE,
                &backups_not_set_up_message(),
                DAY_SECS,
            )
            .await;
            return;
        }
        Err(e) => {
            let msg = format!("Cannot load the backup key: {e}");
            log_legacy!("[Backup] {}", msg);
            notify_backup_failure(&engine, &provider_id, &msg).await;
            return;
        }
    };
    // INVARIANT: A pending switch never pauses a backup. It only asks, once a week.
    match &key {
        SealingKey::Legacy(_) => {
            notify_once(
                &engine,
                SWITCH_PENDING_TITLE,
                &switch_pending_message(),
                WEEK_SECS,
            )
            .await
        }
        SealingKey::Recipient(_) if verify_is_due(&state) => {
            notify_once(
                &engine,
                VERIFY_DUE_TITLE,
                &verify_due_message(),
                VERIFY_REMINDER_SECS,
            )
            .await
        }
        SealingKey::Recipient(_) => {}
    }

    let database_url = crate::core::database_url();
    run_backup(
        guard,
        &engine,
        pool,
        &workspace,
        &database_url,
        key,
        provider.as_ref(),
    )
    .await;
}

const DAY_SECS: i64 = 24 * 60 * 60;
const WEEK_SECS: i64 = 7 * DAY_SECS;
const VERIFY_REMINDER_SECS: i64 = 365 * DAY_SECS;

const BACKUPS_NOT_SET_UP_TITLE: &str = "Backups are not set up";
const SWITCH_PENDING_TITLE: &str = "Switch your backups to a new backup key";
const VERIFY_DUE_TITLE: &str = "Check that you still have your backup key";

fn verify_is_due(state: &recipient::RecipientState) -> bool {
    state
        .secs_unproven()
        .is_some_and(|secs| secs >= VERIFY_REMINDER_SECS)
}

fn switch_pending_message() -> String {
    format!(
        "Your backups still use the old backup key. With a new backup key, nothing on \
         this computer can open your backups. Open {} to switch.",
        SettingsPage::BACKUP.link()
    )
}

fn verify_due_message() -> String {
    format!(
        "A backup is only as good as the key that opens it. Open {} and paste your \
         backup key to check you still have it.",
        SettingsPage::BACKUP.link()
    )
}

async fn notify_once(engine: &SharedEngine, title: &str, message: &str, window_secs: i64) {
    if notified_within(engine, title, window_secs).await {
        return;
    }
    emit_backup_notification(engine, title, message, SettingsPage::BACKUP.tap()).await;
}

async fn notified_within(engine: &SharedEngine, title: &str, window_secs: i64) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM notifications \
         WHERE title = $1 AND created_at > now() - make_interval(secs => $2))",
    )
    .bind(title)
    .bind(window_secs as f64)
    .fetch_one(engine.pool())
    .await
    .unwrap_or_else(|e| {
        log_legacy!("[Backup] Notification dedup lookup failed: {}", e);
        false
    })
}

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
            audience: None,
            push_group: None,
        }))
        .await
    {
        log_legacy!("[Backup] Failed to emit notification: {}", e);
    }

    push::send_push_to_all(engine, title, message, Some(notification_id));
}

fn backup_failure_page(readiness: Option<&crate::core::backup::ProviderReadiness>) -> SettingsPage {
    match readiness {
        Some(r) if !r.connected => SettingsPage::ACCOUNTS,
        _ => SettingsPage::BACKUP,
    }
}

const BACKUP_FAILURE_TITLE: &str = "Backup failed";
const BACKUP_FAILURE_DEDUP_SECS: i64 = 30 * 60;

/// INVARIANT: Compose the failure notification's body: what to do about it, then why it
/// happened.
///
/// The remedy comes first because the error alone is a dead end. A user whose
/// nightly Dropbox backup reported "OAuth token expired but no refresh token
/// available" had to ask a human what to do with that, and the answer, press
/// *Grant access* on the Backup page, was nowhere on the notification or the
/// card it opened.
///
/// **The remedy is chosen from the readiness verdict, never by matching the
/// error text.** `provider_readiness` is the same function the Backup page's
/// connected / ready state comes from, so the notification and the page cannot
/// disagree; a substring match on the error would be a second definition of the
/// same question, drifting the moment a provider reworded a message.
///
/// `readiness` is `None` when the verdict could not be resolved (an unknown
/// provider id, or a DB error on the lookup). That falls back to the destination
/// alone, which is right for every cause.
fn backup_failure_body(
    provider_name: Option<&str>,
    readiness: Option<&crate::core::backup::ProviderReadiness>,
    error: &str,
) -> String {
    let who = provider_name.unwrap_or("Your backup provider");
    let page = backup_failure_page(readiness).link();
    let remedy = match readiness {
        Some(r) if !r.connected => {
            format!("{who} has no connected account, so nothing can upload. Connect it in {page}.")
        }
        Some(r) if !r.ready() => format!(
            "{who} is connected but has not granted the permissions a backup needs. \
             Open {page} and press Grant access."
        ),
        _ => format!("Open {page} to see the details and retry."),
    };
    format!("{remedy}\n\n{error}")
}

/// INVARIANT: Notify the user that a backup failed, with what to do about it.
///
/// Deduplicated to at most one per 30 minutes. The dedup query keys on
/// [`BACKUP_FAILURE_TITLE`], so the title is a single constant for every cause
/// and only the body varies: a title that named the cause would let a
/// cause-alternating failure notify on every single run.
pub(crate) async fn notify_backup_failure(engine: &SharedEngine, provider_id: &str, error: &str) {
    if notified_within(engine, BACKUP_FAILURE_TITLE, BACKUP_FAILURE_DEDUP_SECS).await {
        return;
    }
    let pool = engine.pool();

    let meta = crate::core::backup::provider_meta(provider_id);
    let readiness = match meta.as_ref() {
        Some(m) => match crate::core::backup::provider_readiness(pool, m).await {
            Ok(r) => Some(r),
            Err(e) => {
                log_legacy!(
                    "[Backup] Could not resolve {} readiness for the failure notification: {}",
                    provider_id,
                    e
                );
                None
            }
        },
        None => None,
    };

    let body = backup_failure_body(meta.as_ref().map(|m| m.name), readiness.as_ref(), error);

    emit_backup_notification(
        engine,
        BACKUP_FAILURE_TITLE,
        &body,
        backup_failure_page(readiness.as_ref()).tap(),
    )
    .await;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BackupCatchUpFloor {
    pub(super) last_completed_start: Option<chrono::DateTime<chrono::Utc>>,
    /// INVARIANT: When the schedule preference was last written, on the database clock.
    pub(super) schedule_written: Option<chrono::DateTime<chrono::Utc>>,
    pub(super) restored_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl BackupCatchUpFloor {
    pub(super) fn floor(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.last_completed_start
            .max(self.schedule_written)
            .max(self.restored_at)
    }
}

pub(super) async fn load_backup_catch_up_floor(
    pool: &sqlx::PgPool,
) -> Result<BackupCatchUpFloor, Box<dyn std::error::Error + Send + Sync>> {
    let last_completed_start = crate::core::backup::load_last_successful_backup_run(pool)
        .await?
        .map(|run| run.started_at.unwrap_or(run.finished_at));
    let schedule_written = crate::core::prefs::BACKUP_SCHEDULE
        .try_stored_written_at(pool)
        .await?;
    let restored_at = crate::core::snapshot::boundary::read(pool)
        .await?
        .map(|boundary| boundary.restored_at);
    Ok(BackupCatchUpFloor {
        last_completed_start,
        schedule_written,
        restored_at,
    })
}

pub fn progress_sender(
    event_bus: crate::engine::event_bus::EventBus,
) -> impl Fn(&str, usize, usize) + Send + Sync + 'static {
    move |phase: &str, current: usize, total: usize| {
        // INVARIANT: Broadcast synchronously from both async and blocking callers.
        // Detached sends could deliver progress after completion or failure and
        // leave the Backup card showing an active upload.
        event_bus.broadcast_transient_system(
            crate::engine::event_bus::SystemEvent::BackupProgress {
                phase: phase.to_string(),
                progress: current,
                total,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        backup_failure_body, backup_failure_page, backups_not_set_up_message,
        describe_backup_failure, load_backup_catch_up_floor, spoken_duration,
        switch_pending_message, time_asleep, verify_due_message, verify_is_due,
        BACKUP_FAILURE_TITLE,
    };
    use std::time::Duration;

    #[tokio::test]
    async fn the_catch_up_floor_reads_the_last_completed_start_and_the_schedule_write() {
        use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let at = |s: &str| {
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .with_timezone(&chrono::Utc)
        };

        let empty = load_backup_catch_up_floor(&pool).await.unwrap();
        assert_eq!(empty.last_completed_start, None);
        assert_eq!(empty.schedule_written, None);
        assert_eq!(empty.restored_at, None);

        let completed = at("2026-11-01T02:00:01Z");
        bus.emit(BusEvent::System(SystemEvent::BackupCompleted {
            filename: "a.tar.gz".into(),
            size_bytes: 1,
            started_at: completed,
            finished_at: completed + chrono::Duration::minutes(3),
        }))
        .await
        .unwrap();
        bus.emit(BusEvent::System(SystemEvent::BackupFailed {
            error: "offline".into(),
            started_at: at("2026-11-02T02:00:01Z"),
            finished_at: at("2026-11-02T02:00:09Z"),
        }))
        .await
        .unwrap();
        crate::core::PreferenceStore::set(
            &pool,
            &bus,
            crate::core::prefs::BACKUP_SCHEDULE.key(),
            "0 0 3 * * *",
            None,
        )
        .await
        .unwrap();

        let floor = load_backup_catch_up_floor(&pool).await.unwrap();
        assert_eq!(floor.last_completed_start, Some(completed));
        let written = floor.schedule_written.expect("the write is recorded");
        assert!(written > chrono::Utc::now() - chrono::Duration::minutes(5));

        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_cancelled_schedule_stops_waiting_for_the_slot() {
        use crate::core::snapshot::{ArchiveJobKind, ArchiveJobSlot};
        let slot = ArchiveJobSlot::default();
        let cancel = tokio_util::sync::CancellationToken::new();
        let capture = slot.try_acquire(ArchiveJobKind::SnapshotCapture).unwrap();
        let waiting = tokio::spawn({
            let (slot, cancel) = (slot.clone(), cancel.clone());
            async move {
                super::slot_for_schedule(&slot, Duration::from_secs(60), &cancel)
                    .await
                    .is_none()
            }
        });
        tokio::task::yield_now().await;
        cancel.cancel();
        assert!(waiting.await.unwrap(), "a cancelled wait takes no slot");
        drop(capture);
        assert!(!slot.is_busy());
    }

    #[test]
    fn a_run_that_spanned_a_sleep_reports_the_gap() {
        let wall = chrono::Duration::seconds(24 * 60 + 49);
        let ran = Duration::from_secs(11 * 60 + 40);
        assert_eq!(
            time_asleep(wall, ran),
            Some(Duration::from_secs(13 * 60 + 9))
        );
    }

    #[test]
    fn a_run_that_never_slept_reports_nothing() {
        let ran = Duration::from_secs(300);
        assert_eq!(time_asleep(chrono::Duration::seconds(301), ran), None);
        assert_eq!(time_asleep(chrono::Duration::seconds(359), ran), None);
        assert_eq!(time_asleep(chrono::Duration::seconds(200), ran), None);
        assert_eq!(time_asleep(chrono::Duration::seconds(-5), ran), None);
    }

    #[test]
    fn the_sleep_note_leads_and_the_error_survives() {
        const ERROR: &str = "Lost the connection to Google Drive while starting the upload.";
        assert_eq!(describe_backup_failure(ERROR, None), ERROR);
        let noted = describe_backup_failure(ERROR, Some(Duration::from_secs(13 * 60 + 9)));
        assert_eq!(
            noted,
            format!("The computer slept for 13 minutes during this backup. {ERROR}")
        );
    }

    #[test]
    fn a_sleep_is_spoken_in_whole_minutes_and_hours() {
        assert_eq!(spoken_duration(Duration::from_secs(60)), "1 minute");
        assert_eq!(spoken_duration(Duration::from_secs(89)), "1 minute");
        assert_eq!(spoken_duration(Duration::from_secs(90)), "2 minutes");
        assert_eq!(spoken_duration(Duration::from_secs(2 * 3600)), "2 hours");
        assert_eq!(
            spoken_duration(Duration::from_secs(3600 + 5 * 60)),
            "1 hour 5 minutes"
        );
    }
    use crate::core::backup::ProviderReadiness;
    use crate::scheduler::notifications::{NavigateTarget, Tap};

    fn backup_failure_tap(readiness: Option<&ProviderReadiness>) -> Tap {
        backup_failure_page(readiness).tap()
    }

    fn tapped_view(tap: Tap) -> Option<String> {
        match tap {
            Tap::Navigate { to } => {
                assert_eq!(to.target, NavigateTarget::Settings);
                to.settings_view
            }
            Tap::Modal | Tap::RestoreSnapshot { .. } | Tap::UndoConfigChange { .. } => None,
        }
    }

    fn connected_not_ready() -> ProviderReadiness {
        ProviderReadiness {
            connected: true,
            missing_scopes: vec!["files.metadata.read"],
        }
    }
    fn ready() -> ProviderReadiness {
        ProviderReadiness {
            connected: true,
            missing_scopes: Vec::new(),
        }
    }
    fn not_connected() -> ProviderReadiness {
        ProviderReadiness {
            connected: false,
            missing_scopes: Vec::new(),
        }
    }

    #[test]
    fn a_connected_but_unready_provider_is_told_to_grant_access() {
        let body = backup_failure_body(
            Some("Dropbox"),
            Some(&connected_not_ready()),
            "OAuth token expired but no refresh token available",
        );
        assert!(body.contains("Grant access"), "{body}");
        assert!(body.contains("Backup"), "{body}");
        assert!(body.contains("Dropbox"), "{body}");
    }

    #[test]
    fn an_unconnected_provider_is_sent_to_accounts() {
        let body = backup_failure_body(Some("Dropbox"), Some(&not_connected()), "no account");
        assert!(body.contains("Accounts"), "{body}");
        assert!(
            !body.contains("Grant access"),
            "nothing to grant without an account: {body}"
        );
    }

    #[test]
    fn every_remedy_links_the_page_its_tap_opens() {
        for readiness in [
            Some(&connected_not_ready()),
            Some(&ready()),
            Some(&not_connected()),
            None,
        ] {
            let body = backup_failure_body(Some("Dropbox"), readiness, "e");
            let view = tapped_view(backup_failure_tap(readiness))
                .unwrap_or_else(|| panic!("{readiness:?} must deep-link, not open a modal"));
            let link = match view.as_str() {
                "accounts" => "[Settings → Accounts](settings:accounts)",
                "backup" => "[Settings → System → Backup](settings:backup)",
                other => panic!("unexpected destination {other}"),
            };
            assert!(
                body.contains(link),
                "{readiness:?} taps through to {view} but the body does not link it: {body}"
            );
        }
    }

    #[test]
    fn only_the_unconnected_branch_lands_on_accounts() {
        assert_eq!(
            tapped_view(backup_failure_tap(Some(&not_connected()))).as_deref(),
            Some("accounts")
        );
        for readiness in [Some(&connected_not_ready()), Some(&ready()), None] {
            assert_eq!(
                tapped_view(backup_failure_tap(readiness)).as_deref(),
                Some("backup"),
                "{readiness:?}"
            );
        }
    }

    #[test]
    fn a_ready_provider_gets_the_destination_without_invented_advice() {
        let body = backup_failure_body(Some("Dropbox"), Some(&ready()), "upload timed out");
        assert!(body.contains("Backup"), "{body}");
        assert!(!body.contains("Grant access"), "{body}");
        assert!(!body.contains("Accounts"), "{body}");
    }

    #[test]
    fn an_unresolved_verdict_falls_back_without_losing_the_error() {
        let body = backup_failure_body(None, None, "some failure");
        assert!(body.contains("Backup"), "{body}");
        assert!(body.contains("some failure"), "{body}");
        assert!(!body.contains("Grant access"), "{body}");
    }

    #[test]
    fn every_branch_keeps_the_underlying_error() {
        const ERROR: &str = "OAuth token expired but no refresh token available";
        for readiness in [
            Some(&connected_not_ready()),
            Some(&ready()),
            Some(&not_connected()),
            None,
        ] {
            let body = backup_failure_body(Some("Dropbox"), readiness, ERROR);
            assert!(body.contains(ERROR), "{readiness:?} lost the error: {body}");
        }
    }

    #[test]
    fn the_title_is_one_constant_so_dedup_still_collapses_repeats() {
        assert_eq!(BACKUP_FAILURE_TITLE, "Backup failed");
        let bodies: Vec<String> = [Some(&connected_not_ready()), Some(&ready()), None]
            .into_iter()
            .map(|r| backup_failure_body(Some("Dropbox"), r, "e"))
            .collect();
        assert_eq!(bodies.len(), 3);
    }

    #[test]
    fn an_unknown_provider_is_not_named_by_its_raw_id() {
        let body = backup_failure_body(None, Some(&connected_not_ready()), "e");
        assert!(!body.contains("google_drive"), "{body}");
        assert!(body.starts_with("Your backup provider"), "{body}");
    }

    #[test]
    fn every_backup_remedy_links_the_system_step() {
        const LINK: &str = "[Settings → System → Backup](settings:backup)";
        for readiness in [Some(&connected_not_ready()), Some(&ready()), None] {
            let body = backup_failure_body(Some("Dropbox"), readiness, "e");
            assert!(body.contains(LINK), "{readiness:?}: {body}");
        }
        for message in [
            backups_not_set_up_message(),
            switch_pending_message(),
            verify_due_message(),
        ] {
            assert!(message.contains(LINK), "{message}");
        }
    }

    #[test]
    fn the_verify_reminder_waits_a_year() {
        use crate::core::backup::recipient::RecipientState;
        let state = |secs| RecipientState {
            secs_since_verified: secs,
            ..RecipientState::default()
        };
        assert!(!verify_is_due(&state(None)));
        assert!(!verify_is_due(&state(Some(364 * 24 * 3600))));
        assert!(verify_is_due(&state(Some(365 * 24 * 3600))));
        let own_key = RecipientState {
            secs_since_set: Some(365 * 24 * 3600),
            ..RecipientState::default()
        };
        assert!(verify_is_due(&own_key));
    }
}
