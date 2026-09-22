//! Background marketplace sync and plugin update-check notifications.
//!
//! The engine re-scans every registered *marketplace* on a timer (and at
//! startup / after a marketplace is added) but does NOT silently apply plugin
//! updates — the user asked to be told and to decide. When the scan finds
//! installed plugins with a newer version available, this emits a single,
//! deduplicated notification pointing the user at the Apps section, where each
//! plugin's "Update" button stages the change for the user to confirm.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use chrono::Utc;

use crate::api::SharedEngine;
use crate::core::git_auth::GitCredentials;
use crate::core::plugin_catalog_cache::{self, SCAN_STALL_CUTOFF_SECS};
use crate::core::plugin_marketplaces::{
    clone_urls, load_registry, scan_catalog, update_candidates, MarketplaceCatalog,
    MarketplacePlugin, MarketplaceScanError,
};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::tools::plugins::installed_plugin_summaries;
use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};

pub(crate) const MARKETPLACE_UPDATE_CHECK_CRON: &str = "0 */5 * * * *";

/// Marker file recording the set of update candidates the user has already been
/// notified about, so the 5-minute re-scan doesn't re-notify about the same
/// updates. Lives under `.lucidos/` (gitignored, rebuildable runtime cache) —
/// losing it only costs one extra notification after a fresh engine.
const UPDATE_NOTICE_MARKER: &str = ".lucidos/plugin-update-notice.json";

#[derive(Debug, Default)]
pub(crate) struct PluginUpdateCheckReport {
    pub marketplaces: usize,
    pub candidates: usize,
    pub notified: bool,
    pub errors: Vec<String>,
}

impl PluginUpdateCheckReport {
    /// The lines this check owes the log, one per accumulated failure, and
    /// nothing at all for a clean run.
    ///
    /// Numbered `n/total`, so a partial sync says how much of the catalog it
    /// got through: one failing marketplace out of three is a very different
    /// morning from all three failing.
    fn failure_log_lines(&self) -> Vec<String> {
        let total = self.errors.len();
        self.errors
            .iter()
            .enumerate()
            .map(|(i, e)| format!("sync failure {}/{}: {}", i + 1, total, e))
            .collect()
    }
}

/// Why a scan was asked for. Decides what a caller does when another scan
/// already holds the single-flight guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanCause {
    /// The scheduler's tick, or a reader wanting current data. Joining the
    /// running scan is good enough: there is no change to be newer than.
    Routine,
    /// The registry just changed. The running scan may have read it before the
    /// write, so this one leaves a trailing pass behind rather than joining.
    RegistryChanged,
}

/// A scan is owed because the registry changed. Set BEFORE reaching for the
/// slot, and cleared by whichever pass takes it.
///
/// Claim-then-clear-on-acquire is what makes it race-free. The pass that clears
/// the claim is a pass that reads the registry afterwards, so it sees the
/// write. A claim arriving while a pass is mid-scan stays set, and that pass
/// goes round again on the way out.
///
/// Without it, a scan that read the registry BEFORE the write republishes the
/// old contents under a fresh `scanned_at`. That suppresses a page-open rescan
/// for the whole TTL, so a newly registered marketplace stays invisible. A
/// single flag, not a queue, so a burst of registrations collapses into one
/// follow-up.
static SCAN_REQUEUE: AtomicBool = AtomicBool::new(false);

/// Epoch seconds when the running scan took the guard. Zero means free.
static UPDATE_CHECK_STARTED: AtomicI64 = AtomicI64::new(0);

struct UpdateCheckGuard(i64);

impl UpdateCheckGuard {
    fn try_acquire(now: i64) -> Option<Self> {
        loop {
            let held = UPDATE_CHECK_STARTED.load(Ordering::SeqCst);
            // A clone has no timeout, so a wedged scan holds this for ever. It
            // would freeze the catalog for good, now that a page open reads
            // what the last scan left rather than scanning itself. Past the
            // stall cutoff the slot is taken over: nothing waits on the wedged
            // task, and the alternative is a workspace that never updates again.
            if held != 0 && now.saturating_sub(held) < SCAN_STALL_CUTOFF_SECS {
                return None;
            }
            if UPDATE_CHECK_STARTED
                .compare_exchange(held, now, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                if held != 0 {
                    log!("[PluginUpdateCheck] Taking over a scan slot held since {held}");
                }
                return Some(Self(now));
            }
        }
    }

    /// Is this still the live scan, or has a newer one taken the slot over?
    ///
    /// A wedged clone's task eventually unwinds, long after the takeover above
    /// gave its slot away. By then its result is older than what replaced it.
    /// Writing it would republish stale rows under a fresh `scanned_at` and
    /// clear the live scan's own stamp. So a loser writes nothing.
    fn still_held(&self) -> bool {
        UPDATE_CHECK_STARTED.load(Ordering::SeqCst) == self.0
    }
}

impl Drop for UpdateCheckGuard {
    fn drop(&mut self) {
        // Only clear the slot while we still hold it. A taken-over slot belongs
        // to a live scan, and clearing it would let a third one in beside it.
        let _ =
            UPDATE_CHECK_STARTED.compare_exchange(self.0, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// Run one marketplace sync and report every failure it accumulated.
///
/// The log line is the whole point of the wrapper. Every caller discards the
/// report, so a marketplace whose git fetch fails, or a corrupt
/// `marketplaces.json`, used to leave the five-minute check running in silence.
pub(crate) async fn run_plugin_marketplace_update_check(
    engine: SharedEngine,
    pool: sqlx::PgPool,
    cause: ScanCause,
) -> PluginUpdateCheckReport {
    if cause == ScanCause::RegistryChanged {
        // Claimed before reaching for the slot, so it cannot matter whether we
        // get it. Whoever does clears the claim and then reads the registry.
        SCAN_REQUEUE.store(true, Ordering::SeqCst);
    }
    let (mut report, mut scanned) = run_one_pass(&engine, &pool).await;
    // ONLY the pass that held the slot goes round again. A refused caller
    // draining here would clear the very claim it just made, and the running
    // scan would then find nothing owed: both reviewers caught that shape.
    // A claim still set after our pass arrived while we were scanning. So it
    // landed after we read the registry, and our result does not reflect it.
    while scanned && SCAN_REQUEUE.load(Ordering::SeqCst) && !engine.is_shutting_down() {
        (report, scanned) = run_one_pass(&engine, &pool).await;
    }
    report
}

/// One pass, plus whether it actually ran: `false` means another scan held the
/// slot, so this caller neither scanned nor owns the claim.
async fn run_one_pass(
    engine: &SharedEngine,
    pool: &sqlx::PgPool,
) -> (PluginUpdateCheckReport, bool) {
    let (report, scanned) = check_marketplaces_for_updates(engine.clone(), pool.clone()).await;
    for line in report.failure_log_lines() {
        log!("[PluginUpdateCheck] {line}");
    }
    (report, scanned)
}

/// Stamp the *plugin catalog cache* as scanning, before the scan's own task has
/// had a chance to run.
///
/// `tokio::spawn` returns before its body does. So a caller that spawns a scan
/// and then answers an HTTP request can serve `scanning: false` for a scan it
/// just queued. On a workspace with nothing cached that reads as "No plugins
/// found", at the exact moment the user registered their first marketplace.
/// Every site that spawns a scan calls this first, synchronously.
pub(crate) fn note_scan_queued(workspace: &Path) {
    plugin_catalog_cache::mark_scan_started(workspace, Utc::now());
}

/// Run one scan and leave the *plugin catalog cache* holding its outcome.
///
/// `None` means the scan could not complete at all, which is distinct from a
/// scan whose individual marketplaces failed: those ride in the catalog's own
/// `errors` and still produce a result. Either way the cache settles here, so
/// no caller can leave a start stamp behind and pin the cue on.
async fn scan_into_cache(
    pool: &sqlx::PgPool,
    workspace: &Path,
    guard: &UpdateCheckGuard,
    report: &mut PluginUpdateCheckReport,
) -> Option<MarketplaceCatalog> {
    let mut fail = |what: String| -> Option<MarketplaceCatalog> {
        if guard.still_held() {
            plugin_catalog_cache::record_scan_failure(workspace, &what);
        }
        report.errors.push(what);
        None
    };

    let registry = match load_registry(workspace) {
        Ok(registry) => registry,
        Err(e) => return fail(format!("read marketplace registry: {e}")),
    };
    report.marketplaces = registry.marketplaces.len();
    if registry.marketplaces.is_empty() {
        // Nothing to clone, so skip the DB read and the blocking pass. The
        // empty result still lands in the cache: a catalog that kept the last
        // registry's plugins would offer plugins from nowhere.
        let empty = MarketplaceCatalog {
            marketplaces: vec![],
            plugins: vec![],
            errors: vec![],
        };
        record_scan_if_live(workspace, guard, &empty);
        return Some(empty);
    }

    let installed = match installed_plugin_summaries(pool, workspace).await {
        Ok(installed) => installed,
        Err(e) => return fail(format!("read installed plugins: {e}")),
    };

    let scan_workspace = workspace.to_path_buf();
    let scan_registry = registry.clone();
    // The scan is synchronous, so its credentials are resolved out here.
    let credentials = GitCredentials::resolve_many(pool, &clone_urls(&registry)).await;
    let catalog = match tokio::task::spawn_blocking(move || {
        scan_catalog(&scan_workspace, &scan_registry, &installed, &credentials)
    })
    .await
    {
        Ok(catalog) => catalog,
        Err(e) => return fail(format!("scan marketplaces task panicked: {e}")),
    };

    record_scan_if_live(workspace, guard, &catalog);
    Some(catalog)
}

/// Publish a result only while this scan still owns the slot.
///
/// A scan taken over past the stall cutoff finishes eventually, and its rows
/// are older than the ones that replaced them. Writing them would stamp stale
/// content as fresh and clear the live scan's own start stamp.
fn record_scan_if_live(workspace: &Path, guard: &UpdateCheckGuard, catalog: &MarketplaceCatalog) {
    if guard.still_held() {
        plugin_catalog_cache::record_scan(workspace, catalog, Utc::now());
    } else {
        log!("[PluginUpdateCheck] Discarding a scan whose slot was taken over");
    }
}

/// Broadcast one scan frame. Both variants are transient, so this reaches SSE
/// and writes no row.
async fn announce_scan(engine: &SharedEngine, event: SystemEvent, what: &str) {
    engine
        .event_bus
        .emit_or_log(
            BusEvent::System(event),
            &format!("[PluginUpdateCheck] PluginCatalogScan{what}"),
        )
        .await;
}

/// The boolean says whether this call HELD the slot, which is what decides who
/// drains a pending claim. See [`SCAN_REQUEUE`].
async fn check_marketplaces_for_updates(
    engine: SharedEngine,
    pool: sqlx::PgPool,
) -> (PluginUpdateCheckReport, bool) {
    if engine.is_shutting_down() {
        return (PluginUpdateCheckReport::default(), false);
    }
    let Some(guard) = UpdateCheckGuard::try_acquire(Utc::now().timestamp()) else {
        log!("[PluginUpdateCheck] Skipping run; another marketplace sync is active");
        return (PluginUpdateCheckReport::default(), false);
    };
    // Cleared here, at the top of the pass that holds the slot, so everything
    // below reads a registry newer than any claim already made.
    SCAN_REQUEUE.store(false, Ordering::SeqCst);

    let mut report = PluginUpdateCheckReport::default();
    let workspace = engine.workspace_path().to_path_buf();

    // The scan is the only producer of the plugin catalog cache, so it bookends
    // itself: a start stamp, then either a result or a recorded failure. Both
    // ends announce, which is what raises and lowers the Plugins panel's
    // "Updating…" cue on every connected client. The stamp is re-taken here
    // rather than trusted from `note_scan_queued`, so the scheduler's own run
    // (which nobody queued) is stamped too.
    note_scan_queued(&workspace);
    announce_scan(&engine, SystemEvent::PluginCatalogScanStarted {}, "Started").await;

    let scanned = scan_into_cache(&pool, &workspace, &guard, &mut report).await;

    // A slot taken over mid-scan means a newer pass owns the cache and the cue.
    // Announcing here would lower that pass's "Updating…" and send every client
    // to re-read a result this one was not allowed to write.
    if !guard.still_held() {
        return (report, false);
    }
    announce_scan(
        &engine,
        SystemEvent::PluginCatalogScanned {
            failed: scanned.is_none(),
        },
        "Scanned",
    )
    .await;

    let Some(catalog) = scanned else {
        return (report, true);
    };
    if catalog.marketplaces.is_empty() {
        // No marketplaces → nothing can have an update. Clear any stale marker
        // so a future re-registration starts from a clean slate.
        write_notified_signature(&workspace, &BTreeSet::new());
        return (report, true);
    }

    for error in &catalog.errors {
        report.errors.push(format_scan_error(error));
    }

    let candidates = update_candidates(&catalog);
    report.candidates = candidates.len();

    let current: BTreeSet<String> = candidates
        .iter()
        .map(|p| format!("{}@{}", p.id, p.version))
        .collect();
    let already_notified = read_notified_signature(&workspace);
    // Notify only when something is newly available (a fresh update or a bumped
    // version). Applying one of several updates shrinks the set but introduces
    // no new entry, so it never re-notifies about the rest.
    let has_new = current.difference(&already_notified).next().is_some();

    if candidates.is_empty() {
        // Clear the marker so a future re-appearance of any version is "new".
        write_notified_signature(&workspace, &current);
        return (report, true);
    }

    if !has_new {
        // Persist the (possibly shrunk) set so applied updates drop out of the
        // marker — nothing to notify here, so there's no notification at risk.
        write_notified_signature(&workspace, &current);
        log!(
            "[PluginUpdateCheck] {} update(s) available; already notified",
            candidates.len()
        );
        return (report, true);
    }

    let (title, message) = build_update_notification(&candidates);
    match engine
        .create_notification(
            &title,
            &message,
            None,
            None,
            None,
            Tap::Navigate {
                to: Box::new(build_update_navigation(&candidates)),
            },
            None,
        )
        .await
    {
        Ok(_) => {
            report.notified = true;
            // Record the notified set only after the emit succeeds, so a
            // transient emit failure retries on the next scan instead of being
            // silently swallowed by the marker.
            write_notified_signature(&workspace, &current);
            log!(
                "[PluginUpdateCheck] Notified user of {} available plugin update(s)",
                candidates.len()
            );
        }
        Err(e) => {
            // Logged by the wrapper, with every other accumulated failure.
            report
                .errors
                .push(format!("emit update notification failed: {e}"));
        }
    }

    (report, true)
}

/// Title + body for the "updates available" notification. Singular and plural
/// phrasings; the body lists plugin names so the user knows what's waiting
/// before opening the Plugins panel.
fn build_update_notification(candidates: &[MarketplacePlugin]) -> (String, String) {
    if let [only] = candidates {
        return (
            "Plugin update available".to_string(),
            format!(
                "{} v{} is ready to update. Open Plugins to review.",
                only.name, only.version
            ),
        );
    }
    let mut names: Vec<&str> = candidates.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    (
        "Plugin updates available".to_string(),
        format!(
            "{} plugins have updates: {}. Open Plugins to review.",
            candidates.len(),
            names.join(", ")
        ),
    )
}

/// Where the update notification's tap lands: the Plugins panel's Installed tab,
/// scrolled to (and pulsing) the row of a plugin that has a pending update. With
/// a single candidate that's its row; with several we focus the
/// alphabetically-first by name (deterministic — matches the name ordering in the
/// plural body) while the rest stay visibly chipped in the list. The `id` is the
/// plugin id, which the Installed tab matches against each row's `data-plugin-id`.
fn build_update_navigation(candidates: &[MarketplacePlugin]) -> NavigateUi {
    let focus = candidates
        .iter()
        .min_by(|a, b| a.name.cmp(&b.name))
        .map(|p| p.id.clone());
    NavigateUi {
        target: NavigateTarget::Plugins,
        id: focus,
        ..Default::default()
    }
}

fn marker_path(workspace: &Path) -> PathBuf {
    workspace.join(UPDATE_NOTICE_MARKER)
}

/// Read the set of `id@version` strings the user was last notified about. A
/// missing/unreadable/garbage marker reads as empty — at worst one extra
/// notification, never a missed one.
fn read_notified_signature(workspace: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(marker_path(workspace))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

fn write_notified_signature(workspace: &Path, signature: &BTreeSet<String>) {
    let path = marker_path(workspace);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log!("[PluginUpdateCheck] create marker dir failed: {e}");
            return;
        }
    }
    let entries: Vec<&String> = signature.iter().collect();
    match serde_json::to_string(&entries) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                log!("[PluginUpdateCheck] write update marker failed: {e}");
            }
        }
        Err(e) => log!("[PluginUpdateCheck] serialize update marker failed: {e}"),
    }
}

fn format_scan_error(error: &MarketplaceScanError) -> String {
    format!(
        "scan {} ({}) failed: {}",
        error.marketplace_name, error.source, error.error
    )
}

#[cfg(test)]
#[path = "plugin_updates_tests.rs"]
mod tests;
