//! The plugin catalog cache: what a marketplace scan found, kept on disk so a
//! page open never waits for one.
//!
//! A scan git-clones every registered marketplace, which costs seconds per
//! repo. `GET /api/v1/plugins/catalog` used to run one per request. It now
//! reads this file instead. The scan runs on the scheduler
//! (`scheduler::plugin_updates`), whose pass already ran every five minutes and
//! discarded everything but the update candidates.
//!
//! The file lives under `.lucidos/`, the rebuildable runtime cache. Losing it
//! costs one scan, so every read here is tolerant: a missing, truncated or
//! invalid file reads as empty rather than failing the page.
//!
//! Reasoning and rejected alternatives:
//! `docs/plans/2026-09-22-plugin-catalog-served-from-cache.md`.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::core::plugin_marketplaces::{
    MarketplaceCatalog, MarketplacePlugin, MarketplaceScanError, PluginMarketplaceRegistry,
};

const CACHE_PATH: &str = ".lucidos/plugin-catalog.json";

/// How old a cached scan may be before a page open triggers a fresh one.
///
/// Matched to `MARKETPLACE_UPDATE_CHECK_CRON`, so the scheduler normally gets
/// there first and a page open triggers nothing at all.
const CACHE_TTL_SECS: i64 = 300;

/// How long a scan may claim to be running before we stop believing it.
///
/// `shallow_clone` has no timeout, so a wedged clone holds the single-flight
/// guard for good. Without this bound the panel would show "Updating…" for
/// ever, hiding the fact that its data has stopped moving. Past the cutoff the
/// cue drops and the data's real age is shown instead.
pub const SCAN_STALL_CUTOFF_SECS: i64 = 600;

/// What the last scan produced, plus what is known about the scan itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CachedCatalog {
    #[serde(default)]
    pub plugins: Vec<MarketplacePlugin>,
    #[serde(default)]
    pub errors: Vec<MarketplaceScanError>,
    /// When the scan that produced `plugins` finished. `None` on a workspace
    /// that has never completed one.
    #[serde(default)]
    pub scanned_at: Option<DateTime<Utc>>,
    /// When the scan believed to be running now started. Cleared when it ends.
    #[serde(default)]
    pub scan_started_at: Option<DateTime<Utc>>,
    /// Why the last scan could not complete at all, as opposed to the
    /// per-marketplace failures in `errors`. Cleared by the next good scan.
    #[serde(default)]
    pub scan_error: Option<String>,
}

pub fn cache_path(workspace_path: &Path) -> PathBuf {
    workspace_path.join(CACHE_PATH)
}

/// Read the cache, treating every failure as "nothing cached yet".
///
/// The loader logs a corrupt file and discards it. The next scan overwrites
/// it, so there is nothing to recover and nothing to tell the user.
pub fn load(workspace_path: &Path) -> CachedCatalog {
    let path = cache_path(workspace_path);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return CachedCatalog::default(),
        Err(e) => {
            log!("[PluginCatalogCache] read {} failed: {e}", path.display());
            return CachedCatalog::default();
        }
    };
    match serde_json::from_str(&text) {
        Ok(cached) => cached,
        Err(e) => {
            log!("[PluginCatalogCache] parse {} failed: {e}", path.display());
            CachedCatalog::default()
        }
    }
}

/// Write the cache through a temp file and a rename, so a crash mid-write
/// leaves the previous copy intact rather than a half-parsed one.
pub fn save(
    workspace_path: &Path,
    cached: &CachedCatalog,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = cache_path(workspace_path);
    let dir = path
        .parent()
        .ok_or_else(|| format!("no parent directory for {}", path.display()))?;
    std::fs::create_dir_all(dir)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(&mut temp, cached)?;
    temp.write_all(b"\n")?;
    temp.flush()?;
    temp.persist(&path)?;
    Ok(())
}

/// Serializes the cache's read-modify-write transitions.
///
/// The atomic rename in [`save`] makes one write all-or-nothing, and says
/// nothing about two writers. Two of the three transitions below load, mutate
/// and save. A `record_scan` landing between one's load and its save is
/// reverted: the fresh plugins and `scanned_at` go back to what the loser read,
/// throwing a whole scan away. The window is real, because a page open calls
/// `mark_scan_started` from an HTTP thread while a scan task is finishing.
///
/// Held across a small file read and write, and never across an await.
static CACHE_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// A poisoned lock means a previous writer panicked mid-transition. The file it
/// left is whole either way, so carry on rather than poison every later write.
fn lock_cache() -> MutexGuard<'static, ()> {
    CACHE_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Record that a scan has begun, keeping whatever the last one found.
///
/// The stamp is what `scan_is_running` reads, so it is the whole basis of the
/// "Updating…" cue. Every client learns a scan started, including one that did
/// not ask for it.
pub fn mark_scan_started(workspace_path: &Path, now: DateTime<Utc>) {
    let _serialized = lock_cache();
    let mut cached = load(workspace_path);
    cached.scan_started_at = Some(now);
    write_or_log(workspace_path, &cached, "mark scan started");
}

/// Record a completed scan. Replaces the plugins and clears the scan error.
pub fn record_scan(workspace_path: &Path, catalog: &MarketplaceCatalog, now: DateTime<Utc>) {
    // Takes the lock although it reads nothing, so it cannot land inside
    // another transition's read-modify-write and be undone by it.
    let _serialized = lock_cache();
    let cached = CachedCatalog {
        plugins: catalog.plugins.clone(),
        errors: catalog.errors.clone(),
        scanned_at: Some(now),
        scan_started_at: None,
        scan_error: None,
    };
    write_or_log(workspace_path, &cached, "record scan");
}

/// Record a scan that could not finish, keeping the plugins it could not
/// replace. Stale data beats no data, and the reason rides along so the page
/// can say why its data stopped moving.
pub fn record_scan_failure(workspace_path: &Path, error: &str) {
    let _serialized = lock_cache();
    let mut cached = load(workspace_path);
    cached.scan_started_at = None;
    cached.scan_error = Some(error.to_string());
    write_or_log(workspace_path, &cached, "record scan failure");
}

fn write_or_log(workspace_path: &Path, cached: &CachedCatalog, what: &str) {
    if let Err(e) = save(workspace_path, cached) {
        log!("[PluginCatalogCache] {what} failed: {e}");
    }
}

/// How long ago `then` was, treating a stamp from the future as ancient.
///
/// The stamps are wall-clock, so a clock moved backwards leaves one ahead of
/// `now` and every `now - then` comparison inverts. Both callers below would
/// then latch: the cue would stay up for ever, and the rescan would never come
/// due. ADR 0053 records the same hazard on the DB side. Reading a future stamp
/// as ancient fails towards scanning again, which is the recoverable direction.
fn age(then: DateTime<Utc>, now: DateTime<Utc>) -> Duration {
    let elapsed = now - then;
    if elapsed < Duration::zero() {
        Duration::MAX
    } else {
        elapsed
    }
}

/// Is a scan running right now, as far as anyone can tell?
///
/// A start stamp past [`SCAN_STALL_CUTOFF_SECS`] reads as "no": see that
/// constant for why a wedged clone must not pin the cue on for ever.
pub fn scan_is_running(scan_started_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    match scan_started_at {
        Some(started) => age(started, now) < Duration::seconds(SCAN_STALL_CUTOFF_SECS),
        None => false,
    }
}

/// Should opening a page trigger a fresh scan?
///
/// Yes when nothing has ever been scanned, or when the last one is older than
/// the scheduler's own interval. A scan already running is never doubled up.
pub fn needs_rescan(cached: &CachedCatalog, now: DateTime<Utc>) -> bool {
    if scan_is_running(cached.scan_started_at, now) {
        return false;
    }
    match cached.scanned_at {
        None => true,
        Some(scanned) => age(scanned, now) >= Duration::seconds(CACHE_TTL_SECS),
    }
}

/// Build the catalog a client sees: the live registry, plus the cached plugins
/// and errors that still belong to it.
///
/// The marketplace list is never cached. It is one small file read, and it is
/// the source of truth. So Settings → Marketplaces shows a rename at once,
/// while the plugins behind it are still minutes old.
///
/// The filter is what keeps a stale cache honest. A marketplace removed since
/// the scan would otherwise keep offering installable plugins.
pub fn merge_with_registry(
    cached: &CachedCatalog,
    registry: &PluginMarketplaceRegistry,
) -> MarketplaceCatalog {
    let registered: HashSet<&str> = registry
        .marketplaces
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    MarketplaceCatalog {
        marketplaces: registry.marketplaces.clone(),
        plugins: cached
            .plugins
            .iter()
            .filter(|p| registered.contains(p.marketplace_id.as_str()))
            .cloned()
            .collect(),
        errors: cached
            .errors
            .iter()
            .filter(|e| registered.contains(e.marketplace_id.as_str()))
            .cloned()
            .collect(),
    }
}

#[cfg(test)]
#[path = "plugin_catalog_cache_tests.rs"]
mod tests;
