//! Plugin LLM tools: install / update / uninstall flows.
//!
//! This module owns the user-confirm staging flows (install + uninstall),
//! the `LucidosEngine` tool dispatch, and the shared pending-map plumbing.
//! Two helper concerns live in child modules:
//!
//! - [`source`] — install-source detection + fetching.
//! - [`registry`] — installed-plugin projection / query / update-check.
//!
//! Splitting is purely structural; the public surface stays reachable at
//! `crate::engine::tools::plugins::*` via the re-exports below.


use std::path::{Path, PathBuf};

use crate::core::plugins::{
    compare_versions, detect_conflicts, validate_tree, AUTH_MODULES_DIR, PlannedFile,
    UpdateDecision,
};
use crate::core::DATA_DIR;
use crate::engine::event_bus::{BusEvent, EventBusEmitter, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::LucidosEngine;

mod registry;
mod source;

use registry::{
    check_plugin_updates_impl, fetch_remote_manifest, latest_install, resolve_plugin_query,
};
use source::{copy_atomic, detect_source, fetch_source, SourceType};

// Named by other modules via the `plugins::` path (`engine::tools::files`
// routes plugin-owned deletes here), so it stays a re-export.
pub(crate) use registry::find_plugin_owning_file;

/// Sentinel prefix on the `install_plugin` / `update_plugin` tool result. The
/// agentic loop strips it and re-emits a transient
/// `ThreadEvent::PluginInstallRequested` so the frontend can render the install
/// panel. Mirrors the credentials pattern in
/// `engine::tools::credentials::CREDENTIAL_REQUEST_PREFIX`.
pub(crate) const PLUGIN_INSTALL_REQUEST_PREFIX: &str = "[PLUGIN_INSTALL_REQUEST]";

/// Sentinel prefix on the `uninstall_plugin` tool result. Same pattern as
/// `PLUGIN_INSTALL_REQUEST_PREFIX` — agentic loop intercepts, emits a transient
/// `ThreadEvent::PluginUninstallRequested`, and the frontend renders the
/// uninstall confirm panel. Symmetric with install so the LLM cannot
/// hallucinate "uninstalled" without the user seeing the panel.
pub(crate) const PLUGIN_UNINSTALL_REQUEST_PREFIX: &str = "[PLUGIN_UNINSTALL_REQUEST]";

/// Stale pending installs older than this are dropped on next access — keeps
/// the staging dir from leaking forever if the user never confirms or
/// cancels.
const PENDING_INSTALL_TTL_SECS: i64 = 60 * 60;

/// One plugin uninstall awaiting user confirmation. Mirrors `PendingInstall`
/// but holds a flat file list instead of a staged temp dir — uninstall has
/// no source bytes to keep alive between prepare and confirm.
pub struct PendingUninstall {
    pub(crate) plugin_id: String,
    pub(crate) plugin_version: String,
    pub(crate) plugin_name: String,
    /// Recorded files that exist on disk at prepare time. Confirm attempts
    /// to delete each; any that disappear before confirm fold into the
    /// resulting event's `files_missing`.
    pub(crate) files_present: Vec<String>,
    /// Recorded files already gone at prepare time. Shown in the panel for
    /// transparency but never re-deleted.
    pub(crate) files_missing: Vec<String>,
    pub(crate) created_at: chrono::DateTime<chrono::Utc>,
}

/// One plugin install awaiting user confirmation. Public so `LucidosEngine`
/// can hold the `pending_installs` map; fields are crate-private so only
/// the staging/confirm/cancel helpers in this module can read them.
pub struct PendingInstall {
    /// Owns the staging root; held only for its `Drop` side effect
    /// (`tempfile::TempDir::drop` recursively unlinks). No code reads it,
    /// hence the `dead_code` allow.
    #[allow(dead_code)]
    pub(crate) staging: tempfile::TempDir,
    /// Plugin root inside the staged tree (handles GitHub-tree subpaths).
    pub(crate) plugin_root: PathBuf,
    pub(crate) source_type: SourceType,
    pub(crate) source_string: String,
    pub(crate) plugin_id: String,
    pub(crate) plugin_version: String,
    pub(crate) created_at: chrono::DateTime<chrono::Utc>,
}

impl LucidosEngine {
    /// Dispatch a plugin tool by name. Returns the result string verbatim to
    /// the LLM (success, sentinel, or "Error: ..." line).
    pub(crate) async fn execute_plugin_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> super::ToolOutcome {
        match name {
            crate::llm::tool_names::INSTALL_PLUGIN => {
                let source_str = match args.get("source").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Err("Error: source is required".to_string()),
                };
                super::lift_legacy_string(self.prepare_install_request(&source_str).await)
            }
            crate::llm::tool_names::CHECK_PLUGIN_UPDATES => {
                let single_id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                super::lift_legacy_string(
                    check_plugin_updates_impl(&self.workspace_path, &self.pool, single_id).await,
                )
            }
            crate::llm::tool_names::UPDATE_PLUGIN => {
                let id = match args.get("id").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Err("Error: id is required".to_string()),
                };
                // Updates re-resolve the source from the recorded install record
                // and then funnel through the same confirm flow as a fresh
                // install — the user must consent in the panel before any
                // bytes hit `data/`. Same code path means same UI.
                let installed = match latest_install(&self.pool, &id).await {
                    Ok(Some(rec)) => rec,
                    Ok(None) => {
                        return Err(format!(
                            "Error: plugin '{}' is not currently installed (no PluginInstalled event, or already uninstalled)",
                            id
                        ));
                    }
                    Err(e) => return Err(format!("Error: read install record: {}", e)),
                };
                let installed_version = installed.version().unwrap_or("unknown").to_string();
                let source = match installed.source() {
                    Some(s) => s.to_string(),
                    None => {
                        return Err(format!(
                            "Error: installed manifest for '{}' is missing 'source' — cannot fetch latest",
                            id
                        ));
                    }
                };
                let remote = match fetch_remote_manifest(&self.workspace_path, &source).await {
                    Ok(m) => m,
                    Err(e) => return Err(format!("Error: fetch latest manifest: {}", e)),
                };
                if compare_versions(&installed_version, &remote.version)
                    == UpdateDecision::AlreadyLatest
                {
                    return Ok(format!("Already at latest (v{})", installed_version));
                }
                super::lift_legacy_string(self.prepare_install_request(&source).await)
            }
            crate::llm::tool_names::UNINSTALL_PLUGIN => {
                let id = match args.get("id").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Err("Error: id is required".to_string()),
                };
                super::lift_legacy_string(
                    prepare_uninstall_plugin(
                        &self.workspace_path,
                        &self.pool,
                        &self.pending_uninstalls,
                        &id,
                    )
                    .await,
                )
            }
            other => Err(format!("Error: unknown plugin tool '{}'", other)),
        }
    }

    /// Stage `source_str` into a temp dir, validate the manifest + tree,
    /// register the result in `pending_installs`, and return the
    /// `[PLUGIN_INSTALL_REQUEST]` sentinel so the agentic loop intercepts it.
    /// Runs the sync fetch (git clone or zip extract) on the blocking pool
    /// so the tool dispatcher's tokio worker stays free.
    async fn prepare_install_request(&self, source_str: &str) -> String {
        let workspace = self.workspace_path.clone();
        let pending = self.pending_installs.clone();
        let source = source_str.to_string();
        match tokio::task::spawn_blocking(move || {
            prepare_install_request(&workspace, &pending, &source)
        })
        .await
        {
            Ok(s) => s,
            Err(e) => format!("Error: install staging task panicked: {}", e),
        }
    }
}

type PendingInstallsMap = std::sync::Mutex<std::collections::HashMap<String, PendingInstall>>;

/// Stage `source_str` into a temp dir, validate the manifest + tree, register
/// the result in `pending_installs`, and return the
/// `[PLUGIN_INSTALL_REQUEST]<json>` sentinel. On any failure returns
/// `"Error: ..."` and no entry is registered. Free function so tests can
/// drive it without standing up a `LucidosEngine`.
pub(crate) fn prepare_install_request(
    workspace_path: &Path,
    pending_installs: &std::sync::Arc<PendingInstallsMap>,
    source_str: &str,
) -> String {
    let source = match detect_source(source_str) {
        Ok(s) => s,
        Err(e) => return format!("Error: {}", e),
    };
    let (scratch, plugin_root, source_type) = match fetch_source(workspace_path, &source) {
        Ok(t) => t,
        Err(e) => return format!("Error: {}", e),
    };
    let (manifest, planned) = match validate_tree(&plugin_root) {
        Ok(t) => t,
        Err(e) => return format!("Error: {}", e),
    };

    let data_dir = workspace_path.join(DATA_DIR);
    let overwrites = detect_conflicts(&planned, &data_dir);
    let install_id = uuid::Uuid::new_v4().to_string();

    let preview = serde_json::json!({
        "install_id": install_id,
        "source": source_str,
        "source_type": source_type.as_str(),
        "manifest": manifest.raw,
        "files": planned
            .iter()
            .map(|p| p.data_relative.clone())
            .collect::<Vec<_>>(),
        "overwrites": overwrites,
        "setup": manifest
            .setup
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
        "plugin_id": manifest.id,
        "plugin_version": manifest.version,
        "plugin_name": manifest.name,
    });

    let pending = PendingInstall {
        staging: scratch,
        plugin_root,
        source_type,
        source_string: source_str.to_string(),
        plugin_id: manifest.id.clone(),
        plugin_version: manifest.version.clone(),
        created_at: chrono::Utc::now(),
    };

    sweep_stale_pending(pending_installs);
    pending_installs
        .lock()
        .expect("pending_installs mutex poisoned")
        .insert(install_id, pending);

    format!("{PLUGIN_INSTALL_REQUEST_PREFIX}{}", preview)
}

/// `created_at` accessor for the generic `sweep_stale` helper. Implemented for
/// both `PendingInstall` and `PendingUninstall`; lets one sweep function cover
/// both maps without an `Any` dance.
trait HasCreatedAt {
    fn created_at(&self) -> chrono::DateTime<chrono::Utc>;
}

impl HasCreatedAt for PendingInstall {
    fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.created_at
    }
}

impl HasCreatedAt for PendingUninstall {
    fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.created_at
    }
}

/// Drop pending entries older than `ttl_secs`. Both install and uninstall maps
/// use this to keep abandoned entries from leaking.
fn sweep_stale<T: HasCreatedAt>(
    map: &std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, T>>>,
    ttl_secs: i64,
) {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(ttl_secs);
    let mut guard = map.lock().expect("pending map mutex poisoned");
    guard.retain(|_, entry| entry.created_at() >= cutoff);
}

fn sweep_stale_pending(map: &std::sync::Arc<PendingInstallsMap>) {
    sweep_stale(map, PENDING_INSTALL_TTL_SECS);
}

/// `uninstall_plugin(query)` core. Resolves the free-form query (id,
/// manifest name, or app folder) to the canonical plugin id via
/// `resolve_plugin_query`, looks up the latest install record, builds a
/// `PendingUninstall` (partitioning recorded files into present-on-disk vs
/// already-missing), and returns the `[PLUGIN_UNINSTALL_REQUEST]` sentinel
/// for the agentic loop to surface as the uninstall confirm panel. The
/// actual delete happens in `confirm_pending_uninstall` after the user clicks
/// Confirm — symmetric with install.
pub(crate) async fn prepare_uninstall_plugin(
    workspace_path: &Path,
    pool: &sqlx::PgPool,
    pending_uninstalls: &std::sync::Arc<PendingUninstallsMap>,
    query: &str,
) -> String {
    let id = match resolve_plugin_query(pool, query).await {
        Ok(id) => id,
        Err(msg) => return msg,
    };

    let installed = match latest_install(pool, &id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => {
            // Resolver said yes; record vanished between calls. Race or
            // concurrent uninstall — surface the same not-installed shape
            // so the LLM doesn't see a different error from the same
            // failure mode.
            return format!(
                "Error: plugin '{}' is not currently installed (no PluginInstalled event, or already uninstalled)",
                id
            );
        }
        Err(e) => return format!("Error: read install record: {}", e),
    };

    let version = installed.version().unwrap_or("unknown").to_string();
    let name = installed.name().unwrap_or(&id);
    let files = installed.files();

    prepare_uninstall_request(
        workspace_path,
        pending_uninstalls,
        &id,
        &version,
        name,
        files,
    )
}

/// Type alias kept symmetric with `PendingInstallsMap` — the engine field
/// uses the inner type, but module helpers take this for readability.
pub(crate) type PendingUninstallsMap =
    std::sync::Mutex<std::collections::HashMap<String, PendingUninstall>>;

const PENDING_UNINSTALL_TTL_SECS: i64 = 60 * 60;

/// Stage a `PendingUninstall` and return the `[PLUGIN_UNINSTALL_REQUEST]`
/// sentinel. Free function so tests can drive it without standing up a
/// `LucidosEngine`. Same TTL sweep policy as install.
pub(crate) fn prepare_uninstall_request(
    workspace_path: &Path,
    pending_uninstalls: &std::sync::Arc<PendingUninstallsMap>,
    id: &str,
    version: &str,
    name: &str,
    recorded_files: Vec<String>,
) -> String {
    let data_dir = workspace_path.join(DATA_DIR);
    let mut files_present = Vec::new();
    let mut files_missing = Vec::new();
    for rel in &recorded_files {
        if !is_safe_data_path(rel) {
            // Defense in depth: if a tampered install record carried an
            // unsafe path, never surface it to the panel as deletable.
            // Treat it as already missing for accounting purposes.
            files_missing.push(rel.clone());
            continue;
        }
        if data_dir.join(rel).exists() {
            files_present.push(rel.clone());
        } else {
            files_missing.push(rel.clone());
        }
    }

    let uninstall_id = uuid::Uuid::new_v4().to_string();

    let preview = serde_json::json!({
        "uninstall_id": uninstall_id,
        "plugin_id": id,
        "plugin_version": version,
        "plugin_name": name,
        "files_present": files_present,
        "files_missing": files_missing,
    });

    let pending = PendingUninstall {
        plugin_id: id.to_string(),
        plugin_version: version.to_string(),
        plugin_name: name.to_string(),
        files_present,
        files_missing,
        created_at: chrono::Utc::now(),
    };

    sweep_stale_pending_uninstalls(pending_uninstalls);
    pending_uninstalls
        .lock()
        .expect("pending_uninstalls mutex poisoned")
        .insert(uninstall_id, pending);

    format!("{PLUGIN_UNINSTALL_REQUEST_PREFIX}{}", preview)
}

fn sweep_stale_pending_uninstalls(map: &std::sync::Arc<PendingUninstallsMap>) {
    sweep_stale(map, PENDING_UNINSTALL_TTL_SECS);
}

/// Allowlist guard: a recorded path must live under `data/<content-dir>/`
/// where content-dir is one of `apps|knowhow|triggers|scripts|auth-modules`.
/// Defense-in-depth — called at both prepare and confirm time so a tampered
/// install record can never trick `remove_file` into escaping `data/`.
fn is_safe_data_path(data_relative: &str) -> bool {
    if data_relative.is_empty() || crate::api::is_path_traversal(data_relative) {
        return false;
    }
    let first = data_relative.split(['/', '\\']).next().unwrap_or("");
    crate::core::plugins::CONTENT_DIRS.contains(&first)
}

/// Walk parents of a deleted file up to (but NOT including) the `data/<first
/// segment>` content-dir root, removing any directory that's empty. Stops
/// at the content-dir root so `data/apps/` itself is never deleted even
/// when it's empty (the install layout assumes those exist).
fn prune_empty_parents(data_dir: &Path, data_relative: &str) {
    let path = data_dir.join(data_relative);
    let Some(mut parent) = path.parent().map(|p| p.to_path_buf()) else {
        return;
    };
    let floor = match data_relative.split(['/', '\\']).next() {
        Some(seg) if !seg.is_empty() => data_dir.join(seg),
        _ => return,
    };
    while parent != floor && parent.starts_with(&floor) {
        // Empty check: read_dir returns an iterator; if next() is Some, the
        // dir has at least one entry. Bail out cleanly on any read error
        // (dir gone, permissions) — partial cleanup is fine for uninstall.
        let mut entries = match std::fs::read_dir(&parent) {
            Ok(e) => e,
            Err(_) => return,
        };
        if entries.next().is_some() {
            return;
        }
        if std::fs::remove_dir(&parent).is_err() {
            return;
        }
        match parent.parent() {
            Some(p) => parent = p.to_path_buf(),
            None => return,
        }
    }
}

/// Outcome of a confirmed uninstall: human-readable summary + the file
/// partitions for the HTTP response (frontend uses `files_deleted` to
/// render the "Removed N files" toast).
pub struct ConfirmedUninstall {
    pub summary: String,
    pub files_deleted: Vec<String>,
    pub files_missing: Vec<String>,
}

/// Pop the pending uninstall, delete each `files_present` entry from `data/`,
/// prune empty parent directories, reload the WASM signer map if any
/// `auth-modules/` files were touched, and emit `PluginUninstalled` (extended
/// payload). The `actor` is the device/user who clicked Confirm; it stamps
/// the resulting event so the popover renders a real device label. Mirrors
/// `confirm_pending_install` step-for-step.
pub async fn confirm_pending_uninstall(
    engine: &LucidosEngine,
    uninstall_id: &str,
    actor: Option<MessageOrigin>,
) -> Result<ConfirmedUninstall, String> {
    let pending = {
        sweep_stale_pending_uninstalls(&engine.pending_uninstalls);
        engine
            .pending_uninstalls
            .lock()
            .expect("pending_uninstalls mutex poisoned")
            .remove(uninstall_id)
            .ok_or_else(|| format!("no pending uninstall with id '{}'", uninstall_id))?
    };

    let outcome = uninstall_with_bus(&engine.workspace_path, &engine.event_bus, &pending, actor)
        .await?;

    let auth_prefix = format!("{}/", AUTH_MODULES_DIR);
    if outcome
        .files_deleted
        .iter()
        .any(|p| p.starts_with(&auth_prefix))
    {
        if let Err(e) =
            crate::api::proxy::reload_proxy_modules_into(engine, &engine.workspace_path).await
        {
            log!(
                "[Plugins] auto-reload after uninstall failed (uninstall still succeeded): {}",
                e
            );
        }
    }

    Ok(outcome)
}

/// Drop the pending uninstall and emit `PluginUninstallCanceled` for audit.
/// `actor` is the device/user who clicked Cancel.
pub async fn cancel_pending_uninstall(
    engine: &LucidosEngine,
    uninstall_id: &str,
    actor: Option<MessageOrigin>,
) -> Result<(), String> {
    cancel_pending_uninstall_with_bus(
        &engine.pending_uninstalls,
        &engine.event_bus,
        uninstall_id,
        actor,
    )
    .await
}

pub(crate) async fn cancel_pending_uninstall_with_bus(
    pending_uninstalls: &std::sync::Arc<PendingUninstallsMap>,
    bus: &dyn EventBusEmitter,
    uninstall_id: &str,
    actor: Option<MessageOrigin>,
) -> Result<(), String> {
    sweep_stale_pending_uninstalls(pending_uninstalls);
    let pending = pending_uninstalls
        .lock()
        .expect("pending_uninstalls mutex poisoned")
        .remove(uninstall_id)
        .ok_or_else(|| format!("no pending uninstall with id '{}'", uninstall_id))?;

    bus.emit(BusEvent::System(SystemEvent::PluginUninstallCanceled {
        id: pending.plugin_id,
        version: pending.plugin_version,
        actor,
    }))
    .await
    .map_err(|e| format!("emit PluginUninstallCanceled: {}", e))?;

    Ok(())
}

/// Pure delete-and-emit. Tests inject a `MockEventBus` to assert the event
/// payload without standing up the engine.
pub(crate) async fn uninstall_with_bus(
    workspace_path: &Path,
    bus: &dyn EventBusEmitter,
    pending: &PendingUninstall,
    actor: Option<MessageOrigin>,
) -> Result<ConfirmedUninstall, String> {
    let data_dir = workspace_path.join(DATA_DIR);

    let mut files_deleted = Vec::with_capacity(pending.files_present.len());
    let mut files_missing_now: Vec<String> = pending.files_missing.clone();
    for rel in &pending.files_present {
        if !is_safe_data_path(rel) {
            // Already filtered at prepare_uninstall_request; double-check
            // here so a future caller can't bypass the safety net.
            files_missing_now.push(rel.clone());
            continue;
        }
        let abs = data_dir.join(rel);
        match std::fs::remove_file(&abs) {
            Ok(()) => {
                files_deleted.push(rel.clone());
                prune_empty_parents(&data_dir, rel);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Raced with a manual delete between prepare and confirm.
                files_missing_now.push(rel.clone());
            }
            Err(e) => {
                return Err(format!("delete {}: {}", rel, e));
            }
        }
    }

    let mut all_files = pending.files_present.clone();
    for f in &pending.files_missing {
        if !all_files.contains(f) {
            all_files.push(f.clone());
        }
    }

    bus.emit(BusEvent::System(SystemEvent::PluginUninstalled {
        id: pending.plugin_id.clone(),
        version: pending.plugin_version.clone(),
        files: all_files,
        files_deleted: files_deleted.clone(),
        files_missing: files_missing_now.clone(),
        actor,
    }))
    .await
    .map_err(|e| format!("emit PluginUninstalled: {}", e))?;

    let mut summary = format!(
        "Uninstalled {} v{}",
        pending.plugin_name, pending.plugin_version
    );
    match (files_deleted.len(), files_missing_now.len()) {
        (0, n) => summary.push_str(&format!(
            " — no files removed (all {n} recorded paths were already gone)."
        )),
        (d, 0) => summary.push_str(&format!(" ({d} files removed).")),
        (d, n) => summary.push_str(&format!(" ({d} files removed, {n} already gone).")),
    }

    Ok(ConfirmedUninstall {
        summary,
        files_deleted,
        files_missing: files_missing_now,
    })
}

/// Outcome of a confirmed install: install summary text plus the `data/`-
/// relative paths that were written. Confirm endpoints use the file list to
/// decide whether to trigger a `reload_proxy_modules` (any path under
/// `auth-modules/` means the WASM signer map must refresh).
pub struct ConfirmedInstall {
    pub summary: String,
    pub installed_files: Vec<String>,
}

/// Pop the pending install, run the actual write step from the staged tree,
/// and (if any `auth-modules/` files were written) auto-reload the proxy
/// WASM signer map so the new module is live without an engine restart.
/// Always uses `overwrite=true` because the user already saw the overwrite
/// list in the install panel — clicking Confirm IS the consent. The
/// `actor` is the device/user who clicked Confirm; it's stamped onto the
/// resulting `PluginInstalled` event so the popover shows a real device
/// label instead of `device-<short>`.
pub async fn confirm_pending_install(
    engine: &LucidosEngine,
    install_id: &str,
    actor: Option<MessageOrigin>,
) -> Result<ConfirmedInstall, String> {
    let pending = {
        sweep_stale_pending(&engine.pending_installs);
        engine
            .pending_installs
            .lock()
            .expect("pending_installs mutex poisoned")
            .remove(install_id)
            .ok_or_else(|| format!("no pending install with id '{}'", install_id))?
    };

    let (summary, installed_files) = install_from_unpacked_with_bus(
        &engine.workspace_path,
        &engine.event_bus,
        &pending.plugin_root,
        pending.source_type,
        true,
        actor,
    )
    .await?;

    let auth_prefix = format!("{}/", AUTH_MODULES_DIR);
    if installed_files.iter().any(|p| p.starts_with(&auth_prefix)) {
        if let Err(e) =
            crate::api::proxy::reload_proxy_modules_into(engine, &engine.workspace_path).await
        {
            log!(
                "[Plugins] auto-reload after install failed (install still succeeded): {}",
                e
            );
        }
    }

    Ok(ConfirmedInstall {
        summary,
        installed_files,
    })
}

/// Drop the pending install, emit a `PluginInstallCanceled` audit event.
/// `actor` is the device/user who clicked Cancel.
pub async fn cancel_pending_install(
    engine: &LucidosEngine,
    install_id: &str,
    actor: Option<MessageOrigin>,
) -> Result<(), String> {
    cancel_pending_install_with_bus(
        &engine.pending_installs,
        &engine.event_bus,
        install_id,
        actor,
    )
    .await
}

/// Bus-and-map version so tests can drive the cancel path without an engine.
/// `pending` falls out of scope at function end — the staged `TempDir`'s
/// `Drop` impl unlinks the directory at that point.
pub(crate) async fn cancel_pending_install_with_bus(
    pending_installs: &std::sync::Arc<PendingInstallsMap>,
    bus: &dyn EventBusEmitter,
    install_id: &str,
    actor: Option<MessageOrigin>,
) -> Result<(), String> {
    sweep_stale_pending(pending_installs);
    let pending = pending_installs
        .lock()
        .expect("pending_installs mutex poisoned")
        .remove(install_id)
        .ok_or_else(|| format!("no pending install with id '{}'", install_id))?;

    bus.emit(BusEvent::System(SystemEvent::PluginInstallCanceled {
        id: pending.plugin_id.clone(),
        version: pending.plugin_version.clone(),
        source: pending.source_string.clone(),
        source_type: pending.source_type.as_str().to_string(),
        actor,
    }))
    .await
    .map_err(|e| format!("emit PluginInstallCanceled: {}", e))?;

    Ok(())
}

/// Install a plugin from an already-unpacked directory. Pure orchestration —
/// takes the workspace path and an event bus, so tests can inject a mock.
/// Returns the install summary text plus the `data/`-relative paths that
/// were written; the confirm endpoint uses the file list verbatim (no
/// re-walk) for the auto-reload decision and the HTTP response.
pub(crate) async fn install_from_unpacked_with_bus(
    workspace_path: &Path,
    bus: &dyn EventBusEmitter,
    plugin_root: &Path,
    source_type: SourceType,
    overwrite: bool,
    actor: Option<MessageOrigin>,
) -> Result<(String, Vec<String>), String> {
    let (manifest, planned) = validate_tree(plugin_root).map_err(|e| e.to_string())?;
    let data_dir = workspace_path.join(DATA_DIR);

    let conflicts = detect_conflicts(&planned, &data_dir);
    if !conflicts.is_empty() && !overwrite {
        return Err(format!(
            "would overwrite {} files: [{}]. Re-run with overwrite=true to proceed.",
            conflicts.len(),
            conflicts.join(", ")
        ));
    }

    // Conflict scan ran against the same data_dir snapshot, so a partial
    // write here means a real disk failure (out-of-space, permissions).
    let mut installed_files: Vec<String> = Vec::with_capacity(planned.len());
    for PlannedFile {
        source,
        data_relative,
    } in &planned
    {
        let dst = data_dir.join(data_relative);
        copy_atomic(source, &dst).map_err(|e| {
            format!(
                "extract failed at {} (some files may have already been written): {}",
                data_relative, e
            )
        })?;
        installed_files.push(data_relative.clone());
    }

    let installed_at = chrono::Utc::now().to_rfc3339();
    let from = manifest
        .source
        .as_deref()
        .map(|s| format!(" from {}", short_source(s)))
        .unwrap_or_default();
    let summary = format!("Installed {} v{}{}", manifest.name, manifest.version, from);

    let mut payload = serde_json::Map::new();
    payload.insert("summary".into(), serde_json::json!(summary));
    payload.insert("manifest".into(), manifest.raw.clone());
    payload.insert("files".into(), serde_json::json!(installed_files));
    payload.insert("installed_at".into(), serde_json::json!(installed_at));
    payload.insert("source_type".into(), serde_json::json!(source_type.as_str()));

    bus.emit(BusEvent::System(SystemEvent::PluginInstalled {
        manifest: serde_json::Value::Object(payload),
        files: installed_files.clone(),
        installed_at,
        source_type: source_type.as_str().to_string(),
        actor,
    }))
    .await
    .map_err(|e| format!("event emit failed: {}", e))?;

    let mut result = format!(
        "Installed {} v{} ({} files).",
        manifest.name,
        manifest.version,
        installed_files.len()
    );
    if let Some(setup) = manifest
        .setup
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        result.push_str("\n\nSetup:\n");
        result.push_str(setup);
    }
    Ok((result, installed_files))
}

fn short_source(source: &str) -> String {
    source
        .strip_prefix("https://")
        .or_else(|| source.strip_prefix("http://"))
        .unwrap_or(source)
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string()
}

#[cfg(test)]
#[path = "../plugins_tests/helpers.rs"]
mod helpers;

#[cfg(test)]
#[path = "../plugins_tests/source_detection.rs"]
mod source_detection_tests;

#[cfg(test)]
#[path = "../plugins_tests/install.rs"]
mod install_tests;

#[cfg(test)]
#[path = "../plugins_tests/uninstall.rs"]
mod uninstall_tests;

#[cfg(test)]
#[path = "../plugins_tests/query.rs"]
mod query_tests;
