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
            notify_backup_failure(engine, provider.id(), &msg).await;
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
            notify_backup_failure(&engine, &provider_id, &e.to_string()).await;
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

    push::send_push_to_all(engine, title, message, Some(notification_id)).await;
}

/// A tap that deep-links to one Settings sub-section, the same way the LLM's
/// `navigate_ui` does. `view` must be one of `NAVIGABLE_SETTINGS_VIEWS`
/// (`llm/tools/misc.rs`), which is the set the frontend router renders.
fn settings_tap(view: &str) -> crate::scheduler::notifications::Tap {
    use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};

    Tap::Navigate {
        to: NavigateUi {
            target: NavigateTarget::Settings,
            settings_view: Some(view.to_string()),
            ..Default::default()
        },
    }
}

/// Settings → Backup: the page carrying the health card with the last run and
/// its error, the key, the schedule, and the *Grant access* button.
fn backup_settings_tap() -> crate::scheduler::notifications::Tap {
    settings_tap("backup")
}

/// Where a failure notification should land, which is NOT always the Backup
/// page.
///
/// The tap has to agree with the remedy the body just gave, or the notification
/// argues with itself. For a provider with no account the remedy is *connect
/// it*, and connecting happens only in Settings → Accounts: the Backup page has
/// no account UI, and `system-knowhow/backups.md` is emphatic that sending a
/// user there to connect is how this flow goes wrong. Every other cause is a
/// backup-page matter.
fn backup_failure_tap(
    readiness: Option<&crate::core::backup::ProviderReadiness>,
) -> crate::scheduler::notifications::Tap {
    match readiness {
        Some(r) if !r.connected => settings_tap("accounts"),
        _ => backup_settings_tap(),
    }
}

/// Notify the user that the scheduled backup auto-generated a fresh encryption
/// key. Unlike the manual flow, the user never saw this key as it was created,
/// so they must be told to store it safely — it cannot be recovered and is
/// required to restore. The tap deep-links to Settings → Backup, where the key
/// can be revealed and copied.
async fn notify_backup_key_generated(engine: &SharedEngine) {
    const MESSAGE: &str = "Your scheduled backup created a new encryption key. \
        Store it somewhere safe — you need it to restore, and it cannot be recovered. \
        Open Settings → Backup to view and copy it.";

    emit_backup_notification(
        engine,
        BACKUP_KEY_GENERATED_TITLE,
        MESSAGE,
        backup_settings_tap(),
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
/// alone, which is right for every cause: the Backup page carries the health
/// card, the error and the *Grant access* button, so it is where the user needs
/// to be whatever went wrong.
fn backup_failure_body(
    provider_name: Option<&str>,
    readiness: Option<&crate::core::backup::ProviderReadiness>,
    error: &str,
) -> String {
    // A provider whose meta we could not resolve is named generically rather
    // than by its raw id, which is a wire value the user has never seen.
    let who = provider_name.unwrap_or("Your backup provider");
    // Each branch names the page its own tap opens (see `backup_failure_tap`),
    // so the text and the destination cannot disagree. Connecting is the one
    // remedy that does NOT live on the Backup page.
    let remedy = match readiness {
        Some(r) if !r.connected => format!(
            "{who} has no connected account, so nothing can upload. \
             Connect it in Settings, then Accounts."
        ),
        Some(r) if !r.ready() => format!(
            "{who} is connected but has not granted the permissions a backup needs. \
             Open Settings, then Backup, and press Grant access."
        ),
        _ => "Open Settings, then Backup, to see the details and retry.".to_string(),
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
        // Deep-linked for the same reason as the key-generated notification
        // above: a Tap::Modal here opened a card repeating the error and
        // offering nothing to do about it. Which page depends on the remedy.
        backup_failure_tap(readiness.as_ref()),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::{
        backup_failure_body, backup_failure_tap, backup_settings_tap, BACKUP_FAILURE_TITLE,
    };
    use crate::core::backup::ProviderReadiness;
    use crate::scheduler::notifications::{NavigateTarget, Tap};

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

    /// Every remedy names the page its OWN tap opens. A body naming Accounts
    /// while the tap lands on Backup makes the notification argue with itself,
    /// which is the whole failure this change set out to end.
    #[test]
    fn every_remedy_names_the_page_its_tap_opens() {
        for readiness in [
            Some(&connected_not_ready()),
            Some(&ready()),
            Some(&not_connected()),
            None,
        ] {
            let body = backup_failure_body(Some("Dropbox"), readiness, "e");
            let view = tapped_view(backup_failure_tap(readiness))
                .unwrap_or_else(|| panic!("{readiness:?} must deep-link, not open a modal"));
            let named = match view.as_str() {
                "accounts" => "Settings, then Accounts",
                "backup" => "Settings, then Backup",
                other => panic!("unexpected destination {other}"),
            };
            assert!(
                body.contains(named),
                "{readiness:?} taps through to {view} but the body does not say so: {body}"
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

    /// A backup notification deep-links to the page that carries its remedy.
    /// A `Tap::Modal` is what made the failure notification a dead end.
    #[test]
    fn the_tap_deep_links_to_the_backup_settings_page() {
        assert_eq!(
            tapped_view(backup_settings_tap()).as_deref(),
            Some("backup")
        );
    }

    /// Every destination has to be one the frontend router renders. An id
    /// outside `NAVIGABLE_SETTINGS_VIEWS` (`llm/tools/misc.rs`) toasts
    /// "Unknown settings section" instead of navigating, turning the tap back
    /// into the dead end it replaced.
    #[test]
    fn every_tap_destination_is_a_renderable_settings_view() {
        const RENDERABLE: &[&str] = &["accounts", "backup"];
        let taps = [
            backup_settings_tap(),
            backup_failure_tap(Some(&not_connected())),
            backup_failure_tap(Some(&connected_not_ready())),
            backup_failure_tap(Some(&ready())),
            backup_failure_tap(None),
        ];
        for tap in taps {
            let view = tapped_view(tap).expect("a deep link");
            assert!(RENDERABLE.contains(&view.as_str()), "{view}");
        }
    }
}
