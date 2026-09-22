//! Tests for the plugin update-check notification helpers — notification
//! phrasing and the dedup marker that stops the 5-minute re-scan from
//! re-notifying about updates the user has already seen.

use super::*;
use crate::core::plugin_marketplaces::{MarketplacePlugin, MarketplacePluginStatus};

fn candidate(id: &str, name: &str, version: &str) -> MarketplacePlugin {
    MarketplacePlugin {
        marketplace_id: "mkt".to_string(),
        marketplace_name: "Test Marketplace".to_string(),
        id: id.to_string(),
        name: name.to_string(),
        description: String::new(),
        version: version.to_string(),
        source: format!("https://example.com/{id}"),
        manifest: serde_json::json!({}),
        content: vec!["apps".to_string()],
        categories: vec![],
        files_count: 1,
        status: MarketplacePluginStatus::UpdateAvailable,
        installed_version: Some("0.0.1".to_string()),
        setup_thread_id: None,
        setup_complete: false,
        app_id: Some(id.to_string()),
        modified: false,
        modified_paths: vec![],
    }
}

#[test]
fn single_candidate_uses_singular_phrasing_with_name_and_version() {
    let (title, message) = build_update_notification(&[candidate("weather", "Weather", "1.2.0")]);
    assert_eq!(title, "Plugin update available");
    assert!(message.contains("Weather"), "message: {message}");
    assert!(message.contains("1.2.0"), "message: {message}");
    // Points at Plugins, not the old "Apps" / app store wording.
    assert!(
        message.contains("Open Plugins to review."),
        "message: {message}"
    );
    assert!(
        !message.contains("Apps"),
        "should not mention Apps: {message}"
    );
}

#[test]
fn multiple_candidates_use_plural_phrasing_with_sorted_names_and_count() {
    let (title, message) = build_update_notification(&[
        candidate("weather", "Weather", "1.2.0"),
        candidate("habit", "Habit Tracker", "2.0.0"),
    ]);
    assert_eq!(title, "Plugin updates available");
    assert!(message.contains('2'), "expected count in: {message}");
    // Names are listed alphabetically regardless of candidate order.
    let habit = message.find("Habit Tracker").expect("habit listed");
    let weather = message.find("Weather").expect("weather listed");
    assert!(habit < weather, "names should be sorted: {message}");
    assert!(
        message.contains("Open Plugins to review."),
        "message: {message}"
    );
}

#[test]
fn single_candidate_navigation_focuses_that_plugin_in_the_installed_tab() {
    let nav = build_update_navigation(&[candidate("weather", "Weather", "1.2.0")]);
    assert_eq!(nav.target, NavigateTarget::Plugins);
    assert_eq!(nav.id.as_deref(), Some("weather"));
}

#[test]
fn multiple_candidates_navigation_focuses_alphabetically_first_by_name() {
    // Candidate order is weather-then-habit; the focus is the name-first one
    // (Habit Tracker), matching the plural body's name ordering.
    let nav = build_update_navigation(&[
        candidate("weather", "Weather", "1.2.0"),
        candidate("habit", "Habit Tracker", "2.0.0"),
    ]);
    assert_eq!(nav.target, NavigateTarget::Plugins);
    assert_eq!(nav.id.as_deref(), Some("habit"));
}

/// Every accumulated failure reaches the log.
///
/// All three callers discard the report, so a marketplace whose git fetch
/// failed used to be recorded and then dropped. The five-minute check ran on
/// in silence while one marketplace never synced.
#[test]
fn every_recorded_failure_becomes_one_log_line() {
    let report = PluginUpdateCheckReport {
        marketplaces: 3,
        errors: vec![
            "scan Weather (https://example.com/weather) failed: auth".to_string(),
            "read installed plugins: pool closed".to_string(),
        ],
        ..Default::default()
    };

    let lines = report.failure_log_lines();
    assert_eq!(lines.len(), 2, "one line per failure: {lines:?}");
    assert!(lines[0].contains("1/2"), "line 0: {}", lines[0]);
    assert!(lines[0].contains("failed: auth"), "line 0: {}", lines[0]);
    assert!(lines[1].contains("2/2"), "line 1: {}", lines[1]);
    assert!(lines[1].contains("pool closed"), "line 1: {}", lines[1]);
}

#[test]
fn a_clean_check_logs_nothing() {
    let report = PluginUpdateCheckReport {
        marketplaces: 2,
        candidates: 1,
        notified: true,
        errors: vec![],
    };
    assert!(report.failure_log_lines().is_empty());
}

#[test]
fn marker_roundtrips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mut sig = BTreeSet::new();
    sig.insert("weather@1.2.0".to_string());
    sig.insert("habit@2.0.0".to_string());

    write_notified_signature(dir.path(), &sig);
    let read_back = read_notified_signature(dir.path());

    assert_eq!(read_back, sig);
}

#[test]
fn missing_marker_reads_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_notified_signature(dir.path()).is_empty());
}

#[test]
fn new_update_is_detected_against_an_existing_marker() {
    let dir = tempfile::tempdir().unwrap();
    let mut notified = BTreeSet::new();
    notified.insert("weather@1.2.0".to_string());
    write_notified_signature(dir.path(), &notified);

    // A freshly bumped version is "new" relative to what was already notified.
    let current: BTreeSet<String> = ["weather@1.2.0".to_string(), "weather@1.3.0".to_string()]
        .into_iter()
        .collect();
    let already = read_notified_signature(dir.path());
    assert!(current.difference(&already).next().is_some());
}

#[test]
fn shrinking_set_after_an_apply_is_not_treated_as_new() {
    let dir = tempfile::tempdir().unwrap();
    let notified: BTreeSet<String> = ["weather@1.2.0".to_string(), "habit@2.0.0".to_string()]
        .into_iter()
        .collect();
    write_notified_signature(dir.path(), &notified);

    // User applied "habit"; only "weather" remains — no new entry, so no
    // re-notification.
    let current: BTreeSet<String> = ["weather@1.2.0".to_string()].into_iter().collect();
    let already = read_notified_signature(dir.path());
    assert!(current.difference(&already).next().is_none());
}

/// Run a body against the scan statics with nobody else touching them.
///
/// The slot and the claim are process-global, so two of these in parallel read
/// each other's writes as real conflicts. ONE lock, shared by both modules
/// below: a lock per module is no lock at all, which cost a red run.
fn with_clean_scan_statics<T>(body: impl FnOnce() -> T) -> T {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    UPDATE_CHECK_STARTED.store(0, Ordering::SeqCst);
    SCAN_REQUEUE.store(false, Ordering::SeqCst);
    let out = body();
    UPDATE_CHECK_STARTED.store(0, Ordering::SeqCst);
    SCAN_REQUEUE.store(false, Ordering::SeqCst);
    out
}

/// The single-flight slot, which is now load-bearing in a way it was not.
///
/// Every page open reads what the last scan left, so a scan that never ends
/// freezes the catalog rather than merely skipping one update check. These pin
/// both halves: a running scan keeps the slot, and a wedged one loses it.
mod scan_slot {
    use super::*;

    fn with_free_slot<T>(body: impl FnOnce() -> T) -> T {
        with_clean_scan_statics(body)
    }

    #[test]
    fn a_second_scan_is_refused_while_one_runs() {
        with_free_slot(|| {
            let now = 1_000_000;
            let first = UpdateCheckGuard::try_acquire(now);
            assert!(first.is_some());
            assert!(UpdateCheckGuard::try_acquire(now + 1).is_none());
        });
    }

    #[test]
    fn the_slot_is_free_again_once_the_scan_drops_it() {
        with_free_slot(|| {
            let now = 1_000_000;
            drop(UpdateCheckGuard::try_acquire(now));
            assert!(UpdateCheckGuard::try_acquire(now + 1).is_some());
        });
    }

    /// A taken-over scan must not publish its result. Its rows are older than
    /// the ones that replaced them. Writing them would stamp stale content as
    /// fresh and clear the live scan's own start stamp.
    #[test]
    fn a_taken_over_scan_knows_it_lost_the_slot() {
        with_free_slot(|| {
            let now = 1_000_000;
            let wedged = UpdateCheckGuard::try_acquire(now).unwrap();
            assert!(wedged.still_held());

            let live = UpdateCheckGuard::try_acquire(now + SCAN_STALL_CUTOFF_SECS).unwrap();

            assert!(!wedged.still_held(), "the wedged scan must write nothing");
            assert!(live.still_held());
        });
    }

    /// `shallow_clone` has no timeout, so a wedged clone holds the slot with no
    /// one waiting on it. Without a takeover the workspace never scans again,
    /// and its plugin list is frozen for the life of the engine.
    #[test]
    fn a_wedged_scan_loses_the_slot_past_the_cutoff() {
        with_free_slot(|| {
            let now = 1_000_000;
            let wedged = UpdateCheckGuard::try_acquire(now);
            assert!(wedged.is_some());

            let taken_over = UpdateCheckGuard::try_acquire(now + SCAN_STALL_CUTOFF_SECS);
            assert!(taken_over.is_some(), "a wedged slot must be reclaimable");

            // The wedged task eventually unwinds. Its drop must not free the
            // slot the takeover is holding, or a third scan joins the second.
            drop(wedged);
            assert_eq!(
                UPDATE_CHECK_STARTED.load(Ordering::SeqCst),
                now + SCAN_STALL_CUTOFF_SECS,
            );
        });
    }
}

/// The claim a registry change leaves behind, and who is allowed to clear it.
///
/// Both reviewers of the first attempt found the same bug: the refused caller
/// drained its OWN claim moments after making it, so the running scan found
/// nothing owed and published pre-change contents as fresh.
mod scan_requeue {
    use super::*;

    fn with_clean_flags<T>(body: impl FnOnce() -> T) -> T {
        with_clean_scan_statics(body)
    }

    /// A claim survives a caller that could not get the slot, so the scan
    /// holding it still owes a pass.
    #[test]
    fn a_refused_caller_leaves_the_claim_standing() {
        with_clean_flags(|| {
            let running = UpdateCheckGuard::try_acquire(1_000_000);
            assert!(running.is_some());

            // What `run_plugin_marketplace_update_check` does for a registry
            // change, before it reaches for the slot.
            SCAN_REQUEUE.store(true, Ordering::SeqCst);
            assert!(UpdateCheckGuard::try_acquire(1_000_001).is_none());

            assert!(
                SCAN_REQUEUE.load(Ordering::SeqCst),
                "the running scan must still see a pass is owed",
            );
        });
    }

    /// Whoever takes the slot clears the claim, and does so BEFORE reading the
    /// registry. That ordering is what makes the flag race-free: the pass that
    /// clears it is a pass whose read comes after the write.
    #[test]
    fn taking_the_slot_is_what_clears_the_claim() {
        with_clean_flags(|| {
            SCAN_REQUEUE.store(true, Ordering::SeqCst);
            let holder = UpdateCheckGuard::try_acquire(1_000_000);
            assert!(holder.is_some());

            // The line at the top of `check_marketplaces_for_updates`.
            SCAN_REQUEUE.store(false, Ordering::SeqCst);

            assert!(!SCAN_REQUEUE.load(Ordering::SeqCst));
        });
    }
}
