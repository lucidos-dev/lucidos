//! Plugin LLM tools: install / marketplace registration / update / uninstall flows.
//!
//! This module owns the user-confirm staging flows (install + uninstall),
//! the `LucidosEngine` tool dispatch, and the shared pending-map plumbing.
//! Three helper concerns live in child modules:
//!
//! - [`source`] — install-source detection + fetching.
//! - [`registry`] — installed-plugin projection / query / update-check.
//! - [`merge`]: three-way merge of an update against the user's local edits.
//! - [`marketplaces`]: the single write path for the marketplace registry,
//!   shared with `api::plugins` and announcing every mutation.
//!
//! Splitting is purely structural; the public surface stays reachable at
//! `crate::engine::tools::plugins::*` via the re-exports below.

use std::path::{Path, PathBuf};

use crate::core::git_auth::GitCredentials;
use crate::core::plugins::{
    compare_versions, detect_conflicts, validate_tree, PlannedFile, UpdateDecision,
    AUTH_MODULES_DIR,
};
use crate::core::DATA_DIR;
use crate::engine::event_bus::{BusEvent, EventBusEmitter, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::tools::agent_tool_actor;
use crate::engine::trigger_writes::TriggerWrite;
use crate::engine::LucidosEngine;

pub(crate) mod marketplaces;
mod merge;
mod registry;
mod source;

use merge::{LocalChangeOutcome, MergePlan, ResolvedChange};
use registry::{
    check_plugin_updates_impl, fetch_remote_manifest, latest_install, not_installed_error,
    project_baselines, resolve_plugin_query, PluginBaseline,
};
use source::{
    copy_atomic, credentials_for_source, detect_source, fetch_source, git_file_mode, write_atomic,
    write_atomic_like, SourceType,
};

/// Every installed plugin's merge baseline, keyed by plugin id. Resolved before
/// the blocking fetch and handed into it, since the staged plugin's id is only
/// known once its manifest parses inside that task.
type PluginBaselines = std::collections::BTreeMap<String, PluginBaseline>;

// Named by other modules via the `plugins::` path (`engine::tools::files`
// routes plugin-owned deletes here), so it stays a re-export.
pub(crate) use registry::find_plugin_owning_file;
// The `delete_app` HTTP handler (`api::apps`) routes plugin-owned app deletes
// to the uninstall panel via this — the app-level mirror of the file guard.
pub(crate) use registry::find_plugin_owning_app;
pub(crate) use registry::installed_plugin_summaries;

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
    pub(crate) plugin_name: String,
    /// The manifest's `setup` field, trimmed and normalized to `None` when
    /// empty. On confirm, a non-`None` value spawns a Lucidos Agent thread to
    /// walk the user through the author's setup instructions.
    pub(crate) setup: Option<String>,
    /// What this install would do to files the user has locally edited, decided
    /// at staging time so the confirm panel can state it per file. Empty for a
    /// fresh install and for an untouched plugin.
    pub(crate) merge: MergePlan,
    pub(crate) created_at: chrono::DateTime<chrono::Utc>,
}

impl LucidosEngine {
    /// Dispatch a plugin tool by name. Returns the result string verbatim to
    /// the LLM (success, sentinel, or "Error: ..." line).
    pub(crate) async fn execute_plugin_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> super::ToolOutcome {
        match name {
            crate::llm::tool_names::INSTALL_PLUGIN => {
                let source_str = match args.get("source").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Err("Error: source is required".to_string()),
                };
                super::lift_legacy_string(stage_install_request(self, &source_str).await)
            }
            crate::llm::tool_names::REGISTER_PLUGIN_MARKETPLACE => {
                self.register_plugin_marketplace_tool(args, thread_id).await
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
                    Ok(None) => return Err(not_installed_error(&id)),
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
                let remote =
                    match fetch_remote_manifest(&self.workspace_path, &self.pool, &source).await {
                        Ok(m) => m,
                        Err(e) => return Err(format!("Error: fetch latest manifest: {}", e)),
                    };
                if compare_versions(&installed_version, &remote.version)
                    == UpdateDecision::AlreadyLatest
                {
                    return Ok(format!("Already at latest (v{})", installed_version));
                }
                super::lift_legacy_string(stage_install_request(self, &source).await)
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

    async fn register_plugin_marketplace_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> super::ToolOutcome {
        let source = match args.get("source").and_then(|v| v.as_str()).map(str::trim) {
            Some(s) if !s.is_empty() => s,
            _ => return Err("Error: source is required".to_string()),
        };
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let registration = self
            .register_plugin_marketplace(source, name, Some(agent_tool_actor(thread_id)))
            .await
            .map_err(|e| format!("Error: {e}"))?;

        let response = serde_json::json!({
            "marketplace": registration.marketplace,
            "marketplaces": registration.marketplaces,
            "created": registration.created,
            "commit": registration.commit,
        });

        serde_json::to_string_pretty(&response)
            .map_err(|e| format!("Error: serialize marketplace registration: {e}"))
    }
}

type PendingInstallsMap = std::sync::Mutex<std::collections::HashMap<String, PendingInstall>>;

/// Stage `source_str` into a temp dir, validate the manifest + tree, register
/// the result in `pending_installs`, and return the
/// `[PLUGIN_INSTALL_REQUEST]` sentinel so the agentic loop (or the Plugins
/// panel's HTTP handler) surfaces the confirm panel. Runs the sync fetch (git
/// clone or zip extract) on the blocking pool so the caller's tokio worker
/// stays free.
///
/// Both entry points share this one body: the `install_plugin` / `update_plugin`
/// LLM tools above and `api::plugins`'s panel route. They used to be two
/// byte-identical copies (a `LucidosEngine` method and this free function),
/// which is one place for a fix to land in and another to be forgotten.
pub(crate) async fn stage_install_request(engine: &LucidosEngine, source_str: &str) -> String {
    let workspace = engine.workspace_path.clone();
    let pending = engine.pending_installs.clone();
    let source = source_str.to_string();
    // Read every plugin's merge baseline up front. The staged plugin's id
    // appears only when the fetched manifest parses, inside the blocking task.
    // So it cannot look up its own baseline from in there. A read failure
    // degrades to no baselines, which is today's plain overwrite.
    let baselines = match project_baselines(&engine.pool).await {
        Ok(b) => b,
        Err(e) => {
            log!(@Plugins, "read merge baselines failed (updates will overwrite local edits): {}", e);
            PluginBaselines::new()
        }
    };
    // The staging body is synchronous, so its credentials are resolved here.
    let credentials = credentials_for_source(&engine.pool, source_str).await;
    match tokio::task::spawn_blocking(move || {
        prepare_install_request(&workspace, &pending, &source, &baselines, &credentials)
    })
    .await
    {
        Ok(s) => s,
        Err(e) => format!("Error: install staging task panicked: {}", e),
    }
}

/// Stage an uninstall from the Plugins panel (the uninstall counterpart of
/// [`stage_install_request`]). Resolves `id`, builds a `PendingUninstall`, and
/// returns the `[PLUGIN_UNINSTALL_REQUEST]` sentinel for the HTTP handler to
/// unwrap — the same staging the `uninstall_plugin` LLM tool produces, so both
/// paths land in the identical confirm panel.
pub(crate) async fn stage_uninstall_request(engine: &LucidosEngine, id: &str) -> String {
    prepare_uninstall_plugin(
        &engine.workspace_path,
        &engine.pool,
        &engine.pending_uninstalls,
        id,
    )
    .await
}

/// Normalize a plugin manifest's `setup` instructions: trimmed, with an empty
/// (or whitespace-only) value collapsed to `None`. Used at staging time (panel
/// preview + pending entry) AND at confirm time (comparing the staged setup
/// against the installed version's), so both sides compare on equal terms.
pub(crate) fn normalize_setup(setup: Option<&str>) -> Option<String> {
    setup
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Decide whether confirming an install/update should spawn a setup thread.
/// `new` is the staged version's `setup`; `prior` is the currently-installed
/// version's `setup` (`None` on a fresh install). Both are normalized here, so
/// callers may pass raw or pre-normalized text. Setup runs only when the new
/// version has setup work the installed version didn't already cover:
/// - no `setup` in the new version → never.
/// - fresh install (or the installed version had no setup) with a `setup` → yes.
/// - update whose `setup` differs from the installed version's → yes.
/// - update whose `setup` is identical (after normalization) → no (re-running
///   identical setup on every version bump is noise — the user refinement).
pub(crate) fn setup_is_new(new: Option<&str>, prior: Option<&str>) -> bool {
    match (normalize_setup(new), normalize_setup(prior)) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(n), Some(p)) => n != p,
    }
}

/// Stage `source_str` into a temp dir, validate the manifest + tree, register
/// the result in `pending_installs`, and return the
/// `[PLUGIN_INSTALL_REQUEST]<json>` sentinel. On any failure returns
/// `"Error: ..."` and no entry is registered. Free function so tests can
/// drive it without standing up a `LucidosEngine`.
pub(crate) fn prepare_install_request(
    workspace_path: &Path,
    pending_installs: &std::sync::Arc<PendingInstallsMap>,
    source_str: &str,
    baselines: &PluginBaselines,
    credentials: &GitCredentials,
) -> String {
    let source = match detect_source(source_str) {
        Ok(s) => s,
        Err(e) => return format!("Error: {}", e),
    };
    let (scratch, plugin_root, source_type) =
        match fetch_source(workspace_path, &source, credentials) {
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

    // Normalize the author's setup instructions once: trimmed, with empty
    // collapsed to `None`. The same value feeds the panel preview AND the
    // pending entry that decides whether confirm spawns a setup thread.
    let setup = normalize_setup(manifest.setup.as_deref());

    // An update over a plugin the user has edited: work out per file whether
    // their edit merges, conflicts, or is simply lost, so the panel can say so
    // before they confirm. A fresh install has no baseline and no plan.
    let merge = match baselines.get(&manifest.id) {
        Some(baseline) => merge::plan_local_changes(workspace_path, baseline, &planned),
        None => MergePlan::default(),
    };

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
        "local_changes": merge.preview_json(),
        "setup": setup,
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
        plugin_name: manifest.name.clone(),
        setup,
        merge,
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
            return not_installed_error(&id);
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

    let outcome = {
        // Held across the delete+commit so the dirty check in
        // `change_ops::apply_change` (which also takes this lock) never observes
        // a half-written repo, and so this commit can't race a concurrent
        // write_file/edit_file commit. Mirrors the install confirm flow.
        let _repo_guard = engine.lock_workspace_repo().await;
        uninstall_with_bus(
            &engine.workspace_path,
            &engine.event_bus,
            &pending,
            actor.clone(),
        )
        .await?
    };

    // Auto-delete the plugin's auto-registered triggers (ADR 0019) — scoped by
    // plugin_id, so user-created triggers are untouched. Emitted after the
    // PluginUninstalled so the scheduler unregisters them + drops their
    // trigger.toml projection.
    delete_plugin_triggers(engine, &pending.plugin_id, actor).await;

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

    // Commit the deletions into the workspace git repo BEFORE emitting
    // PluginUninstalled — the symmetric counterpart of the install commit, so a
    // plugin's removal is version-controlled the same way write_file/edit_file
    // deletions are. `Ok(None)` (nothing was tracked) is fine: a plugin
    // installed before the install-commit fix had untracked files, so there's
    // nothing to stage. The confirm flow holds `lock_workspace_repo` around
    // this call.
    if !files_deleted.is_empty() {
        crate::core::commit_data_paths_removed(
            workspace_path,
            &files_deleted,
            &format!(
                "Uninstall plugin: {} v{}",
                pending.plugin_id, pending.plugin_version
            ),
        )
        .map_err(|e| format!("commit uninstalled plugin files: {}", e))?;
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

/// What a confirmed install does with the user's local edits: the merged bytes
/// to put on disk, the saved-aside copies to commit, and the per-outcome lists
/// to report. Default (every list empty) is a fresh install, or an update over
/// a plugin nobody has edited, and behaves exactly as the flow always has.
#[derive(Default)]
pub(crate) struct LocalChangeWrites {
    /// Merged content replacing upstream's on disk. The install commit still
    /// records upstream's bytes for these paths, so the next update's merge
    /// base stays pristine.
    merged: Vec<(String, Vec<u8>)>,
    /// `data/`-relative copies of discarded edits, written before the
    /// overwrite and committed with the merge.
    saved_aside: Vec<String>,
    conflicted: Vec<String>,
    replaced: Vec<String>,
    /// Files the user had deleted, which upstream still ships and so come
    /// back. Kept apart from `replaced`: there is no saved copy for these, and
    /// lumping them in would make `saved_aside` look short by that many.
    restored: Vec<String>,
}

impl LocalChangeWrites {
    fn from_resolved(resolved: &[ResolvedChange<'_>], saved_aside: Vec<String>) -> Self {
        let mut writes = Self {
            saved_aside,
            ..Default::default()
        };
        for change in resolved {
            let path = change.data_relative().to_string();
            match change.outcome {
                LocalChangeOutcome::Merged => match change.merged_bytes() {
                    Some(bytes) => writes.merged.push((path, bytes.to_vec())),
                    // Unreachable: a `Merged` outcome always carries its bytes.
                    // Degrading to `replaced` keeps the report honest rather
                    // than claiming a merge that never happened.
                    None => writes.replaced.push(path),
                },
                LocalChangeOutcome::Conflict => writes.conflicted.push(path),
                LocalChangeOutcome::Replaced => writes.replaced.push(path),
                LocalChangeOutcome::Restored => writes.restored.push(path),
            }
        }
        writes
    }

    fn is_empty(&self) -> bool {
        self.merged.is_empty()
            && self.conflicted.is_empty()
            && self.replaced.is_empty()
            && self.restored.is_empty()
    }

    fn merged_paths(&self) -> Vec<String> {
        self.merged.iter().map(|(p, _)| p.clone()).collect()
    }
}

/// What an update did to the user's local edits, for the confirm response and
/// the `PluginLocalChangesMerged` event. `None` when the install met none.
pub struct LocalChangeReport {
    pub merged: Vec<String>,
    pub conflicted: Vec<String>,
    pub replaced: Vec<String>,
    /// Files the user had deleted that upstream still ships, so they returned.
    /// Nothing is saved aside for these: a deletion has no content to keep.
    pub restored: Vec<String>,
    /// `data/`-relative copies of every edit the update discarded.
    pub saved_paths: Vec<String>,
    /// The commit holding the merged working tree plus those copies.
    pub commit: Option<String>,
}

/// Outcome of a confirmed install: install summary text plus the `data/`-
/// relative paths that were written. Confirm endpoints use the file list to
/// decide whether to trigger a `reload_proxy_modules` (any path under
/// `auth-modules/` means the WASM signer map must refresh).
pub struct ConfirmedInstall {
    pub summary: String,
    pub installed_files: Vec<String>,
    /// Set when the install met local edits to the files it ships.
    pub local_changes: Option<LocalChangeReport>,
    /// The Lucidos Agent thread spawned to walk the user through the plugin's
    /// `setup` instructions, when the manifest carried any. `None` for plugins
    /// with no setup field. The frontend navigates the user to this thread so
    /// setup happens in front of them instead of silently in the background.
    pub setup_thread_id: Option<uuid::Uuid>,
}

/// Write one trigger lifecycle event, logging (not propagating) an emit error:
/// trigger sync is a best-effort side effect of install/uninstall, never a
/// reason to fail the whole operation.
///
/// Through the trigger write chokepoint, so a resync leaves the registry
/// consistent before it moves on: the loop below diffs against
/// `engine.trigger_configs`, and the uninstall path reads it too.
async fn write_plugin_trigger(
    engine: &LucidosEngine,
    write: TriggerWrite,
    trigger_id: &str,
    payload: serde_json::Value,
    actor: Option<MessageOrigin>,
    reason: &str,
) {
    engine
        .emit_trigger_write_or_log(
            write,
            trigger_id,
            payload,
            actor,
            &format!("[Plugins] ({reason})"),
        )
        .await;
}

/// Re-sync a plugin's auto-registered triggers to its shipped `trigger.toml`
/// set (ADR 0019). Matched by `(plugin_id, slug)`:
/// - a declared slug that already exists → `TriggerUpdated` (re-stamps the
///   definition; omits `paused` so a user's pause survives);
/// - a new declared slug → `TriggerCreated` (event-driven, live immediately);
/// - an existing plugin trigger whose slug is no longer declared → `TriggerDeleted`.
///
/// On a fresh install `existing` is empty (pure creates); on update it diffs.
/// The scheduler subscriber turns these into registrations + the on-disk
/// projection. Best-effort throughout.
async fn resync_plugin_triggers(
    engine: &LucidosEngine,
    plugin_id: &str,
    installed_files: &[String],
    actor: Option<MessageOrigin>,
) {
    let data_dir = engine.workspace_path.join(DATA_DIR);

    // Declared = the plugin's shipped trigger.toml files, parsed, with a stable
    // resolved slug (author slug, else derived from name).
    let mut resolved: Vec<(String, crate::triggers::definition::TriggerDefinition)> = Vec::new();
    for rel in installed_files {
        // The DIRECTORY segment (`triggers/<slug>/trigger.toml`) is authoritative
        // for a plugin trigger: it's the recorded install path, the per-trigger
        // knowhow dir, AND the projection key. Overriding any divergent body
        // `slug` keeps all three in lockstep — otherwise the projection would
        // write a second file under the body-slug dir and the boot rebuild would
        // later prune the plugin's recorded file as a false orphan.
        let Some(dir_slug) = plugin_trigger_slug(rel) else {
            continue;
        };
        let slug = dir_slug.to_string();
        let abs = data_dir.join(rel);
        let text = match std::fs::read_to_string(&abs) {
            Ok(t) => t,
            Err(e) => {
                log!("[Plugins] read trigger {} failed: {}", rel, e);
                continue;
            }
        };
        let mut def = match crate::triggers::definition::TriggerDefinition::from_toml(&text) {
            Ok(d) => d,
            Err(e) => {
                log!("[Plugins] skip unparseable trigger {}: {}", rel, e);
                continue;
            }
        };
        if !def.slug.is_empty() && def.slug != slug {
            log!(
                "[Plugins] trigger {} body slug '{}' differs from its directory '{}' — using the directory",
                rel,
                def.slug,
                slug
            );
        }
        def.slug = slug.clone();
        // Drop a second trigger.toml that resolved to the same slug within this
        // plugin (author error) — keeping it would mint two triggers writing one
        // shared data/triggers/<slug>/ projection.
        if resolved.iter().any(|(s, _)| s == &slug) {
            log!(
                "[Plugins] skip duplicate plugin trigger slug '{}' ({})",
                slug,
                rel
            );
            continue;
        }
        resolved.push((slug, def));
    }

    // One read pass: this plugin's currently-registered triggers (id + slug) for
    // the diff, AND the slugs owned by ANY OTHER trigger (user or another plugin)
    // so we never auto-register onto a slug that's already taken — that would
    // clobber a shared data/triggers/<slug>/ projection + per-trigger knowhow dir.
    let (existing, foreign_slugs): (Vec<(String, String)>, std::collections::HashSet<String>) = {
        let configs = engine.trigger_configs.read().unwrap();
        let mut existing = Vec::new();
        let mut foreign = std::collections::HashSet::new();
        for c in configs.values() {
            if c.plugin_id.as_deref() == Some(plugin_id) {
                existing.push((c.id.clone(), c.slug.clone()));
            } else {
                foreign.insert(c.slug.clone());
            }
        }
        (existing, foreign)
    };
    let declared_slugs: std::collections::HashSet<&str> =
        resolved.iter().map(|(s, _)| s.as_str()).collect();

    for (slug, def) in &resolved {
        if let Some((existing_id, _)) = existing.iter().find(|(_, s)| s == slug) {
            let payload = def.to_trigger_payload(existing_id, plugin_id);
            write_plugin_trigger(
                engine,
                TriggerWrite::Updated,
                existing_id,
                payload,
                actor.clone(),
                "plugin resync",
            )
            .await;
        } else if foreign_slugs.contains(slug) {
            // Slug already belongs to a user trigger or another plugin — refuse
            // to hijack it. The plugin's other triggers still register.
            log!(
                "[Plugins] skip plugin '{}' trigger '{}' — slug already in use by another trigger",
                plugin_id,
                slug
            );
            continue;
        } else {
            let new_id = uuid::Uuid::new_v4().to_string();
            let payload = def.to_trigger_payload(&new_id, plugin_id);
            write_plugin_trigger(
                engine,
                TriggerWrite::Created,
                &new_id,
                payload,
                actor.clone(),
                "plugin auto-register",
            )
            .await;
        }
    }
    for (existing_id, slug) in &existing {
        if !declared_slugs.contains(slug.as_str()) {
            write_plugin_trigger(
                engine,
                TriggerWrite::Deleted,
                existing_id,
                serde_json::json!({ "trigger_id": existing_id }),
                actor.clone(),
                "plugin resync",
            )
            .await;
        }
    }
}

/// Delete every trigger a plugin auto-registered (ADR 0019) — emitted on
/// uninstall so the plugin's triggers don't outlive it. Scoped strictly by
/// `plugin_id`, so user-created triggers (`plugin_id == None`) are never touched.
async fn delete_plugin_triggers(
    engine: &LucidosEngine,
    plugin_id: &str,
    actor: Option<MessageOrigin>,
) {
    let owned: Vec<String> = {
        let configs = engine.trigger_configs.read().unwrap();
        configs
            .values()
            .filter(|c| c.plugin_id.as_deref() == Some(plugin_id))
            .map(|c| c.id.clone())
            .collect()
    };
    for id in owned {
        write_plugin_trigger(
            engine,
            TriggerWrite::Deleted,
            &id,
            serde_json::json!({ "trigger_id": id }),
            actor.clone(),
            "plugin uninstall",
        )
        .await;
    }
}

/// Pop the pending install, run the actual write step from the staged tree,
/// and (if any `auth-modules/` files were written) auto-reload the proxy
/// WASM signer map so the new module is live without an engine restart.
/// Always uses `overwrite=true` because the user already saw the overwrite
/// list in the install panel: clicking Confirm IS the consent. The
/// `actor` is the device/user who clicked Confirm; it's stamped onto the
/// resulting `PluginInstalled` event so the popover shows a real device
/// label instead of `device-<short>`.
pub async fn confirm_pending_install(
    engine: &LucidosEngine,
    install_id: &str,
    keep_local_changes: bool,
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

    // Pre-allocate the setup thread id only when this confirm has *new* setup
    // work to run: a fresh install with a `setup` field, or an update whose
    // `setup` instructions differ from the currently-installed version's. A
    // version bump that left `setup` unchanged re-runs nothing (no thread, no
    // navigation) — re-doing identical setup on every update is noise. The
    // recorded id is what lets the Plugins panel card resolve the right thread for
    // its Setup→Open state after a reload.
    //
    // `latest_install` here still sees the *previous* version — the new
    // `PluginInstalled` event isn't emitted until `install_from_unpacked_with_bus`
    // below. A read error is non-fatal: fall back to treating the staged setup
    // as new (spawn) so a transient DB hiccup never silently skips real setup.
    // One lookup answers two questions: whether the setup text is new, and
    // whether this is a re-run. A record at all is what makes it a re-run, and
    // the setup thread's seed says so. A failed read costs both answers, so an
    // update then seeds the first-install line. That is the same direction as
    // the fallback above: run the setup in full rather than half of it.
    let prior = match latest_install(&engine.pool, &pending.plugin_id).await {
        Ok(rec) => rec,
        Err(e) => {
            log!(
                "[Plugins] prior-install lookup failed for '{}' (treating setup as new): {}",
                pending.plugin_id,
                e
            );
            None
        }
    };
    let prior_setup = prior.as_ref().and_then(|r| r.setup()).map(str::to_string);
    let occasion = match &prior {
        Some(rec) => SetupOccasion::Update {
            from: rec.version().map(str::to_string),
        },
        None => SetupOccasion::FreshInstall,
    };
    let setup_thread_id =
        setup_is_new(pending.setup.as_deref(), prior_setup.as_deref()).then(uuid::Uuid::new_v4);

    let (summary, installed_files, local_changes) = {
        let _repo_guard = engine.lock_workspace_repo().await;

        // The panel stated an outcome per edited file. If one of those files
        // moved since, the staged plan describes content that no longer
        // exists, and writing it would clobber the newer edit unseen.
        if let Some(path) = pending.merge.detect_drift(&engine.workspace_path) {
            return Err(format!(
                "'{}' changed after you reviewed this update. Run the update \
                 again to see what it would do now.",
                path
            ));
        }
        let resolved = pending.merge.resolve(keep_local_changes);
        // Before a single byte is overwritten, so a failure past this point
        // cannot take the user's work with it.
        let saved_aside = merge::save_discarded(
            &engine.workspace_path,
            &pending.plugin_id,
            &pending.plugin_version,
            &resolved,
        )?;
        let writes = LocalChangeWrites::from_resolved(&resolved, saved_aside);
        // Built before the writer consumes `writes`; the commit sha is the one
        // field only the writer can supply, so it is filled in afterwards.
        let mut report = (!writes.is_empty()).then(|| LocalChangeReport {
            merged: writes.merged_paths(),
            conflicted: writes.conflicted.clone(),
            replaced: writes.replaced.clone(),
            restored: writes.restored.clone(),
            saved_paths: writes.saved_aside.clone(),
            commit: None,
        });

        let write = install_from_unpacked_with_bus(
            &engine.workspace_path,
            &engine.event_bus,
            &pending.plugin_root,
            InstallContext {
                actor: actor.clone(),
                setup_thread_id,
                local: writes,
                ..InstallContext::plain(pending.source_type, true)
            },
        )
        .await?;
        if let Some(report) = report.as_mut() {
            report.commit = write.local_changes_commit.clone();
        }
        (write.summary, write.installed_files, report)
    };

    announce_local_changes(engine, &pending, local_changes.as_ref(), actor.clone()).await;

    reload_auth_modules_if_needed(engine, &installed_files).await;

    // Auto-register the plugin's shipped event-driven triggers (ADR 0019),
    // stamped with the plugin id. On an update this re-syncs against the prior
    // version's set (add/update/remove by slug). They go live immediately; the
    // user already saw the trigger.toml files in the confirm panel's file list.
    resync_plugin_triggers(engine, &pending.plugin_id, &installed_files, actor.clone()).await;

    // A plugin author's `setup` field is "ask the user / wire this up" work —
    // inert until an agent runs it. Spawn a Lucidos Agent thread so setup
    // actually happens (the thread references the instructions via
    // `system-knowhow/plugin-setup`, see `build_setup_thread_request`); the id
    // flows back so the frontend can navigate the user straight to it.
    // `setup_thread_id` is minted only when `setup_is_new` said yes, and that
    // is already false for an absent or blank `setup`.
    if let Some(thread_id) = setup_thread_id {
        spawn_plugin_setup_thread(engine, thread_id, &pending.plugin_name, &occasion, actor).await;
    }

    Ok(ConfirmedInstall {
        summary,
        installed_files,
        local_changes,
        setup_thread_id,
    })
}

/// Announce what the update did to the user's local edits.
///
/// Two emits, because they answer different questions.
/// `PluginLocalChangesMerged` records the decision, which nothing on disk holds
/// once the commits exist. `ArtifactCreated` per saved copy is what makes those
/// copies show up in the Files list without a reload.
///
/// Best-effort: the plugin is installed and the copies are on disk by now, so a
/// failed emit is logged rather than failing the install.
async fn announce_local_changes(
    engine: &LucidosEngine,
    pending: &PendingInstall,
    report: Option<&LocalChangeReport>,
    actor: Option<MessageOrigin>,
) {
    let Some(report) = report else {
        return;
    };
    engine
        .event_bus
        .emit_or_log(
            BusEvent::System(SystemEvent::PluginLocalChangesMerged {
                id: pending.plugin_id.clone(),
                version: pending.plugin_version.clone(),
                merged: report.merged.clone(),
                conflicted: report.conflicted.clone(),
                replaced: report.replaced.clone(),
                restored: report.restored.clone(),
                saved_paths: report.saved_paths.clone(),
                commit: report.commit.clone(),
                actor: actor.clone(),
            }),
            "[Plugins] PluginLocalChangesMerged",
        )
        .await;

    let Some(commit) = report.commit.clone() else {
        return;
    };
    for path in &report.saved_paths {
        let Some(artifact_path) = path.strip_prefix("artifacts/") else {
            continue;
        };
        engine
            .event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ArtifactCreated {
                    artifact_path: artifact_path.to_string(),
                    commit: commit.clone(),
                    source: Some("plugin_update".to_string()),
                }),
                "[Plugins] ArtifactCreated",
            )
            .await;
    }
}

/// The result of offering a local patch upstream: where the patch was written,
/// and the thread that will take it from there.
pub struct ProposedUpstream {
    /// `data/`-relative path of the generated patch.
    pub patch_path: String,
    pub thread_id: uuid::Uuid,
}

/// Offer the user's local patch to the plugin's author.
///
/// The engine's whole job is to produce the patch and hand it to an agent. It
/// derives the diff from the install commit to the working tree, writes it as
/// an artifact, and spawns a thread. It knows nothing about GitHub: no
/// credentials, no fork, no API call. The procedure lives in
/// `system-knowhow/plugins.md`, which the spawned thread reads, mirroring how
/// `plugin-setup` works for install.
pub async fn propose_local_patch_upstream(
    engine: &LucidosEngine,
    query: &str,
    actor: Option<MessageOrigin>,
) -> Result<ProposedUpstream, String> {
    let id = resolve_plugin_query(&engine.pool, query).await?;
    let installed = match latest_install(&engine.pool, &id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => return Err(format!("plugin '{}' is not currently installed", id)),
        Err(e) => return Err(format!("read install record: {}", e)),
    };
    let commit = installed
        .commit()
        .ok_or_else(|| {
            format!(
                "plugin '{}' was installed before Lucidos recorded a baseline, so there is \
                 nothing to diff your changes against",
                id
            )
        })?
        .to_string();
    let version = installed.version().unwrap_or("unknown").to_string();
    let plugin_name = installed.name().unwrap_or(&id).to_string();
    let baseline = PluginBaseline {
        commit,
        files: installed.files(),
    };

    let patch = merge::local_patch(&engine.workspace_path, &baseline).ok_or_else(|| {
        format!(
            "'{}' has no local changes to propose (nothing differs from v{})",
            plugin_name, version
        )
    })?;

    let patch_path = format!(
        "{}/{}/proposed-v{}.patch",
        merge::SAVED_CHANGES_ROOT,
        id,
        version
    );
    let commit_sha = {
        let _repo_guard = engine.lock_workspace_repo().await;
        write_atomic(
            patch.as_bytes(),
            &engine.workspace_path.join(DATA_DIR).join(&patch_path),
        )?;
        crate::core::commit_data_paths_added(
            &engine.workspace_path,
            std::slice::from_ref(&patch_path),
            &format!("Patch to propose upstream: {} v{}", id, version),
        )
        .map_err(|e| format!("commit proposed patch: {}", e))?
    };

    if let Some(artifact_path) = patch_path.strip_prefix("artifacts/") {
        engine
            .event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::artifact_change(
                    false,
                    artifact_path.to_string(),
                    commit_sha,
                    Some("plugin_upstream_patch".to_string()),
                )),
                "[Plugins] ArtifactCreated",
            )
            .await;
    }

    let thread_id = uuid::Uuid::new_v4();
    engine
        .thread_queue
        .submit(
            build_upstream_patch_thread_request(thread_id, &plugin_name, &patch_path),
            actor,
            None,
        )
        .await;

    Ok(ProposedUpstream {
        patch_path,
        thread_id,
    })
}

/// The Thread Queue request for an upstream-patch thread. A `SubThread` for the
/// same two reasons as the setup thread. Its `prepare` step emits the seeding
/// `MessageReceived` eagerly, so the frontend can navigate into a real thread.
/// And it sidesteps the origin/mode validation an `AgentChat` would hit.
///
/// The seed is one user-facing line naming the plugin and the patch. How to
/// actually open the pull request lives in `system-knowhow/plugins.md`, which
/// keeps procedure out of the prompt.
fn build_upstream_patch_thread_request(
    thread_id: uuid::Uuid,
    plugin_name: &str,
    patch_path: &str,
) -> crate::engine::thread_queue::ThreadQueueRequest {
    crate::engine::thread_queue::ThreadQueueRequest::SubThread {
        // Stamped by `submit` from the submitting task's chain depth.
        depth: 0,
        prompt: format!(
            "Propose my local changes to the {plugin_name} plugin upstream. \
             The patch is at data/{patch_path}."
        ),
        child_thread_id: thread_id,
        parent_thread_id: None,
        spawning_event_id: None,
        title: Some(format!("Propose {plugin_name} patch upstream")),
        model: None,
        reasoning_effort: None,
        pre_emitted_origin: None,
        origin: None,
    }
}

/// Spawn a Lucidos Agent thread that walks the user through completing a
/// plugin's setup (asking questions, wiring config). The seed is a short
/// user-facing line; the "how to" lives in `system-knowhow/plugin-setup` and
/// the author's `setup` text is referenced from the `PluginInstalled` event
/// (see `build_setup_thread_request`). Submitted through the Thread Queue like
/// any background spawn, so it respects admission control. `thread_id` is
/// pre-allocated by the caller (so
/// the same id can be recorded in the `PluginInstalled` event). The submission
/// never fails for this kind (overflow is per-trigger only), so the thread is
/// always either admitted immediately or queued — never dropped.
///
/// Spawned as a [`ThreadQueueRequest::SubThread`], NOT `AgentChat`, for two
/// reasons:
/// 1. **Materialize before navigation.** The Thread Queue executor's `prepare`
///    step emits the seeding `MessageReceived` EAGERLY — synchronously, before
///    `submit` returns, on the immediate-admit path — so the `thread_summaries`
///    row exists by the time this confirm responds. The frontend then navigates
///    into a real, materializing thread instead of an empty view. (`AgentChat`
///    has no eager prepare, so its thread didn't exist until the agent later ran.)
/// 2. **No origin/mode panic.** `AgentChat` paired `origin: actor` — a
///    `MessageOrigin::Device` whose `mode()` is `Human` — with `mode: Agent`.
///    `make_message_received`'s origin/mode validation rejects that mismatch and
///    `.expect()`s, so the setup thread's `MessageReceived` panicked and the
///    thread never materialized. `SubThread` emits with a synthesized Agent-mode
///    origin (`None` here, since there's no parent), sidestepping the validation.
///
/// `actor` still attributes the `ThreadQueued` audit event via `submit`.
async fn spawn_plugin_setup_thread(
    engine: &LucidosEngine,
    thread_id: uuid::Uuid,
    plugin_name: &str,
    occasion: &SetupOccasion,
    actor: Option<MessageOrigin>,
) {
    let request = build_setup_thread_request(thread_id, plugin_name, occasion);
    engine.thread_queue.submit(request, actor, None).await;
}

/// Why a setup thread is being spawned, which is what its seed and title say.
///
/// A separate type rather than an `Option<version>`, because the version is
/// decoration and the occasion is the fact. A legacy `PluginInstalled` row can
/// name no version at all, and `installed_plugin_summaries` shows those as
/// `unknown`. Collapsing the two would seed such an update as a first install.
/// The agent would then skip the whole reuse step and re-ask everything, which
/// is the failure this occasion exists to prevent.
#[derive(Debug)]
enum SetupOccasion {
    FreshInstall,
    /// The plugin was already installed. `from` is the version it was on, when
    /// the prior record names one.
    Update {
        from: Option<String>,
    },
}

/// Build the Thread Queue request for a plugin setup thread. Pure (no engine,
/// no I/O) so the request shape is unit-testable — the `SubThread` choice and
/// the bound `child_thread_id` are load-bearing (see `spawn_plugin_setup_thread`
/// for why), and a regression back to `AgentChat` would silently break setup.
///
/// The seed is a SHORT, user-facing line — the only thing shown in the thread's
/// first bubble. The "how to run a plugin setup" meta-instructions live in
/// `system-knowhow/plugin-setup` (loaded by the agent, see the chat system
/// prompt's load-knowhow nudge), and the plugin author's own `setup` text is
/// referenced from the durable `PluginInstalled` event — neither is embedded
/// here, so the user isn't shown a wall of agent instructions.
///
/// An update says so in both the seed and the title. The agent then picks up
/// where the last run left off instead of starting over, and the thread list
/// stops showing the same name twice.
///
/// Two words are load-bearing across layers. Every seed keeps the verb `set
/// up`, which the system-prompt route and the knowhow's `description` key on.
/// Every update seed says `again`, which is how the knowhow tells the two
/// occasions apart.
fn build_setup_thread_request(
    thread_id: uuid::Uuid,
    plugin_name: &str,
    occasion: &SetupOccasion,
) -> crate::engine::thread_queue::ThreadQueueRequest {
    let (prompt, title) = match occasion {
        SetupOccasion::Update { from: Some(prior) } => (
            format!(
                "Set up {plugin_name} again: its setup instructions changed \
                 since version {prior}."
            ),
            format!("Update {plugin_name} setup"),
        ),
        SetupOccasion::Update { from: None } => (
            format!("Set up {plugin_name} again: this update changed its setup instructions."),
            format!("Update {plugin_name} setup"),
        ),
        SetupOccasion::FreshInstall => (
            format!("Set up the newly installed {plugin_name} plugin."),
            format!("Set up {plugin_name}"),
        ),
    };

    crate::engine::thread_queue::ThreadQueueRequest::SubThread {
        // Stamped by `submit` from the submitting task's chain depth.
        depth: 0,
        prompt,
        child_thread_id: thread_id,
        parent_thread_id: None,
        spawning_event_id: None,
        title: Some(title),
        model: None,
        reasoning_effort: None,
        pre_emitted_origin: None,
        // No launching THREAD to attribute to: an install is a user action, and
        // stamping the clicking device would be the `mode: Agent` mismatch this
        // function's doc comment describes. Distinct from a `relation: "top"`
        // spawn, which has no linkage but does have a spawning thread.
        origin: None,
    }
}

async fn reload_auth_modules_if_needed(engine: &LucidosEngine, installed_files: &[String]) {
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

/// The `<slug>` of a `data/`-relative path iff it is `triggers/<slug>/trigger.toml`.
fn plugin_trigger_slug(data_relative: &str) -> Option<&str> {
    let parts: Vec<&str> = data_relative.split('/').collect();
    match parts.as_slice() {
        ["triggers", slug, "trigger.toml"] if !slug.is_empty() => Some(slug),
        _ => None,
    }
}

/// Reject install if any shipped `trigger.toml` declares a cron `schedule` (or
/// fails to parse). Plugins ship event-driven triggers only (ADR 0019). Reads
/// from the staged `plugin_root` (via each `PlannedFile.source`), so it runs
/// before any file is copied.
fn validate_plugin_triggers_event_driven(planned: &[PlannedFile]) -> Result<(), String> {
    for pf in planned {
        let Some(slug) = plugin_trigger_slug(&pf.data_relative) else {
            continue;
        };
        let text = std::fs::read_to_string(&pf.source)
            .map_err(|e| format!("read trigger definition {}: {}", pf.data_relative, e))?;
        let def = crate::triggers::definition::TriggerDefinition::from_toml(&text)
            .map_err(|e| format!("invalid trigger '{}' ({}): {}", slug, pf.data_relative, e))?;
        if !def.schedule.is_empty() {
            return Err(format!(
                "plugin trigger '{}' declares a cron schedule; plugins may only ship \
                 event-driven (on:) triggers — cron triggers are workspace state. \
                 Remove `schedule` from {}.",
                slug, pf.data_relative
            ));
        }
    }
    Ok(())
}

/// Everything about one install write except the content itself: where it came
/// from, who asked for it, and what to do with the user's local edits.
pub(crate) struct InstallContext {
    pub(crate) source_type: SourceType,
    /// The user already saw the overwrite list in the panel, so a confirm
    /// passes `true`. A caller that has not shown one passes `false` and gets
    /// an error naming the paths instead.
    pub(crate) overwrite: bool,
    /// The device that clicked Confirm, stamped onto `PluginInstalled`.
    pub(crate) actor: Option<MessageOrigin>,
    /// The setup thread spawned for this install, recorded in the
    /// `PluginInstalled` payload so the Plugins panel card's Setup-to-Open
    /// state survives reloads. `None` when the plugin shipped no `setup`.
    pub(crate) setup_thread_id: Option<uuid::Uuid>,
    /// What to do with files the user has locally edited. Default (all empty)
    /// writes every path straight from the staged tree, as a fresh install and
    /// every pre-merge caller always did.
    pub(crate) local: LocalChangeWrites,
}

impl InstallContext {
    /// An install with no local edits to weigh, and nobody to attribute it to.
    pub(crate) fn plain(source_type: SourceType, overwrite: bool) -> Self {
        Self {
            source_type,
            overwrite,
            actor: None,
            setup_thread_id: None,
            local: LocalChangeWrites::default(),
        }
    }
}

/// What the writer did, for the confirm endpoint to report on.
#[derive(Debug)]
pub(crate) struct InstallWrite {
    pub(crate) summary: String,
    /// Every `data/`-relative path written. The confirm uses it verbatim (no
    /// re-walk) for the auto-reload decision and the HTTP response.
    pub(crate) installed_files: Vec<String>,
    /// The second commit, recording the merged working tree plus any
    /// saved-aside copies. `None` when the install met no local edits.
    pub(crate) local_changes_commit: Option<String>,
}

/// Install a plugin from an already-unpacked directory. Pure orchestration:
/// takes the workspace path and an event bus, so tests can inject a mock.
pub(crate) async fn install_from_unpacked_with_bus(
    workspace_path: &Path,
    bus: &dyn EventBusEmitter,
    plugin_root: &Path,
    ctx: InstallContext,
) -> Result<InstallWrite, String> {
    let InstallContext {
        source_type,
        overwrite,
        actor,
        setup_thread_id,
        local,
    } = ctx;
    let (manifest, planned) = validate_tree(plugin_root).map_err(|e| e.to_string())?;
    let data_dir = workspace_path.join(DATA_DIR);

    // Plugins may only ship event-driven (`on:`) triggers — reject a shipped
    // `trigger.toml` with a cron `schedule` BEFORE any file is written (clean
    // failure, nothing copied). Cron is workspace state, not plugin content
    // (ADR 0019, plugins.md "What doesn't belong in a plugin").
    validate_plugin_triggers_event_driven(&planned)?;

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
        let merged = local
            .merged
            .iter()
            .find(|(path, _)| path == data_relative)
            .map(|(_, bytes)| bytes);
        let write = match merged {
            // Mode taken from the shipped file, so a merged path ends up
            // exactly as the plain copy below would have left it.
            Some(bytes) => write_atomic_like(bytes, &dst, source),
            None => copy_atomic(source, &dst),
        };
        write.map_err(|e| {
            format!(
                "extract failed at {} (some files may have already been written): {}",
                data_relative, e
            )
        })?;
        installed_files.push(data_relative.clone());
    }

    // Commit the freshly-written files into the workspace git repo BEFORE
    // emitting PluginInstalled — every other data-writing path (write_file /
    // edit_file / data writes / marketplace registration) version-controls what
    // it writes, and install must too. Without this the plugin's files sit
    // untracked forever (no history, lost on a hard reset, invisible to
    // git-based backups). One commit covers all content dirs (apps/, knowhow/,
    // triggers/, scripts/, auth-modules/). The confirm flow holds
    // `lock_workspace_repo` around this call so it can't race other repo writes.
    // Capture the install commit sha — it is the baseline a later read diffs the
    // plugin's on-disk content against to decide whether the user has locally
    // modified it (the Plugins-list "Modified" badge). Recorded in the
    // `PluginInstalled` payload below; an update re-commits and re-stamps it, so
    // the badge resets. See `registry::plugin_modification_status`.
    //
    // A merged path is recorded from UPSTREAM's bytes, not from the working
    // tree. The install commit is then a byte-exact copy of the shipped
    // version, whatever the merge put on disk. That keeps the next update's
    // merge base honest. Record the merge here instead, and the following
    // update reads the user's patch as upstream's own content, finds no local
    // modification, and drops it.
    let upstream_bytes = |data_relative: &String| -> Result<(String, Vec<u8>, u32), String> {
        let pf = planned
            .iter()
            .find(|pf| &pf.data_relative == data_relative)
            .ok_or_else(|| format!("merged path {} is not in the install plan", data_relative))?;
        let bytes = std::fs::read(&pf.source)
            .map_err(|e| format!("read shipped {}: {}", data_relative, e))?;
        Ok((data_relative.clone(), bytes, git_file_mode(&pf.source)))
    };
    let pristine: Vec<(String, Vec<u8>, u32)> = local
        .merged
        .iter()
        .map(|(path, _)| upstream_bytes(path))
        .collect::<Result<_, _>>()?;

    let install_commit = crate::core::commit_data_paths_with_overrides(
        workspace_path,
        &installed_files,
        &pristine,
        &format!("Install plugin: {} v{}", manifest.id, manifest.version),
    )
    .map_err(|e| format!("commit installed plugin files: {}", e))?;

    // Second commit: the merged working tree and the saved-aside copies, so
    // `git status` is clean and the kept patch has a commit of its own.
    let mut local_paths = local.merged_paths();
    local_paths.extend(local.saved_aside.iter().cloned());
    let local_changes_commit = if local_paths.is_empty() {
        None
    } else {
        Some(
            crate::core::commit_data_paths_added(
                workspace_path,
                &local_paths,
                &format!(
                    "Keep local changes to plugin: {} v{}",
                    manifest.id, manifest.version
                ),
            )
            .map_err(|e| format!("commit kept local plugin changes: {}", e))?,
        )
    };

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
    payload.insert(
        "source_type".into(),
        serde_json::json!(source_type.as_str()),
    );
    payload.insert("commit".into(), serde_json::json!(install_commit));
    if let Some(tid) = setup_thread_id {
        payload.insert("setup_thread_id".into(), serde_json::json!(tid.to_string()));
    }

    bus.emit(BusEvent::System(SystemEvent::PluginInstalled {
        id: manifest.id.clone(),
        manifest: serde_json::Value::Object(payload),
        files: installed_files.clone(),
        installed_at,
        source_type: source_type.as_str().to_string(),
        actor,
    }))
    .await
    .map_err(|e| format!("event emit failed: {}", e))?;

    // One line, whatever the manifest carries. The receipt panel prints this
    // as its description. It renders the manifest's `setup` as its own
    // markdown section below, so setup text here shows the user the same wall
    // twice. No LLM reads it either: `install_plugin` returns the staging
    // sentinel, and the confirm reaching here arrives over HTTP.
    let result = format!(
        "Installed {} v{} ({} files).",
        manifest.name,
        manifest.version,
        installed_files.len()
    );
    Ok(InstallWrite {
        summary: result,
        installed_files,
        local_changes_commit,
    })
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

#[cfg(test)]
#[path = "../plugins_tests/triggers.rs"]
mod triggers_tests;
