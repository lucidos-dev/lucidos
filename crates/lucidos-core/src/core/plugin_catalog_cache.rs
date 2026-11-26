use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::core::plugin_marketplaces::{
    MarketplaceCatalog, MarketplacePlugin, MarketplaceScanError, PluginMarketplaceRegistry,
};

const CACHE_PATH: &str = ".lucidos/plugin-catalog.json";

/// INVARIANT: How old a cached scan may be before a page open triggers a fresh one.
///
/// Matched to `MARKETPLACE_UPDATE_CHECK_CRON`, so the scheduler normally gets
/// there first and a page open triggers nothing at all.
const CACHE_TTL_SECS: i64 = 300;

/// INVARIANT: How long a scan may claim to be running before we stop believing it.
///
/// `shallow_clone` has no timeout, so a wedged clone holds the single-flight
/// guard for good. Without this bound the panel would believe a scan is
/// running for ever, and a failure notice would say "Checking again…" with
/// nothing behind it. Past the cutoff the scan stops counting as running.
pub const SCAN_STALL_CUTOFF_SECS: i64 = 600;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CachedCatalog {
    #[serde(default)]
    pub plugins: Vec<MarketplacePlugin>,
    #[serde(default)]
    pub errors: Vec<MarketplaceScanError>,
    #[serde(default)]
    pub scanned_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scan_started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scan_error: Option<String>,
}

pub fn cache_path(workspace_path: &Path) -> PathBuf {
    workspace_path.join(CACHE_PATH)
}

pub fn load(workspace_path: &Path) -> CachedCatalog {
    let path = cache_path(workspace_path);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return CachedCatalog::default(),
        Err(e) => {
            log_legacy!("[PluginCatalogCache] read {} failed: {e}", path.display());
            return CachedCatalog::default();
        }
    };
    match serde_json::from_str(&text) {
        Ok(cached) => cached,
        Err(e) => {
            log_legacy!("[PluginCatalogCache] parse {} failed: {e}", path.display());
            CachedCatalog::default()
        }
    }
}

pub fn save(
    workspace_path: &Path,
    cached: &CachedCatalog,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    crate::core::write_json_atomic(&cache_path(workspace_path), cached)
}

/// INVARIANT: Serializes the cache's read-modify-write transitions.
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

fn lock_cache() -> MutexGuard<'static, ()> {
    CACHE_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn mark_scan_started(workspace_path: &Path, now: DateTime<Utc>) {
    let _serialized = lock_cache();
    let mut cached = load(workspace_path);
    cached.scan_started_at = Some(now);
    write_or_log(workspace_path, &cached, "mark scan started");
}

pub fn record_scan(workspace_path: &Path, catalog: &MarketplaceCatalog, now: DateTime<Utc>) {
    // INVARIANT: Takes the lock although it reads nothing, so it cannot land inside
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

pub fn record_scan_failure(workspace_path: &Path, error: &str) {
    let _serialized = lock_cache();
    let mut cached = load(workspace_path);
    cached.scan_started_at = None;
    cached.scan_error = Some(error.to_string());
    write_or_log(workspace_path, &cached, "record scan failure");
}

fn write_or_log(workspace_path: &Path, cached: &CachedCatalog, what: &str) {
    if let Err(e) = save(workspace_path, cached) {
        log_legacy!("[PluginCatalogCache] {what} failed: {e}");
    }
}

fn age(then: DateTime<Utc>, now: DateTime<Utc>) -> Duration {
    let elapsed = now - then;
    if elapsed < Duration::zero() {
        Duration::MAX
    } else {
        elapsed
    }
}

pub fn scan_is_running(scan_started_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    match scan_started_at {
        Some(started) => age(started, now) < Duration::seconds(SCAN_STALL_CUTOFF_SECS),
        None => false,
    }
}

pub fn needs_rescan(cached: &CachedCatalog, now: DateTime<Utc>) -> bool {
    if scan_is_running(cached.scan_started_at, now) {
        return false;
    }
    match cached.scanned_at {
        None => true,
        Some(scanned) => age(scanned, now) >= Duration::seconds(CACHE_TTL_SECS),
    }
}

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
