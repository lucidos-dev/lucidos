use std::path::{Path, PathBuf};

use crate::core::plugins::{
    self, compare_versions, detect_conflicts, is_git_url, validate_archive_entry_path,
    validate_tree, PlannedFile, PluginManifest, UpdateDecision, ValidationError,
    AUTH_MODULES_DIR, PLUGIN_ARCHIVE_EXT,
};
use crate::core::DATA_DIR;
use crate::engine::event_bus::{BusEvent, EventBusEmitter, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::LucidosEngine;

/// Sentinel prefix on the `install_plugin` / `update_plugin` tool result. The
/// agentic loop strips it and re-emits a transient
/// `ThreadEvent::PluginInstallRequest` so the frontend can render the install
/// panel. Mirrors the credentials pattern in
/// `engine::tools::credentials::CREDENTIAL_REQUEST_PREFIX`.
pub(crate) const PLUGIN_INSTALL_REQUEST_PREFIX: &str = "[PLUGIN_INSTALL_REQUEST]";

/// Sentinel prefix on the `uninstall_plugin` tool result. Same pattern as
/// `PLUGIN_INSTALL_REQUEST_PREFIX` — agentic loop intercepts, emits a transient
/// `ThreadEvent::PluginUninstallRequest`, and the frontend renders the
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

/// Where the source string points and how to fetch it.
#[derive(Debug)]
enum Source {
    /// Full git URL with optional branch and subpath inside the repo.
    Git {
        url: String,
        branch: Option<String>,
        subpath: Option<String>,
    },
    /// Local path to a `.lucidos-plugin` archive.
    Archive(PathBuf),
}

/// How a successfully-fetched plugin was obtained. Serialized into
/// `PluginInstalled.source_type` via `as_str()` -- the wire format is
/// `"git"` or `"archive"` and downstream consumers (events table,
/// future projections) match on those literals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SourceType {
    Git,
    Archive,
}

impl SourceType {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Archive => "archive",
        }
    }
}

/// Detect the install source by shape of the input string.
fn detect_source(s: &str) -> Result<Source, String> {
    let trimmed = s.trim();

    // GitHub tree URL → repo + branch + subpath.
    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        if let Some(parsed) = parse_github_tree(rest) {
            return Ok(parsed);
        }
    }

    if is_git_url(trimmed) {
        return Ok(Source::Git {
            url: trimmed.to_string(),
            branch: None,
            subpath: None,
        });
    }

    if trimmed.ends_with(PLUGIN_ARCHIVE_EXT) {
        let path = PathBuf::from(trimmed);
        if path.is_file() {
            return Ok(Source::Archive(path));
        }
        return Err(format!(
            "archive path not found or not a file: {}",
            path.display()
        ));
    }

    Err(format!(
        "could not infer source type from '{}' — expected a git URL, a GitHub tree URL, or a path to a .lucidos-plugin file",
        trimmed
    ))
}

fn parse_github_tree(rest: &str) -> Option<Source> {
    let parts: Vec<&str> = rest.splitn(5, '/').collect();
    if parts.len() < 4 || parts[2] != "tree" {
        return None;
    }
    let owner = parts[0];
    let repo = parts[1].trim_end_matches(".git");
    let branch = parts[3].to_string();
    let subpath = if parts.len() == 5 && !parts[4].is_empty() {
        Some(parts[4].trim_end_matches('/').to_string())
    } else {
        None
    };
    Some(Source::Git {
        url: format!("https://github.com/{}/{}.git", owner, repo),
        branch: Some(branch),
        subpath,
    })
}

/// Fetch the source into a fresh `tempfile::TempDir` under
/// `.lucidos/tmp/plugins/`. Returns the TempDir guard (auto-cleans on drop)
/// and the plugin root inside it (where `manifest.toml` lives — for git-tree
/// URLs this is the subpath, not the repo root).
fn fetch_source(
    workspace: &Path,
    source: &Source,
) -> Result<(tempfile::TempDir, PathBuf, SourceType), String> {
    let parent = workspace.join(".lucidos").join("tmp").join("plugins");
    std::fs::create_dir_all(&parent).map_err(|e| format!("create scratch dir: {}", e))?;
    let scratch = tempfile::TempDir::new_in(&parent)
        .map_err(|e| format!("create scratch dir: {}", e))?;

    match source {
        Source::Git {
            url,
            branch,
            subpath,
        } => {
            let clone_target = scratch.path().join("repo");
            // git2 types are not Send — drop them before any await in the caller.
            {
                let mut builder = git2::build::RepoBuilder::new();
                let mut fetch_opts = git2::FetchOptions::new();
                // Shallow clones cut bandwidth for HTTPS/SSH but libgit2's
                // local transport rejects them ("shallow fetch is not
                // supported by the local transport"). Fall back to a full
                // clone for `file://` URLs — they're already local so the
                // bandwidth saving doesn't matter.
                if !url.starts_with("file://") {
                    fetch_opts.depth(1);
                }
                if let Some(b) = branch {
                    builder.branch(b);
                }
                builder.fetch_options(fetch_opts);
                builder
                    .clone(url, &clone_target)
                    .map_err(|e| format!("git clone failed: {}", e))?;
            }
            let _ = std::fs::remove_dir_all(clone_target.join(".git"));

            let plugin_root = match subpath {
                Some(sub) => clone_target.join(sub),
                None => clone_target.clone(),
            };
            if !plugin_root.is_dir() {
                return Err(format!(
                    "subpath not found inside repo: {}",
                    subpath.as_deref().unwrap_or("")
                ));
            }
            Ok((scratch, plugin_root, SourceType::Git))
        }
        Source::Archive(path) => {
            let extract_target = scratch.path().join("plugin");
            std::fs::create_dir_all(&extract_target)
                .map_err(|e| format!("create extract dir: {}", e))?;
            extract_zip(path, &extract_target)?;
            Ok((scratch, extract_target, SourceType::Archive))
        }
    }
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("open archive: {}", e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("read archive: {}", e))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("zip entry {}: {}", i, e))?;
        let raw_name = entry.name().to_string();
        validate_archive_entry_path(&raw_name)
            .map_err(|e: ValidationError| format!("rejected archive entry: {}", e))?;

        let outpath = dest.join(&raw_name);
        if entry.is_dir() {
            std::fs::create_dir_all(&outpath).map_err(|e| format!("mkdir {:?}: {}", outpath, e))?;
            continue;
        }
        if let Some(parent) = outpath.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {:?}: {}", parent, e))?;
        }
        let mut out = std::fs::File::create(&outpath)
            .map_err(|e| format!("create file {:?}: {}", outpath, e))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| format!("write file {:?}: {}", outpath, e))?;
    }
    Ok(())
}

/// Atomic-per-file copy: write to `<dest>.tmp` then rename. Caller is
/// responsible for ensuring the parent dir exists.
fn copy_atomic(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {:?}: {}", parent, e))?;
    }
    let tmp = dst.with_extension(format!(
        "{}.{}.tmp",
        dst.extension()
            .and_then(|s| s.to_str())
            .unwrap_or("plugin"),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::copy(src, &tmp).map_err(|e| format!("copy {:?} → {:?}: {}", src, tmp, e))?;
    std::fs::rename(&tmp, dst).map_err(|e| format!("rename {:?} → {:?}: {}", tmp, dst, e))?;
    Ok(())
}

/// Canonical plugin id for a `PluginInstalled` row: the `id` from the raw
/// manifest in the payload, falling back to the `aggregate_id` column when
/// the manifest field is absent. The fallback exists because legacy events
/// emitted before the `aggregate_id()` projection was fixed (2026-04 and
/// earlier) ended up with `aggregate_id = 'unknown'` even though their
/// payload manifest carries the real id; matching on the payload heals
/// those rows so a later, correctly-stamped `PluginUninstalled` still
/// supersedes them.
fn install_canonical_id(payload: &serde_json::Value, aggregate_id: &str) -> String {
    payload
        .pointer("/data/manifest/manifest/id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(aggregate_id)
        .to_string()
}

/// Canonical plugin id for a `PluginUninstalled` row: the `id` field in
/// the payload (always present from the start), falling back to the
/// `aggregate_id` column for symmetry with [`install_canonical_id`].
fn uninstall_canonical_id(payload: &serde_json::Value, aggregate_id: &str) -> String {
    payload
        .pointer("/data/id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(aggregate_id)
        .to_string()
}

/// Project every `PluginInstalled` / `PluginUninstalled` event into the
/// current installed-plugin set, keyed by canonical plugin id (see
/// [`install_canonical_id`]). Single full scan in sequence order — callers
/// that need both the id list and the records should reuse the result
/// rather than re-projecting per id.
async fn project_installs(
    pool: &sqlx::PgPool,
) -> Result<std::collections::BTreeMap<String, serde_json::Value>, Box<dyn std::error::Error + Send + Sync>>
{
    let rows: Vec<(String, String, serde_json::Value)> = sqlx::query_as(
        r#"SELECT event_type, aggregate_id, payload
           FROM events
           WHERE event_type IN ('PluginInstalled', 'PluginUninstalled')
           ORDER BY sequence ASC"#,
    )
    .fetch_all(pool)
    .await?;

    let mut state: std::collections::BTreeMap<String, serde_json::Value> =
        std::collections::BTreeMap::new();
    for (event_type, aggregate_id, payload) in rows {
        match event_type.as_str() {
            "PluginInstalled" => {
                let id = install_canonical_id(&payload, &aggregate_id);
                state.insert(id, payload);
            }
            "PluginUninstalled" => {
                let id = uninstall_canonical_id(&payload, &aggregate_id);
                state.remove(&id);
            }
            _ => {}
        }
    }
    Ok(state)
}

/// Read the latest known install record for a plugin id by scanning the
/// `events` table. Returns `None` if the plugin is not installed (never
/// installed, or installed then uninstalled).
async fn latest_install(
    pool: &sqlx::PgPool,
    id: &str,
) -> Result<Option<InstalledRecord>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(project_installs(pool)
        .await?
        .remove(id)
        .map(|payload| InstalledRecord { payload }))
}

struct InstalledRecord {
    payload: serde_json::Value,
}

impl InstalledRecord {
    // Raw manifest sits at `payload.data.manifest.manifest.*` — serde's
    // `tag/content` adds one wrapper, the payload-map-inside-SystemEvent.manifest
    // assignment adds the other. See `system-knowhow/building-a-plugin.md`.
    fn version(&self) -> Option<&str> {
        self.payload
            .pointer("/data/manifest/manifest/version")
            .and_then(|v| v.as_str())
    }
    fn source(&self) -> Option<&str> {
        self.payload
            .pointer("/data/manifest/manifest/source")
            .and_then(|v| v.as_str())
    }
    fn name(&self) -> Option<&str> {
        self.payload
            .pointer("/data/manifest/manifest/name")
            .and_then(|v| v.as_str())
    }
    fn files(&self) -> Vec<String> {
        self.payload
            .pointer("/data/files")
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Normalize a free-form plugin reference into the dash-slug form used by
/// install records — lowercased, with runs of whitespace / `_` / `-`
/// collapsed to a single `-`, and any leading/trailing dashes trimmed.
/// "No role playing" → "no-role-playing"; "anti_sycophancy_critique" →
/// "anti-sycophancy-critique". Used by `resolve_plugin_query` to compare a
/// human query against ids, manifest names, and app folders on equal terms.
pub(crate) fn normalize_plugin_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = true; // suppresses leading dashes
    for c in s.chars() {
        if c.is_whitespace() || c == '-' || c == '_' {
            if !prev_dash {
                out.push('-');
                prev_dash = true;
            }
        } else {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            prev_dash = false;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// One plugin that could match a `resolve_plugin_query` lookup. Held in a
/// per-install in-memory snapshot so the matcher can compare against ids,
/// manifest names, and app folders without re-querying the DB per entry.
#[derive(Debug, Clone)]
struct InstalledIndex {
    plugin_id: String,
    plugin_name: String,
    /// Pre-normalized strings the user might type to refer to this plugin:
    /// the id itself, the manifest name, and any `apps/<dir>/...` folder
    /// the plugin owns.
    aliases: std::collections::BTreeSet<String>,
}

impl InstalledIndex {
    fn from(record: &InstalledRecord, id: &str) -> Self {
        let mut aliases = std::collections::BTreeSet::new();
        aliases.insert(normalize_plugin_query(id));
        if let Some(name) = record.name() {
            aliases.insert(normalize_plugin_query(name));
        }
        for file in record.files() {
            // Each `apps/<folder>/...` entry contributes its folder as an
            // alias, so an LLM that picked the on-disk folder name (the
            // original "anti-sycophancy-critique" bug) still resolves to
            // the canonical plugin id.
            if let Some(rest) = file.strip_prefix("apps/") {
                if let Some(folder) = rest.split('/').next() {
                    if !folder.is_empty() {
                        aliases.insert(normalize_plugin_query(folder));
                    }
                }
            }
        }
        Self {
            plugin_id: id.to_string(),
            plugin_name: record.name().unwrap_or(id).to_string(),
            aliases,
        }
    }
}

async fn snapshot_installed(
    pool: &sqlx::PgPool,
) -> Result<Vec<InstalledIndex>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(project_installs(pool)
        .await?
        .into_iter()
        .map(|(id, payload)| {
            let rec = InstalledRecord { payload };
            InstalledIndex::from(&rec, &id)
        })
        .collect())
}

/// Resolve a free-form plugin reference (id, manifest name, or app folder
/// installed by the plugin) to the canonical plugin id stored in the
/// `PluginInstalled` event. Case-insensitive, dash/underscore/whitespace-
/// insensitive. Returns the canonical id on a single match; an error
/// message ready to surface to the LLM otherwise.
///
/// Errors:
/// - "Error: plugin '<query>' is not currently installed (...)" when no
///   currently-installed plugin matches.
/// - "Error: '<query>' matches multiple plugins: [a, b]. Re-run with the
///   exact id." when more than one match — never silently picks one.
pub(crate) async fn resolve_plugin_query(
    pool: &sqlx::PgPool,
    query: &str,
) -> Result<String, String> {
    if query.is_empty() {
        return Err("Error: plugin id/name is required".to_string());
    }

    // Fast path: exact id match still works without a snapshot scan.
    match latest_install(pool, query).await {
        Ok(Some(_)) => return Ok(query.to_string()),
        Ok(None) => {}
        Err(e) => return Err(format!("Error: read install record: {}", e)),
    }

    let installed = snapshot_installed(pool)
        .await
        .map_err(|e| format!("Error: list installed plugins: {}", e))?;
    let needle = normalize_plugin_query(query);
    if needle.is_empty() {
        return Err(format!(
            "Error: plugin '{}' is not currently installed (no PluginInstalled event, or already uninstalled)",
            query
        ));
    }

    let matches: Vec<&InstalledIndex> = installed
        .iter()
        .filter(|p| p.aliases.contains(&needle))
        .collect();

    match matches.as_slice() {
        [] => Err(format!(
            "Error: plugin '{}' is not currently installed (no PluginInstalled event, or already uninstalled)",
            query
        )),
        [single] => Ok(single.plugin_id.clone()),
        many => {
            let mut listing = many
                .iter()
                .map(|p| format!("'{}' (\"{}\")", p.plugin_id, p.plugin_name))
                .collect::<Vec<_>>();
            listing.sort();
            Err(format!(
                "Error: '{}' matches multiple plugins: [{}]. Re-run uninstall_plugin with the exact id.",
                query,
                listing.join(", ")
            ))
        }
    }
}

/// Identifier for the plugin that owns a given on-disk path. Returned by
/// `find_plugin_owning_file` so callers can include both the canonical id
/// (for the suggested `uninstall_plugin` command) and the human name
/// (for the message body) without re-querying.
#[derive(Debug, Clone)]
pub(crate) struct PluginOwner {
    pub(crate) plugin_id: String,
    pub(crate) plugin_name: String,
}

/// Find the currently-installed plugin (if any) that recorded
/// `data_relative` in its `PluginInstalled.files` list. Used by
/// `delete_file` to refuse direct deletes of plugin-owned files and route
/// the agent back through the `uninstall_plugin` confirm panel — the
/// "always confirm before deleting" invariant the agent broke when it
/// bypassed the panel and called `delete_file` per path.
///
/// Only the recorded files are guarded — user-authored files inside the
/// same app dir (e.g. notes the user added later) are NOT guarded, so
/// authoring tools keep working as today.
pub(crate) async fn find_plugin_owning_file(
    pool: &sqlx::PgPool,
    data_relative: &str,
) -> Result<Option<PluginOwner>, Box<dyn std::error::Error + Send + Sync>> {
    if data_relative.is_empty() {
        return Ok(None);
    }
    for (id, payload) in project_installs(pool).await? {
        let rec = InstalledRecord { payload };
        if rec.files().iter().any(|f| f == data_relative) {
            let plugin_name = rec.name().unwrap_or(&id).to_string();
            return Ok(Some(PluginOwner {
                plugin_id: id,
                plugin_name,
            }));
        }
    }
    Ok(None)
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

/// `check_plugin_updates(id?)` core. With `id == None`, surveys every
/// currently-installed plugin (newest `PluginInstalled` not followed by
/// `PluginUninstalled`); otherwise checks just the named plugin. Returns the
/// pretty-printed JSON report verbatim — LLMs and tests read the same shape.
pub(crate) async fn check_plugin_updates_impl(
    workspace_path: &Path,
    pool: &sqlx::PgPool,
    single_id: Option<String>,
) -> String {
    let entries: Vec<(String, Option<InstalledRecord>)> = match single_id {
        Some(id) => {
            let rec = match latest_install(pool, &id).await {
                Ok(rec) => rec,
                Err(e) => {
                    return format!("Error: read install record: {}", e);
                }
            };
            vec![(id, rec)]
        }
        None => {
            // Survey: project the install state once, then defensively skip
            // plugins whose recorded files are all gone from disk. Catches
            // future drift where an uninstall path forgets to emit the event.
            let projected = match project_installs(pool).await {
                Ok(p) => p,
                Err(e) => return format!("Error: list installed plugins: {}", e),
            };
            let data_dir = workspace_path.join(DATA_DIR);
            projected
                .into_iter()
                .filter_map(|(id, payload)| {
                    let rec = InstalledRecord { payload };
                    if all_recorded_files_missing(&data_dir, &rec.files()) {
                        None
                    } else {
                        Some((id, Some(rec)))
                    }
                })
                .collect()
        }
    };

    let mut report: Vec<serde_json::Value> = Vec::with_capacity(entries.len());
    for (id, rec) in entries {
        report.push(check_one(workspace_path, &id, rec).await);
    }

    serde_json::to_string_pretty(&report)
        .unwrap_or_else(|e| format!("Error: serialize report: {}", e))
}

/// Survey-only defensive filter: a plugin counts as actually-uninstalled
/// when every recorded file under `data/` is gone. Empty `files` (legacy
/// records that didn't list files) doesn't trigger the skip — only an
/// affirmative all-missing signal does.
fn all_recorded_files_missing(data_dir: &Path, files: &[String]) -> bool {
    if files.is_empty() {
        return false;
    }
    files.iter().all(|rel| !data_dir.join(rel).exists())
}

async fn check_one(
    workspace_path: &Path,
    id: &str,
    installed: Option<InstalledRecord>,
) -> serde_json::Value {
    let Some(installed) = installed else {
        return serde_json::json!({
            "id": id,
            "error": "plugin not installed (or already uninstalled)"
        });
    };

    let installed_version = installed.version().unwrap_or("unknown").to_string();
    let source = match installed.source() {
        Some(s) => s.to_string(),
        None => {
            return serde_json::json!({
                "id": id,
                "installed_version": installed_version,
                "error": "installed manifest is missing 'source' — cannot fetch latest"
            });
        }
    };

    match fetch_remote_manifest(workspace_path, &source).await {
        Ok(remote) => {
            let changed =
                compare_versions(&installed_version, &remote.version) == UpdateDecision::Update;
            serde_json::json!({
                "id": id,
                "installed_version": installed_version,
                "latest_version": remote.version,
                "changed": changed,
                "source": source,
                "remote_manifest": remote.raw,
            })
        }
        Err(e) => serde_json::json!({
            "id": id,
            "installed_version": installed_version,
            "source": source,
            "error": format!("fetch failed: {}", e)
        }),
    }
}

async fn fetch_remote_manifest(
    workspace_path: &Path,
    source_str: &str,
) -> Result<PluginManifest, String> {
    let source = detect_source(source_str)?;
    let (_scratch, plugin_root, _source_type) = fetch_source(workspace_path, &source)?;
    let manifest_path = plugin_root.join("manifest.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("read manifest: {}", e))?;
    plugins::parse_manifest(&text).map_err(|e| e.to_string())
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
        &name,
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
#[path = "plugins_tests.rs"]
mod tests;
