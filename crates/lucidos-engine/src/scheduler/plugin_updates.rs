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
use std::sync::atomic::{AtomicBool, Ordering};

use crate::api::SharedEngine;
use crate::core::plugin_marketplaces::{
    load_registry, scan_catalog, update_candidates, MarketplacePlugin, MarketplaceScanError,
};
use crate::engine::tools::plugins::installed_plugin_summaries;
use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};

static UPDATE_CHECK_RUNNING: AtomicBool = AtomicBool::new(false);

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

struct UpdateCheckGuard;

impl UpdateCheckGuard {
    fn try_acquire() -> Option<Self> {
        UPDATE_CHECK_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for UpdateCheckGuard {
    fn drop(&mut self) {
        UPDATE_CHECK_RUNNING.store(false, Ordering::SeqCst);
    }
}

pub(crate) async fn run_plugin_marketplace_update_check(
    engine: SharedEngine,
    pool: sqlx::PgPool,
) -> PluginUpdateCheckReport {
    let Some(_guard) = UpdateCheckGuard::try_acquire() else {
        log!("[PluginUpdateCheck] Skipping run; another marketplace sync is active");
        return PluginUpdateCheckReport::default();
    };

    if engine.is_shutting_down() {
        return PluginUpdateCheckReport::default();
    }

    let mut report = PluginUpdateCheckReport::default();
    let workspace = engine.workspace_path().to_path_buf();
    let registry = match load_registry(&workspace) {
        Ok(registry) => registry,
        Err(e) => {
            report
                .errors
                .push(format!("read marketplace registry: {e}"));
            return report;
        }
    };
    report.marketplaces = registry.marketplaces.len();
    if registry.marketplaces.is_empty() {
        // No marketplaces → nothing can have an update. Clear any stale marker
        // so a future re-registration starts from a clean slate.
        write_notified_signature(&workspace, &BTreeSet::new());
        return report;
    }

    let installed = match installed_plugin_summaries(&pool).await {
        Ok(installed) => installed,
        Err(e) => {
            report.errors.push(format!("read installed plugins: {e}"));
            return report;
        }
    };

    let scan_workspace = workspace.clone();
    let scan_registry = registry.clone();
    let catalog = match tokio::task::spawn_blocking(move || {
        scan_catalog(&scan_workspace, &scan_registry, &installed)
    })
    .await
    {
        Ok(catalog) => catalog,
        Err(e) => {
            report
                .errors
                .push(format!("scan marketplaces task panicked: {e}"));
            return report;
        }
    };

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
        if !report.errors.is_empty() {
            log!(
                "[PluginUpdateCheck] Marketplace sync completed with {} scan error(s)",
                report.errors.len()
            );
        }
        return report;
    }

    if !has_new {
        // Persist the (possibly shrunk) set so applied updates drop out of the
        // marker — nothing to notify here, so there's no notification at risk.
        write_notified_signature(&workspace, &current);
        log!(
            "[PluginUpdateCheck] {} update(s) available; already notified",
            candidates.len()
        );
        return report;
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
                to: NavigateUi {
                    target: NavigateTarget::Apps,
                    ..Default::default()
                },
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
            let msg = format!("emit update notification failed: {e}");
            log!("[PluginUpdateCheck] {msg}");
            report.errors.push(msg);
        }
    }

    report
}

/// Title + body for the "updates available" notification. Singular and plural
/// phrasings; the body lists plugin names so the user knows what's waiting
/// before opening the Apps section.
fn build_update_notification(candidates: &[MarketplacePlugin]) -> (String, String) {
    if let [only] = candidates {
        return (
            "Plugin update available".to_string(),
            format!(
                "{} v{} is ready to update. Open Apps to review.",
                only.name, only.version
            ),
        );
    }
    let mut names: Vec<&str> = candidates.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    (
        "Plugin updates available".to_string(),
        format!(
            "{} plugins have updates: {}. Open Apps to review.",
            candidates.len(),
            names.join(", ")
        ),
    )
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
