//! Backup primitives: the RAII guard, the actual run-backup pipeline (used by
//! both the manual API handler and the scheduled cron), and the failure
//! notification dedup helper.

use crate::api::SharedEngine;
use crate::core::keep_awake::{self, AwakeHold, Work};
use crate::scheduler::notifications::SettingsPage;

use super::push;

/// RAII guard for `engine.backup_in_progress`. Acquired atomically so two
/// concurrent backup attempts can't both pass the check; cleared on drop so
/// a panic mid-backup doesn't permanently strand the flag. It keeps the
/// computer awake for as long as it lives.
pub(crate) struct BackupGuard {
    engine: SharedEngine,
    _awake: AwakeHold,
}

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
            .map(|_| Self {
                engine: engine.clone(),
                _awake: keep_awake::hold(Work::Backup, "backup"),
            })
    }
}

impl Drop for BackupGuard {
    fn drop(&mut self) {
        self.engine
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
    let monotonic_start = std::time::Instant::now();
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
            // A retention count we could not READ must not authorize deleting
            // anything: the old default-on-error pruned down to 5 on a pool
            // timeout, discarding up to 45 archives of a workspace configured
            // for 50. Skipping costs one un-pruned run, which the next
            // successful backup cleans up.
            match backup::get_retention_count(pool).await {
                Ok(keep) => {
                    // Scope pruning to THIS workspace's archives so a shared
                    // cloud backup folder (multiple workspaces, one account)
                    // never has one workspace evict another's backups.
                    let workspace_name = backup::workspace_archive_name(workspace);
                    if let Err(e) = backup::prune_old_backups(provider, workspace_name, keep).await
                    {
                        log!("[Backup] Pruning failed (non-fatal): {}", e);
                    }
                }
                Err(e) => log!(
                    "[Backup] Could not read the retention count ({}); skipping pruning this run rather than defaulting to {}",
                    e,
                    crate::core::prefs::BACKUP_RETENTION.default_number()
                ),
            }
        }
        Err(e) => {
            let slept = time_asleep(finished_at - started_at, monotonic_start.elapsed());
            let msg = describe_backup_failure(&e.to_string(), slept);
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
            notify_backup_failure(engine, provider.id(), &msg).await;
        }
    }
}

/// Shortest sleep worth naming in a failure. Below it, the gap between the two
/// clocks is scheduling noise, not a computer that slept.
const SLEEP_NOTE_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(60);

/// How long the computer slept during a run, when it slept long enough to name.
///
/// Wall-clock time keeps moving while the computer sleeps. `Instant` does not,
/// on macOS or Linux. So the difference is the time spent asleep.
fn time_asleep(
    wall: chrono::Duration,
    monotonic: std::time::Duration,
) -> Option<std::time::Duration> {
    let wall = wall.to_std().ok()?;
    wall.checked_sub(monotonic)
        .filter(|gap| *gap >= SLEEP_NOTE_THRESHOLD)
}

/// The failure as the user reads it. A sleep is named first, because a
/// connection that died while the computer slept explains the error under it.
/// The note states only what the clocks measured.
fn describe_backup_failure(error: &str, slept: Option<std::time::Duration>) -> String {
    match slept {
        Some(gap) => format!(
            "The computer slept for {} during this backup. {error}",
            spoken_duration(gap)
        ),
        None => error.to_string(),
    }
}

/// `1 minute`, `13 minutes`, `2 hours`, `1 hour 5 minutes`. Rounded to the
/// nearest minute, and never below one.
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

/// Persist the last-run outcome, logging (never crashing the backup) on
/// failure. Wraps `backup::persist_last_run` so the `run_backup` arms stay
/// terse and both paths get identical error handling.
async fn persist_last_run(pool: &sqlx::PgPool, run: &crate::core::backup::BackupLastRun) {
    if let Err(e) = crate::core::backup::persist_last_run(pool, run).await {
        log!("[Backup] Failed to persist last-run outcome: {}", e);
    }
}

/// Load the workspace's backup key, creating one if absent, and tell the user
/// when this call created it.
///
/// The key is excluded from every archive, so a user who never saw it cannot
/// restore after losing the machine. Every path that mints a key the user is
/// not looking at goes through here: the scheduled run, a manual backup and
/// turning a schedule on. Only `POST /backup/key` calls `ensure_key` directly,
/// because it hands the new key straight to the user's screen.
pub(crate) async fn ensure_backup_key(
    engine: &SharedEngine,
    workspace: &std::path::Path,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let (key, is_new) = crate::core::backup::crypto::ensure_key(workspace)?;
    if is_new {
        log!("[Backup] No backup key found; generated a new encryption key");
        notify_backup_key_generated(engine).await;
    }
    Ok(key)
}

/// Execute a scheduled backup. Called by the cron job.
pub(super) async fn run_scheduled_backup(engine: SharedEngine, provider_id: String) {
    use crate::core::backup;

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
            notify_backup_failure(&engine, &provider_id, &e.to_string()).await;
            return;
        }
    };

    // A scheduled backup must never silently skip just because the user hasn't
    // triggered a manual backup first.
    let key = match ensure_backup_key(&engine, &workspace).await {
        Ok(k) => k,
        Err(e) => {
            log!("[Backup] Failed to load or generate key: {}", e);
            notify_backup_failure(
                &engine,
                &provider_id,
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

    push::send_push_to_all(engine, title, message, Some(notification_id));
}

/// Where a failure notification sends the reader, which is NOT always the
/// Backup page. The body links this page and the tap opens it, so the two
/// cannot disagree.
///
/// For a provider with no account the remedy is *connect it*, and connecting
/// happens only in Settings → Accounts: the Backup page has no account UI, and
/// `system-knowhow/backups.md` is emphatic that sending a user there to connect
/// is how this flow goes wrong. Every other cause is a Backup-page matter: it
/// carries the health card, the error and the *Grant access* button.
fn backup_failure_page(readiness: Option<&crate::core::backup::ProviderReadiness>) -> SettingsPage {
    match readiness {
        Some(r) if !r.connected => SettingsPage::ACCOUNTS,
        _ => SettingsPage::BACKUP,
    }
}

/// The key-generated body. It links the Backup page, where the key can be
/// revealed and copied.
fn backup_key_generated_message() -> String {
    format!(
        "Lucidos created a new encryption key for your backups. \
         Store it somewhere safe: you need it to restore, and it cannot be recovered. \
         Open {} to view and copy it.",
        SettingsPage::BACKUP.link()
    )
}

/// Notify the user that a backup path generated a fresh encryption key they
/// never saw, so they must store it: it cannot be recovered and is required to
/// restore.
async fn notify_backup_key_generated(engine: &SharedEngine) {
    emit_backup_notification(
        engine,
        BACKUP_KEY_GENERATED_TITLE,
        &backup_key_generated_message(),
        SettingsPage::BACKUP.tap(),
    )
    .await;
}

const BACKUP_FAILURE_TITLE: &str = "Backup failed";
const BACKUP_FAILURE_DEDUP_MINUTES: i64 = 30;

/// Compose the failure notification's body: what to do about it, then why it
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
    // A provider whose meta we could not resolve is named generically rather
    // than by its raw id, which is a wire value the user has never seen.
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

/// Notify the user that a backup failed, with what to do about it.
///
/// Deduplicated to at most one per 30 minutes. The dedup query keys on
/// [`BACKUP_FAILURE_TITLE`], so the title is a single constant for every cause
/// and only the body varies: a title that named the cause would let a
/// cause-alternating failure notify on every single run.
pub(crate) async fn notify_backup_failure(engine: &SharedEngine, provider_id: &str, error: &str) {
    let pool = engine.pool();
    // The window is resolved by the database (ADR 0053): `notifications.created_at`
    // is stamped by Postgres, so an engine-computed cutoff would compare the host
    // clock against the database clock. A container clock ahead of the host would
    // suppress a failure the user needs to see.
    let recent: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM notifications \
         WHERE title = $1 AND created_at > now() - make_interval(secs => $2))",
    )
    .bind(BACKUP_FAILURE_TITLE)
    .bind((BACKUP_FAILURE_DEDUP_MINUTES * 60) as f64)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| {
        // A dedup lookup that could not run is UNKNOWN, not "already notified":
        // fall through to notifying, so a DB hiccup costs a duplicate banner
        // rather than the backup failure report itself.
        log!("[Backup] Failure-notification dedup lookup failed: {}", e);
        false
    });

    if recent {
        return;
    }

    // A readiness lookup that cannot answer must not cost the user the
    // notification itself: the failure is the thing worth telling them about,
    // and the fallback wording is correct without a verdict.
    let meta = crate::core::backup::provider_meta(provider_id);
    let readiness = match meta.as_ref() {
        Some(m) => match crate::core::backup::provider_readiness(pool, m).await {
            Ok(r) => Some(r),
            Err(e) => {
                log!(
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
        // A Tap::Modal here opened a card repeating the error and offering
        // nothing to do about it. Which page depends on the remedy.
        backup_failure_page(readiness.as_ref()).tap(),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::{
        backup_failure_body, backup_failure_page, backup_key_generated_message,
        describe_backup_failure, spoken_duration, time_asleep, BACKUP_FAILURE_TITLE,
    };
    use std::time::Duration;

    /// The reported run: 24m49s on the wall clock, of which the process ran for
    /// under 12 minutes.
    #[test]
    fn a_run_that_spanned_a_sleep_reports_the_gap() {
        let wall = chrono::Duration::seconds(24 * 60 + 49);
        let ran = Duration::from_secs(11 * 60 + 40);
        assert_eq!(
            time_asleep(wall, ran),
            Some(Duration::from_secs(13 * 60 + 9))
        );
    }

    /// Scheduling noise between the two clocks is not a sleep. Nor is a wall
    /// clock that stepped backwards, which leaves no gap at all.
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

    /// The Settings sub-section a tap deep-links to, or `None` for a modal.
    fn tapped_view(tap: Tap) -> Option<String> {
        match tap {
            Tap::Navigate { to } => {
                assert_eq!(to.target, NavigateTarget::Settings);
                to.settings_view
            }
            Tap::Modal => None,
        }
    }

    /// Connected, but the grant is short a scope the backup needs. Written as
    /// constructors rather than consts because `missing_scopes` is a `Vec`: the
    /// verdict now carries WHICH scopes are missing, and `ready` is derived from
    /// that list so the two can never disagree.
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

    /// The reported case, and the whole point of the change: a connected
    /// account whose grant is too narrow must be told to press *Grant access*,
    /// and where. Before this, the body was the raw error alone and the user
    /// had to ask a human what to do with it.
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

    /// A provider with no account needs the OTHER page: there is nothing to
    /// grant until an account exists, and the Backup page has no account UI
    /// (`system-knowhow/backups.md` is emphatic that sending a user there to
    /// connect is how this flow goes wrong).
    #[test]
    fn an_unconnected_provider_is_sent_to_accounts() {
        let body = backup_failure_body(Some("Dropbox"), Some(&not_connected()), "no account");
        assert!(body.contains("Accounts"), "{body}");
        assert!(
            !body.contains("Grant access"),
            "nothing to grant without an account: {body}"
        );
    }

    /// Every remedy LINKS the page its OWN tap opens. A body naming Accounts
    /// while the tap lands on Backup makes the notification argue with itself.
    ///
    /// The expected links are spelled out here rather than read from
    /// `SettingsPage`, so this pins the WORDING too: sharing the const with the
    /// code under test would let a wrong path stay green.
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

    /// The one branch whose remedy is not a Backup-page matter taps through to
    /// the page that can actually satisfy it.
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

    /// A ready provider that failed anyway (network, quota, pg_dump) has no
    /// permission remedy, so the body names the destination and stops rather
    /// than inventing advice.
    #[test]
    fn a_ready_provider_gets_the_destination_without_invented_advice() {
        let body = backup_failure_body(Some("Dropbox"), Some(&ready()), "upload timed out");
        assert!(body.contains("Backup"), "{body}");
        assert!(!body.contains("Grant access"), "{body}");
        assert!(!body.contains("Accounts"), "{body}");
    }

    /// A readiness lookup that could not answer must not cost the user the
    /// notification, nor produce a remedy the verdict does not support.
    #[test]
    fn an_unresolved_verdict_falls_back_without_losing_the_error() {
        let body = backup_failure_body(None, None, "some failure");
        assert!(body.contains("Backup"), "{body}");
        assert!(body.contains("some failure"), "{body}");
        assert!(!body.contains("Grant access"), "{body}");
    }

    /// The error survives in EVERY branch. Dropping it would trade one missing
    /// half of the notification for the other: the raw string is what made this
    /// bug diagnosable when the user quoted it.
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

    /// The dedup query keys on the title, so the title must not vary with the
    /// cause. A per-cause title would let a failure that alternates between two
    /// causes notify on every single run, defeating the 30-minute window.
    #[test]
    fn the_title_is_one_constant_so_dedup_still_collapses_repeats() {
        assert_eq!(BACKUP_FAILURE_TITLE, "Backup failed");
        // Nothing in the body composition can reach the title: it takes no
        // readiness argument and returns only the body.
        let bodies: Vec<String> = [Some(&connected_not_ready()), Some(&ready()), None]
            .into_iter()
            .map(|r| backup_failure_body(Some("Dropbox"), r, "e"))
            .collect();
        assert_eq!(bodies.len(), 3);
    }

    /// A provider whose metadata could not be resolved is named generically.
    /// The raw id is a wire value the user has never seen on any screen.
    #[test]
    fn an_unknown_provider_is_not_named_by_its_raw_id() {
        let body = backup_failure_body(None, Some(&connected_not_ready()), "e");
        assert!(!body.contains("google_drive"), "{body}");
        assert!(body.starts_with("Your backup provider"), "{body}");
    }

    /// Backup is a subpanel of Settings → System. A body that stops at
    /// "Settings, then Backup" sends the reader to a page with no Backup on it.
    /// Someone reading the notification where the tap is not to hand, a
    /// lock-screen banner they dismiss, has only this route.
    #[test]
    fn every_backup_remedy_links_the_system_step() {
        const LINK: &str = "[Settings → System → Backup](settings:backup)";
        for readiness in [Some(&connected_not_ready()), Some(&ready()), None] {
            let body = backup_failure_body(Some("Dropbox"), readiness, "e");
            assert!(body.contains(LINK), "{readiness:?}: {body}");
        }
        // The sibling notification points at the same page and must agree.
        let key_message = backup_key_generated_message();
        assert!(key_message.contains(LINK), "{key_message}");
    }

    /// A key the user never saw makes every backup unrestorable once the
    /// machine is lost, because the key file is excluded from the archive.
    /// Turning a schedule on and a manual backup both minted one silently, so
    /// the first scheduled run found it existing and sent no notification.
    /// Only `POST /backup/key`, which shows the key, may mint around the helper.
    #[test]
    fn every_unattended_key_mint_goes_through_the_notifying_helper() {
        use crate::test_support::source_scan::production_sources;
        let allowed = [
            ("core/backup/crypto.rs", usize::MAX),
            ("scheduler/backup.rs", 1),
            ("api/backup.rs", 1),
        ];
        for (rel, text) in production_sources() {
            let calls =
                text.matches("crypto::ensure_key(").count() + text.matches(" ensure_key(").count();
            if calls == 0 {
                continue;
            }
            let limit = allowed
                .iter()
                .find(|(path, _)| *path == rel)
                .map(|(_, n)| *n)
                .unwrap_or(0);
            assert!(
                calls <= limit,
                "{rel} mints a backup key {calls} time(s) without notifying the user; \
                 call scheduler::ensure_backup_key instead"
            );
        }
    }
}
