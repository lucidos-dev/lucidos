//! The Claude Code model picker, discovered from Claude Code itself.
//!
//! Claude Code answers an `initialize` control request with the models its own
//! `/model` picker offers, for the account, provider and model pins it runs
//! under. The engine asks it (`claude_code::probe_cc_models`) and offers its
//! rows first. The offered curated rows of `cc_menu_options.json` follow, for a
//! model Claude Code accepts but did not list. The curated file is also the
//! fallback until the first probe succeeds, and the context-window overlay.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};

use super::claude_code::{offered_cc_model_options, CcMenuOption};
use super::AccountPin;

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
    /// The account the probe ran under. `None` in a cache written before the
    /// engine recorded it, or when `$HOME` was unset.
    #[serde(default)]
    pub account: Option<AccountPin>,
    pub discovered_at: DateTime<Utc>,
    pub models: Vec<DiscoveredModel>,
}

/// Base model ids Claude Code substituted for one account: a session that
/// asked for one ran another. Persisted in `.lucidos/cc-refused-models.json`.
///
/// Dated by the version the refusing session reported, never by the probe
/// cache: a session on a new Claude Code refuses before its probe lands.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CcModelRefusals {
    pub cc_version: Option<String>,
    pub account: Option<AccountPin>,
    pub models: Vec<String>,
}

impl CcModelRefusals {
    /// Record `model` as refused by Claude Code `cc_version` on `account`,
    /// dropping refusals of another version or account first. False when
    /// nothing changed.
    pub fn refuse(
        &mut self,
        model: &str,
        cc_version: Option<&str>,
        account: Option<&AccountPin>,
    ) -> bool {
        if self.cc_version.as_deref() != cc_version || self.account.as_ref() != account {
            *self = Self {
                cc_version: cc_version.map(str::to_string),
                account: account.cloned(),
                models: Vec::new(),
            };
        }
        let base = super::claude_code::cc_model_base(model);
        if self.models.iter().any(|r| r == base) {
            return false;
        }
        self.models.push(base.to_string());
        true
    }

    /// Whether these refusals still hold for a probe of Claude Code
    /// `cc_version`. An unknown version on either side never matches.
    pub fn hold_for(&self, cc_version: Option<&str>) -> bool {
        self.cc_version.is_some() && self.cc_version.as_deref() == cc_version
    }
}

/// The offered list: the cache's rows, then the curated rest, minus refusals.
/// With no cache it is the curated fallback, minus refusals.
pub fn offered_list(
    cache: Option<&CcModelCache>,
    refusals: &CcModelRefusals,
) -> Arc<[CcMenuOption]> {
    let discovered = cache.map(|c| menu_options(&c.models));
    offered_cc_model_options(discovered.as_deref(), &refusals.models)
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
///
/// `reported_account` is the live account that session saw. Another account
/// offers other models, so a cache probed under another one is stale.
pub fn needs_refresh(
    cache: Option<&CcModelCache>,
    last_failure: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    reported_version: Option<&str>,
    reported_account: Option<&AccountPin>,
) -> bool {
    if last_failure.is_some_and(|failed| now - failed < RETRY_AFTER_FAILURE) {
        return false;
    }
    let Some(cache) = cache else { return true };
    if now - cache.discovered_at > CACHE_TTL {
        return true;
    }
    if reported_account.is_some_and(|live| cache.account.as_ref() != Some(live)) {
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
            resolved_model: m.resolved_model.clone(),
            label: m.display_name.clone(),
            description: m.description.clone(),
            supported_models: None,
            context_window: None,
            reasoning_efforts: m.reasoning_efforts(),
            default_effort: None,
            offered: true,
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

pub fn refusals_path(workspace_path: &Path) -> PathBuf {
    workspace_path.join(".lucidos/cc-refused-models.json")
}

/// The refusals on disk. Absent or unreadable reads as none: a refused model
/// is then offered again, and the next session refuses it again.
pub fn load_refusals(workspace_path: &Path) -> CcModelRefusals {
    let path = refusals_path(workspace_path);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| log!("[CcModels] ignoring unreadable {}: {e}", path.display()))
            .unwrap_or_default(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CcModelRefusals::default(),
        Err(e) => {
            log!("[CcModels] ignoring unreadable {}: {e}", path.display());
            CcModelRefusals::default()
        }
    }
}

fn save_refusals(workspace_path: &Path, refusals: &CcModelRefusals) {
    if let Err(e) = crate::core::write_json_atomic(&refusals_path(workspace_path), refusals) {
        log!("[CcModels] Could not save the refused models: {e}");
    }
}

/// This engine's discovered list. One engine serves one workspace, so a
/// process-wide value is per workspace.
struct Discovery {
    cache: Option<CcModelCache>,
    refusals: CcModelRefusals,
    options: Option<Arc<[CcMenuOption]>>,
    error: Option<String>,
    last_failure: Option<DateTime<Utc>>,
    probing: bool,
}

impl Discovery {
    const fn new() -> Self {
        Self {
            cache: None,
            refusals: CcModelRefusals {
                cc_version: None,
                account: None,
                models: Vec::new(),
            },
            options: None,
            error: None,
            last_failure: None,
            probing: false,
        }
    }

    /// The offered list is `None`, the static curated rows, only while
    /// there is neither a probe result nor a refusal to apply.
    fn rebuild_options(&mut self) {
        self.options = (self.cache.is_some() || !self.refusals.models.is_empty())
            .then(|| offered_list(self.cache.as_ref(), &self.refusals));
    }

    /// Drop every refusal. False when there were none.
    fn clear_refusals(&mut self) -> bool {
        if self.refusals.models.is_empty() {
            return false;
        }
        self.refusals = CcModelRefusals::default();
        self.rebuild_options();
        true
    }

    /// Take a probe's result. True when that dropped refusals of another
    /// Claude Code version, which the caller must save.
    fn install(&mut self, cache: CcModelCache) -> bool {
        let stale = !self.refusals.hold_for(cache.cc_version.as_deref());
        self.cache = Some(cache);
        self.error = None;
        self.last_failure = None;
        let cleared = stale && self.clear_refusals();
        self.rebuild_options();
        cleared
    }

    /// Record a refusal by a session on `session`. Only a session on the live
    /// account counts: the probe runs under it, so it owns the picker. True
    /// when the refusals changed.
    fn refuse(
        &mut self,
        model: &str,
        cc_version: Option<&str>,
        session: Option<&AccountPin>,
        live: Option<&AccountPin>,
    ) -> bool {
        if session != live || !self.refusals.refuse(model, cc_version, live) {
            return false;
        }
        self.rebuild_options();
        true
    }

    /// A session reported `live` as the live account. Refusals another account
    /// learned go. True when they did.
    fn observe_live_account(&mut self, live: Option<&AccountPin>) -> bool {
        self.refusals.account.as_ref() != live && self.clear_refusals()
    }
}

static DISCOVERY: RwLock<Discovery> = RwLock::new(Discovery::new());

/// A poisoned lock means a writer panicked between two plain assignments. The
/// value is whole either way, so carry on rather than lose the picker.
fn read() -> std::sync::RwLockReadGuard<'static, Discovery> {
    DISCOVERY.read().unwrap_or_else(|e| e.into_inner())
}

fn write() -> std::sync::RwLockWriteGuard<'static, Discovery> {
    DISCOVERY.write().unwrap_or_else(|e| e.into_inner())
}

/// The offered rows: Claude Code's list, then the curated rows it omitted,
/// minus refusals. `None` while the static curated rows apply unchanged.
pub fn offered_options() -> Option<Arc<[CcMenuOption]>> {
    read().options.clone()
}

/// Serve what the last run left on disk. Called once at boot.
pub fn load(workspace_path: &Path) {
    let cache = load_cache(workspace_path);
    let refusals = load_refusals(workspace_path);
    let mut state = write();
    state.cache = cache;
    state.refusals = refusals;
    state.rebuild_options();
}

// The three writers below save refusals under the lock, so an older state
// can never overwrite a newer one on disk.

/// Make a probe's result the discovered half of the offered list, and clear
/// any earlier probe error. Refusals from another Claude Code version go.
pub fn install(workspace_path: &Path, cache: CcModelCache) {
    let mut state = write();
    if state.install(cache) {
        save_refusals(workspace_path, &state.refusals);
    }
}

/// Stop offering `model`, refused by Claude Code `cc_version` in a session on
/// `session`. False when nothing changed: the refusal was known, or the
/// session ran on another account than the live one.
pub fn record_refusal(
    workspace_path: &Path,
    model: &str,
    cc_version: Option<&str>,
    session: Option<&AccountPin>,
    live: Option<&AccountPin>,
) -> bool {
    let mut state = write();
    if !state.refuse(model, cc_version, session, live) {
        return false;
    }
    save_refusals(workspace_path, &state.refusals);
    true
}

/// A session reported `live` as the live account. Drops refusals another
/// account learned, so a switch of account offers its models again.
pub fn observe_live_account(workspace_path: &Path, live: Option<&AccountPin>) {
    let mut state = write();
    if state.observe_live_account(live) {
        save_refusals(workspace_path, &state.refusals);
    }
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
        source: if state.cache.is_some() {
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
pub fn claim_refresh(
    reported_version: Option<&str>,
    reported_account: Option<&AccountPin>,
) -> Option<ProbeSlot> {
    let mut state = write();
    let due = needs_refresh(
        state.cache.as_ref(),
        state.last_failure,
        Utc::now(),
        reported_version,
        reported_account,
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
