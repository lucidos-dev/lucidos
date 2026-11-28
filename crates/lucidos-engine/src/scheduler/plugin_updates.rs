use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use chrono::Utc;

use crate::core::git_auth::GitCredentials;
use crate::core::plugin_catalog_cache::{self, SCAN_STALL_CUTOFF_SECS};
use crate::core::plugin_marketplaces::{
    clone_urls, load_registry, scan_catalog, update_candidates, MarketplaceCatalog,
    MarketplacePlugin, MarketplaceScanError,
};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::release_notices::running_release;
use crate::engine::types::SharedEngine;
use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};

pub(crate) const MARKETPLACE_UPDATE_CHECK_CRON: &str = "0 */5 * * * *";

const UPDATE_NOTICE_MARKER: &str = ".lucidos/plugin-update-notice.json";

#[derive(Debug, Default)]
pub struct PluginUpdateCheckReport {
    pub marketplaces: usize,
    pub candidates: usize,
    pub notified: bool,
    pub errors: Vec<String>,
}

impl PluginUpdateCheckReport {
    fn failure_log_lines(&self) -> Vec<String> {
        let total = self.errors.len();
        self.errors
            .iter()
            .enumerate()
            .map(|(i, e)| format!("sync failure {}/{}: {}", i + 1, total, e))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanCause {
    Routine,
    RegistryChanged,
    Requested,
}

impl ScanCause {
    pub(crate) fn leaves_trailing_pass(self) -> bool {
        matches!(self, Self::RegistryChanged | Self::Requested)
    }
}

/// INVARIANT: A scan is owed because the registry changed or a user asked for one. Set
/// BEFORE reaching for the slot, and cleared by whichever pass takes it.
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

static UPDATE_CHECK_STARTED: AtomicI64 = AtomicI64::new(0);

struct UpdateCheckGuard(i64);

impl UpdateCheckGuard {
    fn try_acquire(now: i64) -> Option<Self> {
        loop {
            let held = UPDATE_CHECK_STARTED.load(Ordering::SeqCst);
            // INVARIANT: A clone has no timeout, so a wedged scan holds this for ever. It
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
                    log_legacy!("[PluginUpdateCheck] Taking over a scan slot held since {held}");
                }
                return Some(Self(now));
            }
        }
    }

    /// INVARIANT: Is this still the live scan, or has a newer one taken the slot over?
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
        // INVARIANT: Only clear the slot while we still hold it. A taken-over slot belongs
        // to a live scan, and clearing it would let a third one in beside it.
        let _ =
            UPDATE_CHECK_STARTED.compare_exchange(self.0, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// INVARIANT: Run one marketplace sync and report every failure it accumulated.
///
/// The log line is the whole point of the wrapper. Every caller discards the
/// report, so a marketplace whose git fetch fails, or a corrupt
/// `marketplaces.json`, used to leave the five-minute check running in silence.
pub async fn run_plugin_marketplace_update_check(
    engine: SharedEngine,
    pool: sqlx::PgPool,
    cause: ScanCause,
) -> PluginUpdateCheckReport {
    if cause.leaves_trailing_pass() {
        // INVARIANT: Claimed before reaching for the slot, so it cannot matter whether we
        // get it. Whoever does clears the claim and then reads the registry.
        SCAN_REQUEUE.store(true, Ordering::SeqCst);
    }
    let (mut report, mut scanned) = run_one_pass(&engine, &pool).await;
    // INVARIANT: ONLY the pass that held the slot goes round again. A refused caller
    // draining here would clear the very claim it just made, and the running
    // scan would then find nothing owed: both reviewers caught that shape.
    // A claim still set after our pass arrived while we were scanning. So it
    // landed after we read the registry, and our result does not reflect it.
    while scanned && SCAN_REQUEUE.load(Ordering::SeqCst) && !engine.is_shutting_down() {
        (report, scanned) = run_one_pass(&engine, &pool).await;
    }
    report
}

async fn run_one_pass(
    engine: &SharedEngine,
    pool: &sqlx::PgPool,
) -> (PluginUpdateCheckReport, bool) {
    let (report, scanned) = check_marketplaces_for_updates(engine.clone(), pool.clone()).await;
    for line in report.failure_log_lines() {
        log_legacy!("[PluginUpdateCheck] {line}");
    }
    (report, scanned)
}

/// INVARIANT: Stamp the *plugin catalog cache* as scanning, before the scan's own task has
/// had a chance to run.
///
/// `tokio::spawn` returns before its body does. So a caller that spawns a scan
/// and then answers an HTTP request can serve `scanning: false` for a scan it
/// just queued. On a workspace with nothing cached that reads as "No plugins
/// found", at the exact moment the user registered their first marketplace.
/// Every site that spawns a scan calls this first, synchronously.
pub fn note_scan_queued(workspace: &Path) {
    plugin_catalog_cache::mark_scan_started(workspace, Utc::now());
}

async fn scan_into_cache(
    engine: &SharedEngine,
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
        let empty = MarketplaceCatalog {
            marketplaces: vec![],
            plugins: vec![],
            errors: vec![],
        };
        record_scan_if_live(workspace, guard, &empty);
        return Some(empty);
    }

    let installed = match engine
        .agent_host()
        .installed_plugin_summaries(engine, pool)
        .await
    {
        Ok(installed) => installed,
        Err(e) => return fail(format!("read installed plugins: {e}")),
    };

    let scan_workspace = workspace.to_path_buf();
    let scan_registry = registry.clone();
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

/// INVARIANT: Publish a result only while this scan still owns the slot.
///
/// A scan taken over past the stall cutoff finishes eventually, and its rows
/// are older than the ones that replaced them. Writing them would stamp stale
/// content as fresh and clear the live scan's own start stamp.
fn record_scan_if_live(workspace: &Path, guard: &UpdateCheckGuard, catalog: &MarketplaceCatalog) {
    if guard.still_held() {
        plugin_catalog_cache::record_scan(workspace, catalog, Utc::now());
        crate::core::plugin_marketplaces::prune_media_for(workspace, catalog);
    } else {
        log_legacy!("[PluginUpdateCheck] Discarding a scan whose slot was taken over");
    }
}

async fn announce_scan(engine: &SharedEngine, event: SystemEvent, what: &str) {
    engine
        .event_bus
        .emit_or_log(
            BusEvent::System(event),
            &format!("[PluginUpdateCheck] PluginCatalogScan{what}"),
        )
        .await;
}

async fn check_marketplaces_for_updates(
    engine: SharedEngine,
    pool: sqlx::PgPool,
) -> (PluginUpdateCheckReport, bool) {
    if engine.is_shutting_down() {
        return (PluginUpdateCheckReport::default(), false);
    }
    let Some(guard) = UpdateCheckGuard::try_acquire(Utc::now().timestamp()) else {
        log_legacy!("[PluginUpdateCheck] Skipping run; another marketplace sync is active");
        return (PluginUpdateCheckReport::default(), false);
    };
    // INVARIANT: Cleared here, at the top of the pass that holds the slot, so everything
    // below reads a registry newer than any claim already made.
    SCAN_REQUEUE.store(false, Ordering::SeqCst);

    let mut report = PluginUpdateCheckReport::default();
    let workspace = engine.workspace_path().to_path_buf();

    note_scan_queued(&workspace);
    announce_scan(&engine, SystemEvent::PluginCatalogScanStarted {}, "Started").await;

    let scanned = scan_into_cache(&engine, &pool, &workspace, &guard, &mut report).await;

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
        write_notified_signature(&workspace, &BTreeSet::new());
        return (report, true);
    }

    for error in &catalog.errors {
        report.errors.push(format_scan_error(error));
    }

    let candidates = update_candidates(&catalog, &running_release());
    report.candidates = candidates.len();

    let current: BTreeSet<String> = candidates
        .iter()
        .map(|p| format!("{}@{}", p.id, p.version))
        .collect();
    let already_notified = read_notified_signature(&workspace);
    let has_new = current.difference(&already_notified).next().is_some();

    if candidates.is_empty() {
        write_notified_signature(&workspace, &current);
        return (report, true);
    }

    if !has_new {
        write_notified_signature(&workspace, &current);
        log_legacy!(
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
            // INVARIANT: Record the notified set only after the emit succeeds, so a
            // transient emit failure retries on the next scan instead of being
            // silently swallowed by the marker.
            write_notified_signature(&workspace, &current);
            log_legacy!(
                "[PluginUpdateCheck] Notified user of {} available plugin update(s)",
                candidates.len()
            );
        }
        Err(e) => {
            report
                .errors
                .push(format!("emit update notification failed: {e}"));
        }
    }

    (report, true)
}

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

/// INVARIANT: Where the update notification's tap lands: the Plugins panel's Installed tab.
/// A single candidate also scrolls to and pulses its row; the `id` is the plugin
/// id, matched against each row's `data-plugin-id`. Several candidates focus no
/// row, since each one's button already reads Update.
fn build_update_navigation(candidates: &[MarketplacePlugin]) -> NavigateUi {
    let focus = match candidates {
        [only] => Some(only.id.clone()),
        _ => None,
    };
    NavigateUi {
        target: NavigateTarget::Plugins,
        id: focus,
        ..Default::default()
    }
}

fn marker_path(workspace: &Path) -> PathBuf {
    workspace.join(UPDATE_NOTICE_MARKER)
}

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
            log_legacy!("[PluginUpdateCheck] create marker dir failed: {e}");
            return;
        }
    }
    let entries: Vec<&String> = signature.iter().collect();
    match serde_json::to_string(&entries) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                log_legacy!("[PluginUpdateCheck] write update marker failed: {e}");
            }
        }
        Err(e) => log_legacy!("[PluginUpdateCheck] serialize update marker failed: {e}"),
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
