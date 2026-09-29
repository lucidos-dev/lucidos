//! The Claude Code model picker, discovered from Claude Code itself.
//!
//! Claude Code answers an `initialize` control request with the models its own
//! `/model` picker offers, for the account, provider and model pins it runs
//! under. The engine asks it (`claude_code::probe_cc_models`), keeps only that
//! list, and serves it as the picker. `cc_menu_options.json` is the fallback
//! until the first probe succeeds, and the context-window overlay.
//!
//! The picker is exactly the discovered list, so a model Claude Code does not
//! list is not offered. See ADR 0325 for that choice and what it costs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};

use super::claude_code::CcMenuOption;

/// How long a discovered list stays fresh when nothing else says it moved.
pub const CACHE_TTL: chrono::Duration = chrono::Duration::hours(24);

/// How long a failed probe holds off the next one. Without it, every session
/// start would spawn another cold Claude Code while the failure lasts.
pub const RETRY_AFTER_FAILURE: chrono::Duration = chrono::Duration::hours(1);

/// One row of Claude Code's model list, in Claude Code's own field names.
///
/// Typed on purpose. The `initialize` reply also carries the signed-in account,
/// and a struct that names only these fields cannot carry it into the cache.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_model: Option<String>,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_effort: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_effort_levels: Option<Vec<String>>,
}

impl DiscoveredModel {
    /// The tiers this model accepts. A model that does not support effort
    /// offers none. `None` means it supports effort without naming tiers, so
    /// the effort table answers.
    fn reasoning_efforts(&self) -> Option<Vec<String>> {
        match self.supports_effort {
            Some(true) => self.supported_effort_levels.clone(),
            _ => Some(Vec::new()),
        }
    }
}

/// What one successful probe found, as persisted in `.lucidos/cc-models.json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CcModelCache {
    /// `claude --version` at probe time, when it answered.
    pub cc_version: Option<String>,
    pub discovered_at: DateTime<Utc>,
    pub models: Vec<DiscoveredModel>,
}

/// Where the picker's list came from, for the commands route to report.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Provenance {
    pub source: ModelsSource,
    pub discovered_at: Option<DateTime<Utc>>,
    pub cc_version: Option<String>,
    /// The last probe's failure, kept until a later probe succeeds.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelsSource {
    Discovered,
    Fallback,
}

/// Read the model list out of an `initialize` reply body. Every other field,
/// the account included, is dropped here.
///
/// An empty list is an error, never a picker: a reply that names no models
/// says something went wrong, and serving it would leave nothing to pick.
pub fn parse_initialize_models(body: &serde_json::Value) -> Result<Vec<DiscoveredModel>, String> {
    let models = body
        .get("models")
        .ok_or("Claude Code's initialize reply has no models field")?;
    let models: Vec<DiscoveredModel> = serde_json::from_value(models.clone())
        .map_err(|e| format!("Claude Code's model list did not parse: {e}"))?;
    if models.is_empty() {
        return Err("Claude Code's initialize reply listed no models".to_string());
    }
    Ok(models)
}

/// Whether the cache should be probed again.
///
/// `reported_version` is the version a session just reported. A mismatch means
/// Claude Code updated under us, and an update is when its list moves. A cache
/// with no recorded version has nothing to compare, so only its age counts.
pub fn needs_refresh(
    cache: Option<&CcModelCache>,
    last_failure: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    reported_version: Option<&str>,
) -> bool {
    if last_failure.is_some_and(|failed| now - failed < RETRY_AFTER_FAILURE) {
        return false;
    }
    let Some(cache) = cache else { return true };
    if now - cache.discovered_at > CACHE_TTL {
        return true;
    }
    match (reported_version, cache.cc_version.as_deref()) {
        (Some(reported), Some(probed)) => reported != probed,
        _ => false,
    }
}

/// The picker rows for a discovered list, in Claude Code's order.
pub fn menu_options(models: &[DiscoveredModel]) -> Vec<CcMenuOption> {
    models
        .iter()
        .map(|m| CcMenuOption {
            value: m.value.clone(),
            label: m.display_name.clone(),
            description: m.description.clone(),
            supported_models: None,
            context_window: None,
            reasoning_efforts: m.reasoning_efforts(),
        })
        .collect()
}

pub fn cache_path(workspace_path: &Path) -> PathBuf {
    workspace_path.join(".lucidos/cc-models.json")
}

/// The cache on disk, or `None` when it is absent or unreadable. An unreadable
/// one is logged and then treated as absent, so the next probe replaces it.
pub fn load_cache(workspace_path: &Path) -> Option<CcModelCache> {
    let path = cache_path(workspace_path);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log!("[CcModels] ignoring unreadable {}: {e}", path.display());
            return None;
        }
    };
    serde_json::from_slice(&bytes)
        .map_err(|e| log!("[CcModels] ignoring unreadable {}: {e}", path.display()))
        .ok()
}

pub fn save_cache(
    workspace_path: &Path,
    cache: &CcModelCache,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    crate::core::write_json_atomic(&cache_path(workspace_path), cache)
}

/// This engine's discovered list. One engine serves one workspace, so a
/// process-wide value is per workspace.
struct Discovery {
    cache: Option<CcModelCache>,
    options: Option<Arc<[CcMenuOption]>>,
    error: Option<String>,
    last_failure: Option<DateTime<Utc>>,
    probing: bool,
}

static DISCOVERY: RwLock<Discovery> = RwLock::new(Discovery {
    cache: None,
    options: None,
    error: None,
    last_failure: None,
    probing: false,
});

/// A poisoned lock means a writer panicked between two plain assignments. The
/// value is whole either way, so carry on rather than lose the picker.
fn read() -> std::sync::RwLockReadGuard<'static, Discovery> {
    DISCOVERY.read().unwrap_or_else(|e| e.into_inner())
}

fn write() -> std::sync::RwLockWriteGuard<'static, Discovery> {
    DISCOVERY.write().unwrap_or_else(|e| e.into_inner())
}

/// The discovered picker rows, or `None` until a probe has succeeded.
pub fn discovered_options() -> Option<Arc<[CcMenuOption]>> {
    read().options.clone()
}

/// Make `cache` the picker. Clears any earlier probe error.
pub fn install(cache: CcModelCache) {
    let options: Arc<[CcMenuOption]> = menu_options(&cache.models).into();
    let mut state = write();
    state.options = Some(options);
    state.cache = Some(cache);
    state.error = None;
    state.last_failure = None;
}

/// Record a failed probe. The last good list stays the picker, and the next
/// probe waits [`RETRY_AFTER_FAILURE`].
pub fn record_error(message: String) {
    let mut state = write();
    state.error = Some(message);
    state.last_failure = Some(Utc::now());
}

pub fn provenance() -> Provenance {
    let state = read();
    Provenance {
        source: if state.options.is_some() {
            ModelsSource::Discovered
        } else {
            ModelsSource::Fallback
        },
        discovered_at: state.cache.as_ref().map(|c| c.discovered_at),
        cc_version: state.cache.as_ref().and_then(|c| c.cc_version.clone()),
        error: state.error.clone(),
    }
}

/// Claim the one probe slot when the list needs a probe. `None` when it does
/// not, or when a probe is already running, so a burst of session starts
/// spawns one probe, not one each. Checked and claimed under one lock.
pub fn claim_refresh(reported_version: Option<&str>) -> Option<ProbeSlot> {
    let mut state = write();
    let due = needs_refresh(
        state.cache.as_ref(),
        state.last_failure,
        Utc::now(),
        reported_version,
    );
    if !due || state.probing {
        return None;
    }
    state.probing = true;
    Some(ProbeSlot)
}

/// Frees the probe slot when dropped, however the probe ended.
pub struct ProbeSlot;

impl Drop for ProbeSlot {
    fn drop(&mut self) {
        write().probing = false;
    }
}

#[cfg(test)]
#[path = "cc_model_discovery_tests.rs"]
mod tests;
