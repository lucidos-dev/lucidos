use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};

use super::coding_agent_models::{
    cc_model_base, offered_cc_model_options, AccountPin, CcMenuOption,
};

pub const CACHE_TTL: chrono::Duration = chrono::Duration::hours(24);

pub const RETRY_AFTER_FAILURE: chrono::Duration = chrono::Duration::hours(1);

/// INVARIANT: One row of Claude Code's model list, in Claude Code's own field names.
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
    /// INVARIANT: The tiers this model accepts. A model that does not support effort
    /// offers none. `None` means it supports effort without naming tiers, so
    /// the effort table answers.
    fn reasoning_efforts(&self) -> Option<Vec<String>> {
        match self.supports_effort {
            Some(true) => self.supported_effort_levels.clone(),
            _ => Some(Vec::new()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CcModelCache {
    pub cc_version: Option<String>,
    #[serde(default)]
    pub account: Option<AccountPin>,
    pub discovered_at: DateTime<Utc>,
    pub models: Vec<DiscoveredModel>,
}

/// INVARIANT: Base model ids Claude Code substituted for one account: a session that
/// asked for one ran another. Persisted in `.lucidos/cc-refused-models.json`.
///
/// Dated by the version the refusing session reported, never by the probe
/// cache: a session on a new Claude Code refuses before its probe lands.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CcModelRefusals {
    pub cc_version: Option<String>,
    pub account: Option<AccountPin>,
    pub models: Vec<String>,
    #[serde(default)]
    pub told: Vec<String>,
}

impl CcModelRefusals {
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
                ..Self::default()
            };
        }
        if self.holds(model) {
            return false;
        }
        self.models.push(cc_model_base(model).to_string());
        true
    }

    pub fn holds(&self, model: &str) -> bool {
        let base = cc_model_base(model);
        self.models.iter().any(|r| r == base)
    }

    pub fn forget(&mut self, model: &str) -> bool {
        let before = self.models.len();
        let base = cc_model_base(model);
        self.models.retain(|r| r != base);
        self.told.retain(|r| r != base);
        self.models.len() != before
    }

    pub fn untold(&self, model: &str) -> bool {
        self.holds(model) && !self.told.iter().any(|r| r == cc_model_base(model))
    }

    pub fn tell(&mut self, model: &str) -> bool {
        if !self.untold(model) {
            return false;
        }
        self.told.push(cc_model_base(model).to_string());
        true
    }

    pub fn hold_for(&self, cc_version: Option<&str>) -> bool {
        self.cc_version.is_some() && self.cc_version.as_deref() == cc_version
    }
}

/// INVARIANT: The offered list: the cache's rows, then the curated rest. A refusal never
/// takes a row off: the owner decides whether a swapped model leaves the
/// ladder, and an outage can be temporary.
pub fn offered_list(cache: &CcModelCache) -> Arc<[CcMenuOption]> {
    offered_cc_model_options(Some(&menu_options(&cache.models)))
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Provenance {
    pub source: ModelsSource,
    pub discovered_at: Option<DateTime<Utc>>,
    pub cc_version: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelsSource {
    Discovered,
    Fallback,
}

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

pub fn load_cache(workspace_path: &Path) -> Option<CcModelCache> {
    let path = cache_path(workspace_path);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log_legacy!("[CcModels] ignoring unreadable {}: {e}", path.display());
            return None;
        }
    };
    serde_json::from_slice(&bytes)
        .map_err(|e| log_legacy!("[CcModels] ignoring unreadable {}: {e}", path.display()))
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

pub fn load_refusals(workspace_path: &Path) -> CcModelRefusals {
    let path = refusals_path(workspace_path);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| log_legacy!("[CcModels] ignoring unreadable {}: {e}", path.display()))
            .unwrap_or_default(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CcModelRefusals::default(),
        Err(e) => {
            log_legacy!("[CcModels] ignoring unreadable {}: {e}", path.display());
            CcModelRefusals::default()
        }
    }
}

fn save_refusals(workspace_path: &Path, refusals: &CcModelRefusals) {
    if let Err(e) = crate::core::write_json_atomic(&refusals_path(workspace_path), refusals) {
        log_legacy!("[CcModels] Could not save the refused models: {e}");
    }
}

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
                told: Vec::new(),
            },
            options: None,
            error: None,
            last_failure: None,
            probing: false,
        }
    }

    fn rebuild_options(&mut self) {
        self.options = self.cache.as_ref().map(offered_list);
    }

    fn clear_refusals(&mut self) -> bool {
        if self.refusals.models.is_empty() {
            return false;
        }
        self.refusals = CcModelRefusals::default();
        true
    }

    fn install(&mut self, cache: CcModelCache) -> bool {
        let stale = !self.refusals.hold_for(cache.cc_version.as_deref());
        self.cache = Some(cache);
        self.error = None;
        self.last_failure = None;
        let cleared = stale && self.clear_refusals();
        self.rebuild_options();
        cleared
    }

    fn refuse(
        &mut self,
        model: &str,
        cc_version: Option<&str>,
        session: Option<&AccountPin>,
        live: Option<&AccountPin>,
    ) -> bool {
        session == live && self.refusals.refuse(model, cc_version, live)
    }

    fn serve(
        &mut self,
        model: &str,
        session: Option<&AccountPin>,
        live: Option<&AccountPin>,
    ) -> bool {
        session == live && self.refusals.forget(model)
    }

    fn observe_live_account(&mut self, live: Option<&AccountPin>) -> bool {
        self.refusals.account.as_ref() != live && self.clear_refusals()
    }
}

static DISCOVERY: RwLock<Discovery> = RwLock::new(Discovery::new());

/// INVARIANT: A poisoned lock means a writer panicked between two plain assignments. The
/// value is whole either way, so carry on rather than lose the picker.
fn read() -> std::sync::RwLockReadGuard<'static, Discovery> {
    DISCOVERY.read().unwrap_or_else(|e| e.into_inner())
}

fn write() -> std::sync::RwLockWriteGuard<'static, Discovery> {
    DISCOVERY.write().unwrap_or_else(|e| e.into_inner())
}

pub fn offered_options() -> Option<Arc<[CcMenuOption]>> {
    read().options.clone()
}

pub fn is_refused(model: &str) -> bool {
    read().refusals.holds(model)
}

pub fn refusal_untold(model: &str) -> bool {
    read().refusals.untold(model)
}

pub fn record_refusal_told(workspace_path: &Path, model: &str) -> bool {
    let mut state = write();
    if !state.refusals.tell(model) {
        return false;
    }
    save_refusals(workspace_path, &state.refusals);
    true
}

pub fn load(workspace_path: &Path) {
    let cache = load_cache(workspace_path);
    let refusals = load_refusals(workspace_path);
    let mut state = write();
    state.cache = cache;
    state.refusals = refusals;
    state.rebuild_options();
}

// INVARIANT: The writers below save refusals under the lock, so an older state can
// never overwrite a newer one on disk.

pub fn install(workspace_path: &Path, cache: CcModelCache) {
    let mut state = write();
    if state.install(cache) {
        save_refusals(workspace_path, &state.refusals);
    }
}

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

pub fn record_served(
    workspace_path: &Path,
    model: &str,
    session: Option<&AccountPin>,
    live: Option<&AccountPin>,
) -> bool {
    let mut state = write();
    if !state.serve(model, session, live) {
        return false;
    }
    save_refusals(workspace_path, &state.refusals);
    true
}

pub fn observe_live_account(workspace_path: &Path, live: Option<&AccountPin>) {
    let mut state = write();
    if state.observe_live_account(live) {
        save_refusals(workspace_path, &state.refusals);
    }
}

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

/// INVARIANT: Claim the one probe slot when the list needs a probe. `None` when it does
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

pub struct ProbeSlot;

impl Drop for ProbeSlot {
    fn drop(&mut self) {
        write().probing = false;
    }
}

#[cfg(test)]
#[path = "cc_model_discovery_tests.rs"]
mod tests;
