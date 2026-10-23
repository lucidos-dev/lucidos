//! Plugin LLM tools: install / marketplace registration / update / uninstall flows.
//!
//! This module owns the user-confirm staging flows (install + uninstall),
//! the `LucidosEngine` tool dispatch, and the shared pending-map plumbing.
//! Three helper concerns live in child modules:
//!
//! - [`source`] — install-source detection + fetching.
//! - [`registry`] — installed-plugin projection / query / update-check.
//! - [`merge`]: what an update does to the files on disk: the three-way merge
//!   against the user's local edits, and the removal of dropped files.
//! - [`marketplaces`]: the single write path for the marketplace registry,
//!   shared with `api::plugins` and announcing every mutation.
//!
//! Splitting is purely structural; the public surface stays reachable at
//! `crate::engine::tools::plugins::*` via the re-exports below.

use std::path::{Path, PathBuf};

use crate::core::git_auth::GitCredentials;
use crate::core::plugins::{
    check_engine_requirement, check_font_room, compare_versions, detect_conflicts,
    engine_requirement_of, validate_tree, PlannedFile, PluginManifest, UpdateDecision,
    AUTH_MODULES_DIR,
};
use crate::core::{plugin_media, DATA_DIR};
use crate::engine::event_bus::{BusEvent, EventBusEmitter, SystemEvent};
use crate::engine::release_notices::running_release;
use crate::engine::thread_events::{
    EngineReason, MessageOrigin, PluginSetupCause, PluginSetupOccasion,
};
use crate::engine::tools::agent_tool_actor;
use crate::engine::trigger_writes::TriggerWrite;
use crate::engine::LucidosEngine;
use crate::triggers::affected::{self, AffectedTrigger};
use crate::triggers::TriggerConfig;

mod affected_triggers;
pub(crate) mod marketplaces;
mod merge;
mod registry;
mod source;

use merge::{LocalChangeOutcome, ResolvedChange, UpdatePlan};
use registry::{
    check_plugin_updates_impl, fetch_remote_manifest, latest_install, not_installed_error,
    project_installed_plugins, resolve_plugin_query, InstalledPlugins, PluginBaseline, PriorSetup,
};
use source::{
    copy_atomic, credentials_for_source, detect_source, fetch_source, git_file_mode, write_atomic,
    write_atomic_like, SourceType,
};

/// Every installed plugin and its merge baseline, or why they could not be
/// read. Resolved before the blocking fetch and handed into it, since the
/// staged plugin's id is only known once its manifest parses inside that task.
type InstalledPluginsRead = Result<InstalledPlugins, String>;

// Named by other modules via the `plugins::` path (`engine::tools::files`
// routes plugin-owned deletes here), so it stays a re-export.
pub(crate) use registry::find_plugin_owning_file;
// The `delete_app` HTTP handler (`api::apps`) routes plugin-owned app deletes
// to the uninstall panel via this — the app-level mirror of the file guard.
pub(crate) use registry::find_plugin_owning_app;
pub(crate) use registry::installed_plugin_summaries;

/// Sentinel prefix on the `install_plugin` / `update_plugin` tool result. The
/// agentic loop strips it and emits the *form request*
/// `ThreadEvent::PluginInstallRequested` so the frontend can render the install
/// panel. Mirrors the credentials pattern in
/// `engine::tools::credentials::CREDENTIAL_REQUEST_PREFIX`.
pub(crate) const PLUGIN_INSTALL_REQUEST_PREFIX: &str = "[PLUGIN_INSTALL_REQUEST]";

/// Sentinel prefix on the `uninstall_plugin` tool result. Same pattern as
/// `PLUGIN_INSTALL_REQUEST_PREFIX`: the agentic loop intercepts, emits the form
/// request `ThreadEvent::PluginUninstallRequested`, and the frontend renders the
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
    /// The setup the staged version asks for. On confirm, [`setup_occasion`]
    /// decides from it whether a Lucidos Agent thread walks the user through
    /// the author's setup instructions.
    pub(crate) setup: StagedSetup,
    /// What this install would do to the user's edited files, and which files
    /// it would remove. Decided at staging time, so the confirm panel can state
    /// it per file. Empty for a fresh install.
    pub(crate) plan: UpdatePlan,
    /// Every installed plugin's recorded paths as the plan saw them, `None`
    /// when they could not be read. See [`check_records_still_match`].
    pub(crate) recorded_files: Option<RecordedFiles>,
    pub(crate) created_at: chrono::DateTime<chrono::Utc>,
}

/// Recorded paths per installed plugin id: [`InstalledPlugins::files`].
type RecordedFiles = std::collections::BTreeMap<String, Vec<String>>;

/// Refuse a confirm whose plan the install records no longer support. A newer
/// version of this plugin may record files the plan never saw, and another
/// install may now record a path the plan removes. Acting on the old plan
/// would orphan the first and delete the second.
///
/// A failed read is unknown, so it refuses only a confirm that deletes.
pub(crate) fn check_records_still_match(
    plugin_id: &str,
    staged: Option<&RecordedFiles>,
    now: &InstalledPluginsRead,
    removals: &[String],
) -> Result<(), String> {
    let Some(staged) = staged else {
        return Ok(());
    };
    match now {
        Ok(now) => {
            let others = now.recorded_by_others(plugin_id);
            if now.files.get(plugin_id) == staged.get(plugin_id)
                && !removals.iter().any(|rel| others.contains(rel.as_str()))
            {
                return Ok(());
            }
            Err(
                "The installed plugins changed after you reviewed this update. Run the \
                 update again to see what it would do now."
                    .to_string(),
            )
        }
        Err(e) if !removals.is_empty() => Err(format!(
            "Could not re-read the installed plugins' records, so Lucidos cannot tell whether \
             this update would delete another plugin's files. Nothing was changed. Try again \
             in a moment. Cause: {}",
            e
        )),
        Err(_) => Ok(()),
    }
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
                super::lift_legacy_string(stage_uninstall_request(self, &id).await)
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

pub(crate) type PendingInstallsMap =
    std::sync::Mutex<std::collections::HashMap<String, PendingInstall>>;

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
    // Read every installed plugin up front. The staged plugin's id appears
    // only when the fetched manifest parses, inside the blocking task. So it
    // cannot look up its own install record from in there.
    let installed = project_installed_plugins(&engine.pool).await.map_err(|e| {
        log!(@Plugins, "read installed plugins failed: {}", e);
        e.to_string()
    });
    // The staging body is synchronous, so its credentials are resolved here.
    let credentials = credentials_for_source(&engine.pool, source_str).await;
    let running = running_release();
    let staged = match tokio::task::spawn_blocking(move || {
        prepare_install_request(
            &workspace,
            &pending,
            &source,
            &installed,
            &credentials,
            &running,
        )
    })
    .await
    {
        Ok(s) => s,
        Err(e) => format!("Error: install staging task panicked: {}", e),
    };
    affected_triggers::annotate_staged(staged, &affected_triggers::snapshot_triggers(engine))
}

/// Stage an uninstall (the uninstall counterpart of [`stage_install_request`]).
/// Resolves `id`, builds a `PendingUninstall`, and returns the
/// `[PLUGIN_UNINSTALL_REQUEST]` sentinel. The `uninstall_plugin` LLM tool and
/// the Plugins panel's route both call it, so both land in the same panel.
pub(crate) async fn stage_uninstall_request(engine: &LucidosEngine, id: &str) -> String {
    let staged = prepare_uninstall_plugin(
        &engine.workspace_path,
        &engine.pool,
        &engine.pending_uninstalls,
        id,
    )
    .await;
    affected_triggers::annotate_staged(staged, &affected_triggers::snapshot_triggers(engine))
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

/// The setup a staged plugin version asks for, as the manifest states it.
/// `text` is normalized; `revision` is 0 when the manifest has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StagedSetup {
    pub(crate) text: Option<String>,
    pub(crate) revision: u64,
    pub(crate) reason: Option<String>,
}

impl StagedSetup {
    fn from_manifest(manifest: &PluginManifest) -> Self {
        Self {
            text: normalize_setup(manifest.setup.as_deref()),
            revision: manifest.setup_revision,
            reason: manifest.setup_reason.clone(),
        }
    }
}

/// Decide whether confirming an install or update spawns a setup thread, and
/// why. Staging (the panel's note) and confirm (the spawn) both call this, so
/// the panel never promises a thread confirm would skip. `prior` is the
/// installed version's setup state, `None` on a fresh install.
///
/// - no `setup` in the staged version: no thread.
/// - no prior record: a fresh install.
/// - text differs from the installed version's: an update, instructions changed.
/// - revision greater than the installed one: an update, revision raised.
/// - both: an update for both reasons.
/// - neither: no thread. Re-running identical setup on every version bump is
///   noise, and a revision carried unchanged into the next release or lowered
///   asks for nothing.
pub(crate) fn setup_occasion(
    staged: &StagedSetup,
    prior: Option<&PriorSetup>,
) -> Option<PluginSetupOccasion> {
    let text = staged.text.as_ref()?;
    let Some(prior) = prior else {
        return Some(PluginSetupOccasion::FreshInstall);
    };
    let changed = prior.text.as_ref() != Some(text);
    let raised = staged.revision > prior.revision;
    let (revision, reason) = (staged.revision, staged.reason.clone());
    let cause = match (changed, raised) {
        (true, true) => PluginSetupCause::Both { revision, reason },
        (true, false) => PluginSetupCause::InstructionsChanged,
        (false, true) => PluginSetupCause::RevisionRaised { revision, reason },
        (false, false) => return None,
    };
    Some(PluginSetupOccasion::Update {
        from_version: prior.version.clone(),
        cause,
    })
}

/// Stage `source_str` into a temp dir, validate the manifest + tree, register
/// the result in `pending_installs`, and return the
/// `[PLUGIN_INSTALL_REQUEST]<json>` sentinel. On any failure returns
/// `"Error: ..."` and no entry is registered. Free function so tests can
/// drive it without standing up a `LucidosEngine`. `running` is
/// [`running_release`], passed in so tests can stage against any release.
pub(crate) fn prepare_install_request(
    workspace_path: &Path,
    pending_installs: &std::sync::Arc<PendingInstallsMap>,
    source_str: &str,
    installed: &InstalledPluginsRead,
    credentials: &GitCredentials,
    running: &Result<semver::Version, String>,
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
    // Before anything is registered, so a refusal opens no panel and leaves
    // nothing behind but the staging dir, which drops on return.
    if let Err(mismatch) = check_engine_requirement(&manifest.raw, running) {
        return format!(
            "Error: {}",
            mismatch.refusal(&manifest.name, &manifest.version)
        );
    }

    let data_dir = workspace_path.join(DATA_DIR);
    if let Err(e) = check_font_room(&planned, &data_dir) {
        return format!("Error: {}", e);
    }
    let overwrites = detect_conflicts(&planned, &data_dir);
    let install_id = uuid::Uuid::new_v4().to_string();

    // Normalize the author's setup instructions once: trimmed, with empty
    // collapsed to `None`. The same value feeds the panel preview AND the
    // pending entry that decides whether confirm spawns a setup thread.
    let setup = StagedSetup::from_manifest(&manifest);

    let is_update = installed
        .as_ref()
        .is_ok_and(|installed| installed.contains(&manifest.id));
    // The same decision confirm makes, so the panel states what confirm does.
    // Confirm re-decides from the live record, which stays the authority.
    let setup_run = setup_occasion(
        &setup,
        installed
            .as_ref()
            .ok()
            .and_then(|installed| installed.setups.get(&manifest.id)),
    );

    // An update: work out per file whether the user's edit merges, conflicts,
    // or is lost, and which files the new version dropped. The panel then says
    // so before they confirm. A fresh install has no plan.
    // Unreadable install records refuse any staging that would overwrite a
    // file: the panel could not name the user's edits, and confirm would not
    // save them.
    let plan = match installed {
        Ok(installed) => match installed.files.get(&manifest.id) {
            Some(recorded) => merge::plan_update(
                workspace_path,
                recorded,
                installed.baselines.get(&manifest.id),
                &planned,
                &installed.recorded_by_others(&manifest.id),
            ),
            None => UpdatePlan::default(),
        },
        Err(e) if !overwrites.is_empty() => {
            return format!(
                "Error: could not read the installed plugins' records, so Lucidos cannot tell \
                 whether '{}' would overwrite your local edits. Nothing was changed. Try again \
                 in a moment. Cause: {}",
                manifest.id, e
            );
        }
        Err(_) => UpdatePlan::default(),
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
        "removes": plan.removes(),
        "is_update": is_update,
        "local_changes": plan.preview_json(),
        "setup": setup.text,
        "setup_run": setup_run,
        "plugin_id": manifest.id,
        "plugin_version": manifest.version,
        "plugin_name": manifest.name,
        "engine_requirement": engine_requirement_of(&manifest.raw),
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
        plan,
        recorded_files: installed.as_ref().ok().map(|i| i.files.clone()),
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

/// Drop pending entries older than `ttl_secs` and return their ids. Both
/// install and uninstall maps use this to keep abandoned entries from leaking.
fn sweep_stale<T: HasCreatedAt>(
    map: &std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, T>>>,
    ttl_secs: i64,
) -> Vec<String> {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(ttl_secs);
    let mut guard = map.lock().expect("pending map mutex poisoned");
    let mut dropped = Vec::new();
    guard.retain(|id, entry| {
        let live = entry.created_at() >= cutoff;
        if !live {
            dropped.push(id.clone());
        }
        live
    });
    dropped
}

/// Drop the stagings past their TTL from both maps. Returns the ids dropped, so
/// the caller can close their *form requests* as expired.
pub(crate) fn sweep_expired_stagings(
    installs: &std::sync::Arc<PendingInstallsMap>,
    uninstalls: &std::sync::Arc<PendingUninstallsMap>,
) -> Vec<String> {
    let mut dropped = sweep_stale(installs, PENDING_INSTALL_TTL_SECS);
    dropped.extend(sweep_stale(uninstalls, PENDING_UNINSTALL_TTL_SECS));
    dropped
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

/// Whether an update may delete a recorded path the new version dropped.
/// Checked at staging and again at confirm. A trigger definition never
/// qualifies: it is a gitignored projection, and the trigger re-sync deletes
/// it together with the trigger.
fn update_may_remove(data_relative: &str) -> bool {
    is_safe_data_path(data_relative) && plugin_trigger_slug(data_relative).is_none()
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
    /// Triggers the plugin does not own that relied on a deleted file.
    pub affected_triggers: Vec<AffectedTrigger>,
}

/// Pop the pending uninstall, delete each `files_present` entry from `data/`,
/// prune empty parent directories, reload the WASM signer map if any
/// `auth-modules/` files were touched, and emit `PluginUninstalled` (extended
/// payload). The `actor` is the device/user who clicked Confirm; it stamps
/// the resulting event, and the popover names that device from its id. Mirrors
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
            &affected_triggers::snapshot_triggers(engine),
            actor.clone(),
        )
        .await?
    };

    // Auto-delete the plugin's auto-registered triggers (ADR 0019) — scoped by
    // plugin_id, so user-created triggers are untouched. Emitted after the
    // PluginUninstalled so the scheduler unregisters them + drops their
    // trigger.toml projection.
    delete_plugin_triggers(engine, &pending.plugin_id, actor).await;

    reload_auth_modules_if_needed(engine, &outcome.files_deleted, "uninstall").await;

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
/// payload without standing up the engine. `triggers` is every trigger, for
/// reporting the ones a deleted file leaves without their knowhow.
pub(crate) async fn uninstall_with_bus(
    workspace_path: &Path,
    bus: &dyn EventBusEmitter,
    pending: &PendingUninstall,
    triggers: &[TriggerConfig],
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

    // Media is never in the file list, so it goes by the plugin id.
    let media = plugin_media::installed_dir(workspace_path, &pending.plugin_id);
    // The id comes from the install record. Never delete through one that
    // names more than one folder.
    let removal = match plugin_media::is_folder_name(&pending.plugin_id) {
        true => plugin_media::remove_copy(&media),
        false => Ok(()),
    };
    let media_note = removal.err().map(|e| {
        log!("[Plugins] could not delete {}: {}", media.display(), e);
        format!(" Its pictures could not be deleted: {e}.")
    });

    let mut all_files = pending.files_present.clone();
    for f in &pending.files_missing {
        if !all_files.contains(f) {
            all_files.push(f.clone());
        }
    }

    let affected = affected::affected_triggers(&files_deleted, triggers, &pending.plugin_id);
    bus.emit(BusEvent::System(SystemEvent::PluginUninstalled {
        id: pending.plugin_id.clone(),
        version: pending.plugin_version.clone(),
        files: all_files,
        files_deleted: files_deleted.clone(),
        files_missing: files_missing_now.clone(),
        affected_triggers: affected.clone(),
        actor: actor.clone(),
    }))
    .await
    .map_err(|e| format!("emit PluginUninstalled: {}", e))?;
    affected_triggers::notify(
        bus,
        &pending.plugin_name,
        affected_triggers::Removal::Uninstall,
        &affected,
        actor,
    )
    .await;

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
    summary.push_str(&media_note.unwrap_or_default());

    Ok(ConfirmedUninstall {
        summary,
        files_deleted,
        files_missing: files_missing_now,
        affected_triggers: affected,
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
    /// Edited files the new version dropped, saved aside and then removed.
    removed: Vec<String>,
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
                LocalChangeOutcome::Removed => writes.removed.push(path),
            }
        }
        writes
    }

    fn is_empty(&self) -> bool {
        self.merged.is_empty()
            && self.conflicted.is_empty()
            && self.replaced.is_empty()
            && self.restored.is_empty()
            && self.removed.is_empty()
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
    /// Edited files the new version no longer ships. Each has a saved copy.
    pub removed: Vec<String>,
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
    /// Files the previous version shipped and this one dropped, now deleted.
    pub removed_files: Vec<String>,
    /// Triggers the plugin does not own that relied on a removed file.
    pub affected_triggers: Vec<AffectedTrigger>,
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

    // Pre-allocate the setup thread id only when this confirm has setup work
    // to run: see [`setup_occasion`]. A version bump with nothing to run
    // spawns no thread and navigates nowhere. The recorded id is what lets the
    // Plugins panel card resolve the right thread for its Setup→Open state
    // after a reload.
    //
    // `latest_install` here still sees the *previous* version, since the new
    // `PluginInstalled` event is not emitted until `install_from_unpacked_with_bus`
    // below. A failed read costs the prior record, so the install reads as a
    // fresh one: run the setup in full rather than skip real work.
    let prior = match latest_install(&engine.pool, &pending.plugin_id).await {
        Ok(rec) => rec.map(|rec| rec.prior_setup()),
        Err(e) => {
            log!(
                "[Plugins] prior-install lookup failed for '{}' (treating setup as new): {}",
                pending.plugin_id,
                e
            );
            None
        }
    };
    let occasion = setup_occasion(&pending.setup, prior.as_ref());
    let setup_thread_id = occasion.as_ref().map(|_| uuid::Uuid::new_v4());

    let (write, local_changes) = {
        let _repo_guard = engine.lock_workspace_repo().await;

        // The panel stated an outcome per edited or removed file. If one of
        // those moved since, the staged plan describes content that is gone,
        // and acting on it would clobber the newer edit unseen.
        if let Some(path) = pending.plan.detect_drift(&engine.workspace_path) {
            return Err(format!(
                "'{}' changed after you reviewed this update. Run the update \
                 again to see what it would do now.",
                path
            ));
        }
        let removals = pending.plan.removes();
        let records_now = project_installed_plugins(&engine.pool)
            .await
            .map_err(|e| e.to_string());
        check_records_still_match(
            &pending.plugin_id,
            pending.recorded_files.as_ref(),
            &records_now,
            &removals,
        )?;
        let resolved = pending.plan.resolve(keep_local_changes);
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
            removed: writes.removed.clone(),
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
                removals,
                triggers: affected_triggers::snapshot_triggers(engine),
                ..InstallContext::plain(pending.source_type, true)
            },
        )
        .await?;
        if let Some(report) = report.as_mut() {
            report.commit = write.local_changes_commit.clone();
        }
        (write, report)
    };
    let InstallWrite {
        summary,
        installed_files,
        removed_files,
        affected_triggers,
        ..
    } = write;

    announce_local_changes(engine, &pending, local_changes.as_ref(), actor.clone()).await;

    let touched: Vec<String> = installed_files
        .iter()
        .chain(&removed_files)
        .cloned()
        .collect();
    reload_auth_modules_if_needed(engine, &touched, "install").await;

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
    // `occasion` is `None` for an absent or blank `setup`.
    if let (Some(thread_id), Some(occasion)) = (setup_thread_id, occasion) {
        let plugin = SeededPlugin {
            id: pending.plugin_id.clone(),
            name: pending.plugin_name.clone(),
            version: pending.plugin_version.clone(),
        };
        spawn_plugin_setup_thread(engine, thread_id, &plugin, occasion, actor).await;
    }

    Ok(ConfirmedInstall {
        summary,
        installed_files,
        removed_files,
        affected_triggers,
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
                removed: report.removed.clone(),
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
                    // A plugin update the user confirmed, not a thread's turn.
                    writer_thread_id: None,
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
                    // Written before the thread that proposes it exists.
                    None,
                )),
                "[Plugins] ArtifactCreated",
            )
            .await;
    }

    let thread_id = uuid::Uuid::new_v4();
    let plugin = SeededPlugin {
        id,
        name: plugin_name,
        version,
    };
    let request =
        build_upstream_patch_thread_request(thread_id, &plugin, &patch_path, actor.as_ref());
    engine.thread_queue.submit(request, actor, None).await;

    Ok(ProposedUpstream {
        patch_path,
        thread_id,
    })
}

/// The Thread Queue request for an upstream-patch thread. A `SubThread` for the
/// same reason as the setup thread: its `prepare` step emits the seeding
/// `MessageReceived` eagerly, so the frontend can navigate into a real thread.
///
/// The seed is one user-facing line naming the plugin and the patch. How to
/// actually open the pull request lives in `system-knowhow/plugins.md`, which
/// keeps procedure out of the prompt. The engine wrote the line, so the engine
/// is its origin, with the requesting device carried beside it.
fn build_upstream_patch_thread_request(
    thread_id: uuid::Uuid,
    plugin: &SeededPlugin,
    patch_path: &str,
    actor: Option<&MessageOrigin>,
) -> crate::engine::thread_queue::ThreadQueueRequest {
    let plugin_name = &plugin.name;
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
        wake_mode: None,
        title: Some(format!("Propose {plugin_name} patch upstream")),
        model: None,
        reasoning_effort: None,
        provider: None,
        pre_emitted_origin: None,
        origin: Some(MessageOrigin::engine(
            EngineReason::PluginUpstreamProposal {
                plugin_id: plugin.id.clone(),
                plugin_name: plugin.name.clone(),
                version: plugin.version.clone(),
                patch_path: patch_path.to_string(),
                confirmed_on_device_id: confirming_device_id(actor),
            },
        )),
    }
}

/// The plugin an engine-seeded thread is about: what its seed names and what
/// its `EngineReason` records.
#[derive(Debug)]
struct SeededPlugin {
    id: String,
    name: String,
    /// The version installed now.
    version: String,
}

/// The device behind a confirm, when a device made it. The `ThreadQueued`
/// audit event still records any other caller. Only a device has a name the
/// popover can show.
fn confirming_device_id(actor: Option<&MessageOrigin>) -> Option<String> {
    match actor {
        Some(MessageOrigin::Device { device_id }) => Some(device_id.clone()),
        _ => None,
    }
}

/// Spawn a Lucidos Agent thread that walks the user through completing a
/// plugin's setup (asking questions, wiring config). The seed is a short
/// user-facing line; the "how to" lives in `system-knowhow/plugin-setup` and
/// the author's `setup` text is referenced from the `PluginInstalled` event
/// (see `build_setup_thread_request`). Submitted through the Thread Queue like
/// any background spawn, so it respects admission control. `thread_id` is
/// pre-allocated by the caller, so the same id can be recorded in the
/// `PluginInstalled` event. The submission never fails for this kind (overflow
/// is per-trigger only): the thread is admitted at once or queued, never
/// dropped.
///
/// Spawned as a [`ThreadQueueRequest::SubThread`], NOT `AgentChat`, so it
/// materializes before navigation. The executor's `prepare` step emits the
/// seeding `MessageReceived` eagerly, before `submit` returns on the
/// immediate-admit path. The `thread_summaries` row therefore exists by the
/// time this confirm responds, and the frontend navigates into a real thread.
///
/// `actor` attributes the `ThreadQueued` audit event via `submit`, and its
/// device (when it is one) rides on the seed's `EngineReason`.
async fn spawn_plugin_setup_thread(
    engine: &LucidosEngine,
    thread_id: uuid::Uuid,
    plugin: &SeededPlugin,
    occasion: PluginSetupOccasion,
    actor: Option<MessageOrigin>,
) {
    let request = build_setup_thread_request(thread_id, plugin, occasion, actor.as_ref());
    engine.thread_queue.submit(request, actor, None).await;
}

/// Build the Thread Queue request for a plugin setup thread. Pure (no engine,
/// no I/O) so the request shape is unit-testable. The `SubThread` choice and
/// the bound `child_thread_id` are load-bearing (see `spawn_plugin_setup_thread`
/// for why), and a regression back to `AgentChat` would silently break setup.
///
/// The seed is a SHORT, user-facing line: the only thing shown in the thread's
/// first bubble. The "how to run a plugin setup" meta-instructions live in
/// `system-knowhow/plugin-setup`, and the plugin author's own `setup` text is
/// referenced from the durable `PluginInstalled` event. Neither is embedded
/// here, so the user isn't shown a wall of agent instructions.
///
/// An update says so in both the seed and the title. The agent then picks up
/// where the last run left off instead of starting over, and the thread list
/// stops showing the same name twice.
///
/// Two words are load-bearing across layers. Every seed keeps the verb `set
/// up`, which the system-prompt route and the knowhow's `description` key on.
/// Every update seed says `again`, which is how the knowhow tells the two
/// occasions apart. An update seed's wording also names its cause, which the
/// knowhow branches on.
///
/// The engine wrote the seed, so its origin is `EngineReason::PluginSetup`.
/// The confirming device is secondary attribution, never the origin: a device
/// origin would render the engine's words as the user's own.
fn build_setup_thread_request(
    thread_id: uuid::Uuid,
    plugin: &SeededPlugin,
    occasion: PluginSetupOccasion,
    actor: Option<&MessageOrigin>,
) -> crate::engine::thread_queue::ThreadQueueRequest {
    let plugin_name = &plugin.name;
    let (prompt, title) = match &occasion {
        PluginSetupOccasion::Update {
            from_version,
            cause,
        } => (
            update_seed(plugin, from_version.as_deref(), cause),
            format!("Update {plugin_name} setup"),
        ),
        PluginSetupOccasion::FreshInstall => (
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
        wake_mode: None,
        title: Some(title),
        model: None,
        reasoning_effort: None,
        provider: None,
        pre_emitted_origin: None,
        origin: Some(MessageOrigin::engine(EngineReason::PluginSetup {
            plugin_id: plugin.id.clone(),
            plugin_name: plugin.name.clone(),
            version: plugin.version.clone(),
            occasion,
            confirmed_on_device_id: confirming_device_id(actor),
        })),
    }
}

/// The seed line for a setup update. The wording says which cause ran it, so
/// the agent knows whether the setup text changed or only the author asked.
fn update_seed(plugin: &SeededPlugin, prior: Option<&str>, cause: &PluginSetupCause) -> String {
    let name = &plugin.name;
    let since = prior.map(|v| format!(" since version {v}"));
    let (line, reason) = match cause {
        PluginSetupCause::InstructionsChanged => (
            match &since {
                Some(since) => format!("its setup instructions changed{since}."),
                None => "this update changed its setup instructions.".to_string(),
            },
            None,
        ),
        PluginSetupCause::RevisionRaised { reason, .. } => (
            match &since {
                Some(since) => format!(
                    "version {} asks to run its setup again{since}.",
                    plugin.version
                ),
                None => "this update asks to run its setup again.".to_string(),
            },
            reason.as_deref(),
        ),
        PluginSetupCause::Both { reason, .. } => (
            match &since {
                Some(since) => format!(
                    "its setup instructions changed{since}, and the author asks to run it again."
                ),
                None => "this update changed its setup instructions, \
                         and the author asks to run it again."
                    .to_string(),
            },
            reason.as_deref(),
        ),
    };
    let note = reason
        .map(|r| format!(" Author's note: {r}"))
        .unwrap_or_default();
    format!("Set up {name} again: {line}{note}")
}

/// Reload the proxy's WASM signer map when `touched_files` reaches into
/// `auth-modules/`. `operation` names the install or uninstall for the log,
/// which has already succeeded whatever the reload does.
async fn reload_auth_modules_if_needed(
    engine: &LucidosEngine,
    touched_files: &[String],
    operation: &str,
) {
    let auth_prefix = format!("{}/", AUTH_MODULES_DIR);
    if touched_files.iter().any(|p| p.starts_with(&auth_prefix)) {
        if let Err(e) =
            crate::api::proxy::reload_proxy_modules_into(engine, &engine.workspace_path).await
        {
            log!(
                "[Plugins] auto-reload after {operation} failed ({operation} still succeeded): {}",
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
    /// `data/`-relative paths the previous version shipped and this one
    /// dropped, deleted after the writes. Edited ones are already saved aside.
    pub(crate) removals: Vec<String>,
    /// Every trigger, for reporting the ones a removal leaves without their
    /// knowhow. Empty when nothing is removed.
    pub(crate) triggers: Vec<TriggerConfig>,
}

impl InstallContext {
    /// An install with no local edits to weigh, nothing to remove, and nobody
    /// to attribute it to.
    pub(crate) fn plain(source_type: SourceType, overwrite: bool) -> Self {
        Self {
            source_type,
            overwrite,
            actor: None,
            setup_thread_id: None,
            local: LocalChangeWrites::default(),
            removals: Vec::new(),
            triggers: Vec::new(),
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
    /// Every `data/`-relative path deleted because this version dropped it.
    pub(crate) removed_files: Vec<String>,
    pub(crate) affected_triggers: Vec<AffectedTrigger>,
    /// The second commit, recording the merged working tree plus any
    /// saved-aside copies. `None` when the install met no local edits.
    pub(crate) local_changes_commit: Option<String>,
}

/// Delete the files the previous version shipped and this one dropped, and
/// prune the folders that leaves empty. Returns the paths deleted.
///
/// Runs after the writes, so a failed write deletes nothing. The guard is
/// re-checked here, whatever the staging decided.
fn remove_dropped_files(data_dir: &Path, removals: &[String]) -> Result<Vec<String>, String> {
    let mut removed = Vec::with_capacity(removals.len());
    for rel in removals.iter().filter(|rel| update_may_remove(rel)) {
        match std::fs::remove_file(data_dir.join(rel)) {
            Ok(()) => {
                prune_empty_parents(data_dir, rel);
                removed.push(rel.clone());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(format!(
                    "remove {} (the new files are already written): {}",
                    rel, e
                ))
            }
        }
    }
    Ok(removed)
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
        removals,
        triggers,
    } = ctx;
    let (manifest, planned) = validate_tree(plugin_root).map_err(|e| e.to_string())?;
    let data_dir = workspace_path.join(DATA_DIR);

    // Plugins may only ship event-driven (`on:`) triggers — reject a shipped
    // `trigger.toml` with a cron `schedule` BEFORE any file is written (clean
    // failure, nothing copied). Cron is workspace state, not plugin content
    // (ADR 0019, plugins.md "What doesn't belong in a plugin").
    validate_plugin_triggers_event_driven(&planned)?;
    check_font_room(&planned, &data_dir)?;

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
    let removed_files = remove_dropped_files(&data_dir, &removals)?;
    let affected = affected::affected_triggers(&removed_files, &triggers, &manifest.id);

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

    // The deletions land in the same commit, so it holds exactly what this
    // version ships and nothing an older one left behind.
    let install_commit = crate::core::commit_data_paths_with_overrides(
        workspace_path,
        &installed_files,
        &pristine,
        &removed_files,
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

    let media_note = keep_installed_media(workspace_path, plugin_root, &manifest);

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
        files_removed: removed_files.clone(),
        affected_triggers: affected.clone(),
        installed_at,
        source_type: source_type.as_str().to_string(),
        actor: actor.clone(),
    }))
    .await
    .map_err(|e| format!("event emit failed: {}", e))?;
    affected_triggers::notify(
        bus,
        &manifest.name,
        affected_triggers::Removal::Update {
            version: &manifest.version,
        },
        &affected,
        actor,
    )
    .await;

    // One line, whatever the manifest carries. The receipt panel prints this
    // as its description. It renders the manifest's `setup` as its own
    // markdown section below, so setup text here shows the user the same wall
    // twice. No LLM reads it either: `install_plugin` returns the staging
    // sentinel, and the confirm reaching here arrives over HTTP.
    let removed_note = match removed_files.len() {
        0 => String::new(),
        1 => " Removed 1 file this version no longer ships.".to_string(),
        n => format!(" Removed {n} files this version no longer ships."),
    };
    let result = format!(
        "Installed {} v{} ({} files).{}{}",
        manifest.name,
        manifest.version,
        installed_files.len(),
        removed_note,
        media_note.unwrap_or_default()
    );
    Ok(InstallWrite {
        summary: result,
        installed_files,
        removed_files,
        affected_triggers: affected,
        local_changes_commit,
    })
}

/// Copy the plugin's media to its installed media copy, replacing whatever an
/// older version left there. Never fails the install (ADR 0414). A copy that
/// cannot be written answers a sentence for the receipt instead.
fn keep_installed_media(
    workspace_path: &Path,
    plugin_root: &Path,
    manifest: &crate::core::plugins::PluginManifest,
) -> Option<String> {
    let resolved = plugin_media::resolve(plugin_root, &manifest.media);
    let copy = plugin_media::installed_dir(workspace_path, &manifest.id);
    let error = plugin_media::write_copy(&resolved, &copy).err()?;
    log!(
        "[Plugins] could not keep the media of '{}': {}",
        manifest.id,
        error
    );
    Some(format!(" Its pictures could not be kept: {error}."))
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

#[cfg(test)]
#[path = "../plugins_tests/media.rs"]
mod media_tests;

#[cfg(test)]
#[path = "../plugins_tests/widgets.rs"]
mod widgets_tests;

#[cfg(test)]
#[path = "../plugins_tests/affected_triggers.rs"]
mod affected_triggers_tests;
