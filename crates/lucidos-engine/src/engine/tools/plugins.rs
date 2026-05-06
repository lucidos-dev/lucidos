use std::path::{Path, PathBuf};

use crate::core::plugins::{
    self, compare_versions, detect_conflicts, is_git_url, validate_archive_entry_path,
    validate_tree, PlannedFile, PluginManifest, UpdateDecision, ValidationError,
};
use crate::core::DATA_DIR;
use crate::engine::event_bus::{BusEvent, EventBusEmitter, SystemEvent};
use crate::engine::LucidosEngine;

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
pub(crate) enum SourceType {
    Git,
    Archive,
}

impl SourceType {
    fn as_str(self) -> &'static str {
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

    if trimmed.ends_with(".lucidos-plugin") {
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

/// Read the latest known install record for a plugin id by scanning the
/// `events` table. Returns `None` if the plugin is not installed (never
/// installed, or installed then uninstalled).
async fn latest_install(
    pool: &sqlx::PgPool,
    id: &str,
) -> Result<Option<InstalledRecord>, Box<dyn std::error::Error + Send + Sync>> {
    // Look at the newest install + newest uninstall for this id; if uninstall
    // is newer than install, the plugin is gone.
    let install: Option<(serde_json::Value, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        r#"SELECT payload, created
           FROM events
           WHERE event_type = 'PluginInstalled' AND aggregate_id = $1
           ORDER BY sequence DESC
           LIMIT 1"#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    let Some((payload, install_at)) = install else {
        return Ok(None);
    };

    let uninstall_at: Option<(chrono::DateTime<chrono::Utc>,)> = sqlx::query_as(
        r#"SELECT created
           FROM events
           WHERE event_type = 'PluginUninstalled' AND aggregate_id = $1
           ORDER BY sequence DESC
           LIMIT 1"#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    if let Some((u,)) = uninstall_at {
        if u >= install_at {
            return Ok(None);
        }
    }

    Ok(Some(InstalledRecord { payload }))
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

/// IDs of all currently-installed plugins (newest install not followed by an
/// uninstall). Used by `check_plugin_updates(None)`.
async fn list_installed_ids(
    pool: &sqlx::PgPool,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"SELECT DISTINCT aggregate_id
           FROM events
           WHERE event_type = 'PluginInstalled'"#,
    )
    .fetch_all(pool)
    .await?;
    let mut ids = Vec::with_capacity(rows.len());
    for (id,) in rows {
        if latest_install(pool, &id).await?.is_some() {
            ids.push(id);
        }
    }
    ids.sort();
    Ok(ids)
}

impl LucidosEngine {
    /// Dispatch a plugin tool by name. Returns the result string verbatim to
    /// the LLM (success or "Error: ..." line).
    pub(crate) async fn execute_plugin_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> String {
        match name {
            crate::llm::tool_names::INSTALL_PLUGIN => {
                let source_str = match args.get("source").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return "Error: source is required".to_string(),
                };
                let overwrite = args
                    .get("overwrite")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                match install_from_source_with_bus(
                    &self.workspace_path,
                    &self.event_bus,
                    &source_str,
                    overwrite,
                )
                .await
                {
                    Ok(msg) => msg,
                    Err(e) => format!("Error: {}", e),
                }
            }
            crate::llm::tool_names::CHECK_PLUGIN_UPDATES => {
                let single_id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                check_plugin_updates_impl(&self.workspace_path, &self.pool, single_id).await
            }
            crate::llm::tool_names::UPDATE_PLUGIN => {
                let id = match args.get("id").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return "Error: id is required".to_string(),
                };
                update_plugin_impl(&self.workspace_path, &self.event_bus, &self.pool, &id).await
            }
            crate::llm::tool_names::UNINSTALL_PLUGIN => {
                let id = match args.get("id").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return "Error: id is required".to_string(),
                };
                uninstall_plugin_impl(&self.event_bus, &self.pool, &id).await
            }
            other => format!("Error: unknown plugin tool '{}'", other),
        }
    }
}

/// Install a plugin by source string (git URL, GitHub tree URL, or
/// `.lucidos-plugin` archive path). Detects the shape, fetches into a temp
/// dir, then delegates to `install_from_unpacked_with_bus`. Free function so
/// the lifecycle tests can drive the same code path the LLM tool uses
/// without standing up a full `LucidosEngine`.
pub(crate) async fn install_from_source_with_bus(
    workspace_path: &Path,
    bus: &dyn EventBusEmitter,
    source_str: &str,
    overwrite: bool,
) -> Result<String, String> {
    let source = detect_source(source_str)?;
    let (_scratch, plugin_root, source_type) = fetch_source(workspace_path, &source)?;
    install_from_unpacked_with_bus(workspace_path, bus, &plugin_root, source_type, overwrite).await
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
    let ids = match single_id {
        Some(id) => vec![id],
        None => match list_installed_ids(pool).await {
            Ok(ids) => ids,
            Err(e) => return format!("Error: list installed plugins: {}", e),
        },
    };

    let mut report: Vec<serde_json::Value> = Vec::with_capacity(ids.len());
    for id in ids {
        report.push(check_one(workspace_path, pool, &id).await);
    }

    serde_json::to_string_pretty(&report)
        .unwrap_or_else(|e| format!("Error: serialize report: {}", e))
}

async fn check_one(
    workspace_path: &Path,
    pool: &sqlx::PgPool,
    id: &str,
) -> serde_json::Value {
    let installed = match latest_install(pool, id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => {
            return serde_json::json!({
                "id": id,
                "error": "plugin not installed (or already uninstalled)"
            });
        }
        Err(e) => {
            return serde_json::json!({ "id": id, "error": format!("read install record: {}", e) });
        }
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

/// `update_plugin(id)` core. Re-fetches the recorded `source`, compares
/// semver, and re-runs the install with `overwrite=true` when the remote
/// version is strictly greater. No-ops with a friendly message when already
/// at latest (including when the remote is older — intentional downgrades
/// aren't supported by `update_plugin`).
pub(crate) async fn update_plugin_impl(
    workspace_path: &Path,
    bus: &dyn EventBusEmitter,
    pool: &sqlx::PgPool,
    id: &str,
) -> String {
    let installed = match latest_install(pool, id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => {
            return format!(
                "Error: plugin '{}' is not currently installed (no PluginInstalled event, or already uninstalled)",
                id
            );
        }
        Err(e) => return format!("Error: read install record: {}", e),
    };

    let installed_version = installed.version().unwrap_or("unknown").to_string();
    let source = match installed.source() {
        Some(s) => s.to_string(),
        None => {
            return format!(
                "Error: installed manifest for '{}' is missing 'source' — cannot fetch latest",
                id
            );
        }
    };

    let remote = match fetch_remote_manifest(workspace_path, &source).await {
        Ok(m) => m,
        Err(e) => return format!("Error: fetch latest manifest: {}", e),
    };

    if compare_versions(&installed_version, &remote.version) == UpdateDecision::AlreadyLatest {
        return format!("Already at latest (v{})", installed_version);
    }

    match install_from_source_with_bus(workspace_path, bus, &source, true).await {
        Ok(msg) => msg,
        Err(e) => format!("Error: {}", e),
    }
}

/// `uninstall_plugin(id)` core. Looks up the latest install record, emits
/// `PluginUninstalled` with its file list, and returns the user-facing guide
/// text. v1 is guide-only — files stay until the LLM (or user) deletes them.
pub(crate) async fn uninstall_plugin_impl(
    bus: &dyn EventBusEmitter,
    pool: &sqlx::PgPool,
    id: &str,
) -> String {
    let installed = match latest_install(pool, id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => {
            return format!(
                "Error: plugin '{}' is not currently installed (no PluginInstalled event, or already uninstalled)",
                id
            );
        }
        Err(e) => return format!("Error: read install record: {}", e),
    };

    let version = installed.version().unwrap_or("unknown").to_string();
    let files = installed.files();

    match uninstall_with_bus(bus, id, &version, files).await {
        Ok(msg) => msg,
        Err(e) => format!("Error: {}", e),
    }
}

/// Install a plugin from an already-unpacked directory. Pure orchestration —
/// takes the workspace path and an event bus, so tests can inject a mock.
pub(crate) async fn install_from_unpacked_with_bus(
    workspace_path: &Path,
    bus: &dyn EventBusEmitter,
    plugin_root: &Path,
    source_type: SourceType,
    overwrite: bool,
) -> Result<String, String> {
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
        actor: None,
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
    Ok(result)
}

/// Emit `PluginUninstalled` and format the user-facing guide. Pulled out so
/// tests can call it without a PgPool.
pub(crate) async fn uninstall_with_bus(
    bus: &dyn EventBusEmitter,
    id: &str,
    version: &str,
    files: Vec<String>,
) -> Result<String, String> {
    bus.emit(BusEvent::System(SystemEvent::PluginUninstalled {
        id: id.to_string(),
        version: version.to_string(),
        files: files.clone(),
        actor: None,
    }))
    .await
    .map_err(|e| format!("emit PluginUninstalled: {}", e))?;

    let mut out = format!("Plugin \"{}\" v{} marked uninstalled.\n\n", id, version);
    if files.is_empty() {
        out.push_str("No files were recorded at install time — nothing to delete manually.\n");
    } else {
        out.push_str(&format!(
            "To remove its files, delete these {} paths under data/:\n",
            files.len()
        ));
        for f in &files {
            out.push_str(&format!("  - {}\n", f));
        }
        out.push_str(
            "\nSome files may have been edited since install, or shared with another plugin — \
             review before deletion.",
        );
    }
    Ok(out)
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
mod tests {
    use super::*;

    #[test]
    fn detect_source_github_tree_url_with_subpath() {
        let s = detect_source("https://github.com/lucidos-dev/plugins/tree/main/browser-learning")
            .unwrap();
        match s {
            Source::Git {
                url,
                branch,
                subpath,
            } => {
                assert_eq!(url, "https://github.com/lucidos-dev/plugins.git");
                assert_eq!(branch.as_deref(), Some("main"));
                assert_eq!(subpath.as_deref(), Some("browser-learning"));
            }
            other => panic!("expected Git, got {:?}", other),
        }
    }

    #[test]
    fn detect_source_github_tree_url_without_subpath() {
        let s = detect_source("https://github.com/lucidos-dev/plugin-x/tree/main").unwrap();
        match s {
            Source::Git {
                url,
                branch,
                subpath,
            } => {
                assert_eq!(url, "https://github.com/lucidos-dev/plugin-x.git");
                assert_eq!(branch.as_deref(), Some("main"));
                assert_eq!(subpath, None);
            }
            other => panic!("expected Git, got {:?}", other),
        }
    }

    #[test]
    fn detect_source_plain_https_repo() {
        let s = detect_source("https://github.com/x/y.git").unwrap();
        match s {
            Source::Git {
                url,
                branch,
                subpath,
            } => {
                assert_eq!(url, "https://github.com/x/y.git");
                assert_eq!(branch, None);
                assert_eq!(subpath, None);
            }
            other => panic!("expected Git, got {:?}", other),
        }
    }

    #[test]
    fn detect_source_ssh() {
        let s = detect_source("git@github.com:x/y.git").unwrap();
        assert!(matches!(s, Source::Git { .. }));
    }

    #[test]
    fn detect_source_archive_missing_file() {
        let err = detect_source("/tmp/no-such-thing.lucidos-plugin").unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn detect_source_unknown_shape() {
        let err = detect_source("just-a-name").unwrap_err();
        assert!(err.contains("could not infer"));
    }

    #[test]
    fn short_source_strips_https_and_git_suffix() {
        assert_eq!(
            short_source("https://github.com/a/b.git"),
            "github.com/a/b"
        );
        assert_eq!(short_source("https://github.com/a/b/"), "github.com/a/b");
    }

    #[test]
    fn validate_archive_entry_path_is_used() {
        // Smoke: the public function in core::plugins still rejects ../.
        assert!(validate_archive_entry_path("a/../b").is_err());
    }

    // --- Integration test: full install / conflict / overwrite / uninstall ---
    //
    // Builds a `.lucidos-plugin` zip in a temp dir, extracts it via the same
    // code path the live tool uses, and asserts the EventBus receives the
    // expected `PluginInstalled` and `PluginUninstalled` frames. Uses the
    // in-memory `MockEventBus` so no PgPool is needed.

    use crate::engine::event_bus::MockEventBus;
    use std::io::Write;

    const FIXTURE_MANIFEST: &str = r#"
id = "fixture-plugin"
version = "0.1.0"
name = "Fixture Plugin"
description = "test"
source = "https://github.com/x/y"
"#;

    fn build_archive(tmp: &Path, archive_name: &str, manifest: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let archive_path = tmp.join(archive_name);
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("manifest.toml", opts).unwrap();
        zip.write_all(manifest.as_bytes()).unwrap();
        for (path, body) in files {
            zip.start_file(*path, opts).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
        archive_path
    }

    fn build_fixture_archive(tmp: &Path, knowhow_body: &str) -> PathBuf {
        build_archive(
            tmp,
            "fixture.lucidos-plugin",
            FIXTURE_MANIFEST,
            &[
                ("knowhow/fixture.md", knowhow_body.as_bytes()),
                ("triggers/fixture/fixture.md", b"---\nname: Fixture\n---\nrun me"),
            ],
        )
    }

    fn extract_to(tmp: &Path, archive: &Path) -> PathBuf {
        let dest = tmp.join("unpacked");
        std::fs::create_dir_all(&dest).unwrap();
        super::extract_zip(archive, &dest).unwrap();
        dest
    }

    fn fresh_workspace() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "lucidos_plugins_int_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(p.join("data")).unwrap();
        p
    }

    fn build_sourceless_fixture(tmp: &Path) -> PathBuf {
        const SOURCELESS_MANIFEST: &str = r#"
id = "sourceless-plugin"
version = "0.1.0"
name = "Sourceless Plugin"
description = "test"
"#;
        build_archive(
            tmp,
            "sourceless.lucidos-plugin",
            SOURCELESS_MANIFEST,
            &[("knowhow/sourceless.md", b"---\nname: S\n---\nx")],
        )
    }

    #[tokio::test]
    async fn install_without_source_succeeds_and_omits_from_in_summary() {
        let scratch = fresh_workspace();
        let archive_dir = scratch.join("archive");
        std::fs::create_dir_all(&archive_dir).unwrap();
        let archive = build_sourceless_fixture(&archive_dir);
        let unpacked = extract_to(&archive_dir, &archive);

        let bus = MockEventBus::new();
        let msg = install_from_unpacked_with_bus(&scratch, &bus, &unpacked, SourceType::Archive, false)
            .await
            .expect("install must succeed even with no source field");
        assert_eq!(msg, "Installed Sourceless Plugin v0.1.0 (1 files).");

        let events = bus.emitted_events();
        assert_eq!(events.len(), 1);
        match &events[0] {
            BusEvent::System(SystemEvent::PluginInstalled { manifest, .. }) => {
                let summary = manifest
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                assert_eq!(
                    summary, "Installed Sourceless Plugin v0.1.0",
                    "summary must not include 'from <source>' when no source is set"
                );
                let payload_source = manifest
                    .get("manifest")
                    .and_then(|m| m.get("source"));
                assert!(
                    payload_source.is_none(),
                    "raw manifest in event payload must not contain a `source` key when the manifest omitted it (got: {:?})",
                    payload_source
                );
            }
            other => panic!("expected PluginInstalled, got {:?}", other),
        }

        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[tokio::test]
    async fn install_appends_setup_text_when_manifest_declares_it() {
        const SETUP_MANIFEST: &str = r#"
id = "with-setup"
version = "0.1.0"
name = "With Setup"
description = "Plugin that needs post-install wiring"
setup = "Create a daily trigger that loads `knowhow/with-setup/run.md`. Suggested cron: `0 0 4 * * *`."
"#;
        let scratch = fresh_workspace();
        let archive_dir = scratch.join("archive");
        std::fs::create_dir_all(&archive_dir).unwrap();
        let archive = build_archive(
            &archive_dir,
            "withsetup.lucidos-plugin",
            SETUP_MANIFEST,
            &[("knowhow/with-setup/run.md", b"---\nname: Run\n---\nx")],
        );
        let unpacked = extract_to(&archive_dir, &archive);

        let bus = MockEventBus::new();
        let msg = install_from_unpacked_with_bus(&scratch, &bus, &unpacked, SourceType::Archive, false)
            .await
            .expect("install must succeed");

        assert!(
            msg.starts_with("Installed With Setup v0.1.0 (1 files)."),
            "install summary line must come first, got: {:?}",
            msg
        );
        assert!(
            msg.contains("Setup:"),
            "tool result must label the setup section so the LLM acts on it, got: {:?}",
            msg
        );
        assert!(
            msg.contains("Create a daily trigger that loads `knowhow/with-setup/run.md`. Suggested cron: `0 0 4 * * *`."),
            "tool result must include the verbatim setup text from the manifest, got: {:?}",
            msg
        );

        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[tokio::test]
    async fn install_omits_setup_section_when_manifest_has_no_setup() {
        let scratch = fresh_workspace();
        let archive_dir = scratch.join("archive");
        std::fs::create_dir_all(&archive_dir).unwrap();
        let archive = build_fixture_archive(&archive_dir, "---\nname: F\n---\nx");
        let unpacked = extract_to(&archive_dir, &archive);

        let bus = MockEventBus::new();
        let msg = install_from_unpacked_with_bus(&scratch, &bus, &unpacked, SourceType::Archive, false)
            .await
            .expect("install must succeed");

        assert_eq!(msg, "Installed Fixture Plugin v0.1.0 (2 files).");

        let _ = std::fs::remove_dir_all(&scratch);
    }

    // ---- DB-backed regression + lifecycle tests --------------------------
    //
    // These exercise the live `EventBus` + `latest_install` path that the four
    // plugin tools (install / check_plugin_updates / update_plugin /
    // uninstall_plugin) actually run through. The MockEventBus tests above
    // assert event shape; these assert the round-trip from emit → DB →
    // `InstalledRecord` works, which is where the "missing source" regression
    // hid for so long.

    use crate::engine::event_bus::EventBus;
    use crate::test_support::{setup_test_db, teardown_test_db};

    /// Run a git command in `dir`, panicking on any failure. Used by the
    /// `file://` git-source tests where we stand up a bare repo locally so
    /// `update_plugin` has somewhere to re-fetch from without depending on
    /// the live `lucidos-dev/plugins` GitHub repo.
    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap_or_else(|e| panic!("git {} failed: {}", args.join(" "), e));
        assert!(
            out.status.success(),
            "git {} in {:?} failed: {}",
            args.join(" "),
            dir,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Stand up a bare git repo at `<scratch>/<name>.git` plus a working
    /// clone at `<scratch>/<name>-work/`, then commit `manifest.toml` and a
    /// `knowhow/<id>.md` file referencing `version`. Returns the bare repo
    /// path and the work tree so the caller can bump the version later.
    fn make_local_git_plugin(
        scratch: &Path,
        name: &str,
        id: &str,
        version: &str,
        knowhow_body: &str,
    ) -> (PathBuf, PathBuf) {
        let bare = scratch.join(format!("{}.git", name));
        let work = scratch.join(format!("{}-work", name));
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        git(&bare, &["init", "--bare", "--initial-branch=main"]);

        let bare_url = format!("file://{}", bare.display());
        let manifest = format!(
            r#"id = "{id}"
version = "{version}"
name = "Local Git Plugin"
description = "test"
source = "{bare_url}"
"#,
            id = id,
            version = version,
            bare_url = bare_url,
        );
        std::fs::write(work.join("manifest.toml"), manifest).unwrap();
        std::fs::create_dir_all(work.join("knowhow")).unwrap();
        std::fs::write(work.join(format!("knowhow/{}.md", id)), knowhow_body).unwrap();

        git(&work, &["init", "--initial-branch=main"]);
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "initial"]);
        git(&work, &["remote", "add", "origin", &bare.to_string_lossy()]);
        git(&work, &["push", "origin", "main"]);
        (bare, work)
    }

    /// Replace the manifest version + knowhow body in an existing work tree
    /// and push to the bare repo. Used by the `update_plugin` test.
    fn bump_local_git_plugin(
        work: &Path,
        id: &str,
        old_version: &str,
        new_version: &str,
        new_body: &str,
    ) {
        let manifest = std::fs::read_to_string(work.join("manifest.toml")).unwrap();
        let updated = manifest.replace(
            &format!("version = \"{}\"", old_version),
            &format!("version = \"{}\"", new_version),
        );
        std::fs::write(work.join("manifest.toml"), updated).unwrap();
        std::fs::write(work.join(format!("knowhow/{}.md", id)), new_body).unwrap();
        git(work, &["add", "."]);
        git(work, &["commit", "-m", "bump"]);
        git(work, &["push", "origin", "main"]);
    }

    /// Regression test for the "installed manifest is missing 'source'" bug.
    ///
    /// The PluginInstalled event payload nests the manifest inside the
    /// SystemEvent's `manifest` field (see `install_from_unpacked_with_bus`),
    /// and the persisted JSONB column wraps everything in serde's
    /// `{type, data}` envelope. Earlier versions of `InstalledRecord` read
    /// `payload.manifest.source`, which silently returned None and bubbled
    /// up as the misleading "installed manifest is missing 'source'" error
    /// from `check_plugin_updates`. This test installs a plugin via the same
    /// code path the LLM tool uses, then asserts the install record is
    /// findable by id and that source / version / files round-trip.
    #[tokio::test]
    async fn latest_install_round_trips_id_version_source_and_files() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb_rx) = EventBus::new(pool.clone());

        let scratch = fresh_workspace();
        let archive_dir = scratch.join("archive");
        std::fs::create_dir_all(&archive_dir).unwrap();
        let archive = build_fixture_archive(&archive_dir, "v1");
        let unpacked = extract_to(&archive_dir, &archive);

        install_from_unpacked_with_bus(&scratch, &bus, &unpacked, SourceType::Archive, false)
            .await
            .expect("install must succeed");

        // Install record must be findable by the manifest id (not "unknown").
        let installed = latest_install(&pool, "fixture-plugin")
            .await
            .expect("query must succeed")
            .expect("install record must be findable by manifest id, not 'unknown'");

        // Without the fix, all three of these return None / empty.
        assert_eq!(installed.version(), Some("0.1.0"), "version round-trip");
        assert_eq!(
            installed.source(),
            Some("https://github.com/x/y"),
            "source URL must be retrievable from install record — \
             this is the regression that caused 'missing source' errors in check_plugin_updates"
        );
        let mut files = installed.files();
        files.sort();
        assert_eq!(
            files,
            vec![
                "knowhow/fixture.md".to_string(),
                "triggers/fixture/fixture.md".to_string()
            ],
            "files list must round-trip from PluginInstalled payload"
        );

        let _ = std::fs::remove_dir_all(&scratch);
        teardown_test_db(&db_name).await;
    }

    /// Fetch the persisted payload(s) for an aggregate id, oldest first, so
    /// tests can index by emit order.
    async fn read_events(
        pool: &sqlx::PgPool,
        event_type: &str,
        id: &str,
    ) -> Vec<serde_json::Value> {
        let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
            r#"SELECT payload FROM events
               WHERE event_type = $1 AND aggregate_id = $2
               ORDER BY sequence ASC"#,
        )
        .bind(event_type)
        .bind(id)
        .fetch_all(pool)
        .await
        .expect("query events");
        rows.into_iter().map(|(p,)| p).collect()
    }

    // ---- e2e test 1 -------------------------------------------------------

    /// Install from a local `.lucidos-plugin` archive via the same code path
    /// the LLM tool uses (`install_from_source_with_bus`). Verifies files
    /// land under `data/<dir>/...` and that a PluginInstalled event is
    /// persisted with the manifest payload.
    #[tokio::test]
    async fn e2e_install_from_local_archive() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb_rx) = EventBus::new(pool.clone());

        let scratch = fresh_workspace();
        let archive_dir = scratch.join("archive");
        std::fs::create_dir_all(&archive_dir).unwrap();
        let archive = build_fixture_archive(&archive_dir, "v1-from-archive");

        let msg = install_from_source_with_bus(
            &scratch,
            &bus,
            archive.to_str().unwrap(),
            false,
        )
        .await
        .expect("install from archive must succeed");
        assert_eq!(msg, "Installed Fixture Plugin v0.1.0 (2 files).");

        // Files landed under data/.
        assert_eq!(
            std::fs::read_to_string(scratch.join("data/knowhow/fixture.md")).unwrap(),
            "v1-from-archive"
        );
        assert!(scratch.join("data/triggers/fixture/fixture.md").is_file());

        // Exactly one PluginInstalled event, payload reflects the manifest
        // and the archive source type.
        let events = read_events(&pool, "PluginInstalled", "fixture-plugin").await;
        assert_eq!(events.len(), 1, "exactly one PluginInstalled event");
        let raw_manifest = events[0]
            .pointer("/data/manifest/manifest")
            .expect("raw manifest must be nested at /data/manifest/manifest");
        assert_eq!(raw_manifest["id"], "fixture-plugin");
        assert_eq!(raw_manifest["version"], "0.1.0");
        assert_eq!(events[0]["data"]["source_type"], "archive");
        let files: Vec<&str> = events[0]["data"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(files.contains(&"knowhow/fixture.md"));
        assert!(files.contains(&"triggers/fixture/fixture.md"));

        let _ = std::fs::remove_dir_all(&scratch);
        teardown_test_db(&db_name).await;
    }

    // ---- e2e test 2 -------------------------------------------------------

    /// Install from a local bare git repo via `file://...git` URL — the same
    /// `install_from_source_with_bus` path that handles GitHub URLs in
    /// production. Verifies the source URL is retrievable from the install
    /// record so `check_plugin_updates` and `update_plugin` can re-fetch.
    #[tokio::test]
    async fn e2e_install_from_local_git_source_records_source_url() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb_rx) = EventBus::new(pool.clone());

        let scratch = fresh_workspace();
        let repos_dir = scratch.join("repos");
        std::fs::create_dir_all(&repos_dir).unwrap();
        let (bare, _work) = make_local_git_plugin(
            &repos_dir,
            "fixture-git",
            "git-fixture-plugin",
            "0.1.0",
            "v1 body",
        );
        let source_url = format!("file://{}", bare.display());

        let msg = install_from_source_with_bus(&scratch, &bus, &source_url, false)
            .await
            .expect("git install must succeed");
        assert_eq!(msg, "Installed Local Git Plugin v0.1.0 (1 files).");
        assert_eq!(
            std::fs::read_to_string(scratch.join("data/knowhow/git-fixture-plugin.md")).unwrap(),
            "v1 body"
        );

        // Source URL must round-trip — without it, update_plugin can't re-fetch.
        let installed = latest_install(&pool, "git-fixture-plugin")
            .await
            .unwrap()
            .expect("install record must exist");
        assert_eq!(installed.source(), Some(source_url.as_str()));
        assert_eq!(installed.version(), Some("0.1.0"));

        let events = read_events(&pool, "PluginInstalled", "git-fixture-plugin").await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["data"]["source_type"], "git");

        let _ = std::fs::remove_dir_all(&scratch);
        teardown_test_db(&db_name).await;
    }

    // ---- e2e test 3 -------------------------------------------------------

    /// Regression test for the "installed manifest is missing 'source'" bug
    /// at the tool-handler level (the version 1 test exercises only
    /// `InstalledRecord`). Drives the full `check_plugin_updates_impl` path
    /// the way the LLM dispatcher calls it, and asserts the JSON report
    /// contains real version + source data instead of the misleading error.
    #[tokio::test]
    async fn e2e_check_plugin_updates_returns_real_data_after_install() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb_rx) = EventBus::new(pool.clone());

        let scratch = fresh_workspace();
        let repos_dir = scratch.join("repos");
        std::fs::create_dir_all(&repos_dir).unwrap();
        let (bare, _work) = make_local_git_plugin(
            &repos_dir,
            "fixture-check",
            "check-fixture-plugin",
            "0.1.0",
            "stable body",
        );
        let source_url = format!("file://{}", bare.display());

        install_from_source_with_bus(&scratch, &bus, &source_url, false)
            .await
            .expect("install");

        let report_json = check_plugin_updates_impl(
            &scratch,
            &pool,
            Some("check-fixture-plugin".to_string()),
        )
        .await;
        let report: Vec<serde_json::Value> =
            serde_json::from_str(&report_json).expect("report parses as JSON");
        assert_eq!(report.len(), 1, "single id → single report entry");
        let entry = &report[0];

        assert_eq!(entry["id"], "check-fixture-plugin");
        assert!(
            entry.get("error").is_none(),
            "must NOT report 'missing source' — got: {}",
            entry
        );
        assert_eq!(entry["installed_version"], "0.1.0");
        assert_eq!(entry["latest_version"], "0.1.0");
        assert_eq!(entry["changed"], false);
        assert_eq!(entry["source"], source_url);

        let _ = std::fs::remove_dir_all(&scratch);
        teardown_test_db(&db_name).await;
    }

    // ---- e2e test 4 -------------------------------------------------------

    /// `update_plugin` re-fetches when the upstream version bumps. Installs
    /// v0.1.0, pushes a v0.1.1 commit to the same bare repo, then invokes
    /// `update_plugin_impl`. Asserts the new content lands on disk and a
    /// second PluginInstalled event is recorded (updates reuse that variant
    /// per the documented contract — there is no separate PluginUpdated).
    #[tokio::test]
    async fn e2e_update_plugin_re_fetches_when_version_bumps() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb_rx) = EventBus::new(pool.clone());

        let scratch = fresh_workspace();
        let repos_dir = scratch.join("repos");
        std::fs::create_dir_all(&repos_dir).unwrap();
        let (bare, work) = make_local_git_plugin(
            &repos_dir,
            "fixture-update",
            "update-fixture-plugin",
            "0.1.0",
            "v1 body",
        );
        let source_url = format!("file://{}", bare.display());

        // 1. Install v0.1.0.
        install_from_source_with_bus(&scratch, &bus, &source_url, false)
            .await
            .expect("install v0.1.0");
        let knowhow_path = scratch.join("data/knowhow/update-fixture-plugin.md");
        assert_eq!(std::fs::read_to_string(&knowhow_path).unwrap(), "v1 body");

        // 2. Bump upstream to v0.1.1.
        bump_local_git_plugin(&work, "update-fixture-plugin", "0.1.0", "0.1.1", "v2 body");

        // 3. update_plugin re-fetches and re-installs.
        let msg = update_plugin_impl(&scratch, &bus, &pool, "update-fixture-plugin").await;
        assert!(
            msg.starts_with("Installed Local Git Plugin v0.1.1"),
            "update message: {}",
            msg
        );

        // 4. New content is on disk and a second PluginInstalled was recorded.
        assert_eq!(std::fs::read_to_string(&knowhow_path).unwrap(), "v2 body");
        let events = read_events(&pool, "PluginInstalled", "update-fixture-plugin").await;
        assert_eq!(events.len(), 2, "install + update = 2 PluginInstalled events");
        assert_eq!(
            events[0].pointer("/data/manifest/manifest/version").unwrap(),
            "0.1.0"
        );
        assert_eq!(
            events[1].pointer("/data/manifest/manifest/version").unwrap(),
            "0.1.1"
        );

        // 5. Re-running update with no upstream change is a no-op (no third event).
        let again = update_plugin_impl(&scratch, &bus, &pool, "update-fixture-plugin").await;
        assert_eq!(again, "Already at latest (v0.1.1)");
        assert_eq!(
            read_events(&pool, "PluginInstalled", "update-fixture-plugin").await.len(),
            2,
            "no-op update must not emit a third PluginInstalled"
        );

        let _ = std::fs::remove_dir_all(&scratch);
        teardown_test_db(&db_name).await;
    }

    // ---- e2e test 5 -------------------------------------------------------

    /// `uninstall_plugin` emits PluginUninstalled with the install's file
    /// list. v1 is GUIDE-ONLY: the engine MUST NOT delete files — the LLM
    /// chains to a separate file-delete step once the user confirms.
    #[tokio::test]
    async fn e2e_uninstall_plugin_emits_event_and_does_not_delete_files() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb_rx) = EventBus::new(pool.clone());

        let scratch = fresh_workspace();
        let archive_dir = scratch.join("archive");
        std::fs::create_dir_all(&archive_dir).unwrap();
        let archive = build_fixture_archive(&archive_dir, "stays on disk");

        install_from_source_with_bus(&scratch, &bus, archive.to_str().unwrap(), false)
            .await
            .expect("install");
        let knowhow_path = scratch.join("data/knowhow/fixture.md");
        let trigger_path = scratch.join("data/triggers/fixture/fixture.md");
        assert!(knowhow_path.is_file());

        let msg = uninstall_plugin_impl(&bus, &pool, "fixture-plugin").await;
        assert!(
            msg.contains("Plugin \"fixture-plugin\" v0.1.0 marked uninstalled."),
            "uninstall message: {}",
            msg
        );
        assert!(msg.contains("knowhow/fixture.md"));
        assert!(msg.contains("triggers/fixture/fixture.md"));

        // Guide-only: files MUST NOT be deleted.
        assert!(
            knowhow_path.is_file(),
            "uninstall must not delete files — v1 is guide-only"
        );
        assert!(trigger_path.is_file());

        // Event recorded with the file list.
        let events = read_events(&pool, "PluginUninstalled", "fixture-plugin").await;
        assert_eq!(events.len(), 1, "exactly one PluginUninstalled event");
        let payload = &events[0];
        assert_eq!(payload["data"]["id"], "fixture-plugin");
        assert_eq!(payload["data"]["version"], "0.1.0");
        let recorded_files: Vec<&str> = payload["data"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(recorded_files.contains(&"knowhow/fixture.md"));
        assert!(recorded_files.contains(&"triggers/fixture/fixture.md"));

        // After uninstall, latest_install returns None for this id.
        let after = latest_install(&pool, "fixture-plugin").await.unwrap();
        assert!(
            after.is_none(),
            "uninstall must hide the install record from subsequent lookups"
        );

        let _ = std::fs::remove_dir_all(&scratch);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn install_then_conflict_then_overwrite_then_uninstall() {
        let scratch = fresh_workspace();
        let workspace = scratch.clone();
        let archive_dir = scratch.join("archive");
        std::fs::create_dir_all(&archive_dir).unwrap();
        let archive = build_fixture_archive(&archive_dir, "v1");
        let unpacked = extract_to(&archive_dir, &archive);

        let bus = MockEventBus::new();

        // 1. Fresh install lands files and emits PluginInstalled.
        let msg = install_from_unpacked_with_bus(&workspace, &bus, &unpacked, SourceType::Archive, false)
            .await
            .expect("install should succeed on empty workspace");
        assert_eq!(msg, "Installed Fixture Plugin v0.1.0 (2 files).");

        let kn_path = workspace.join("data/knowhow/fixture.md");
        let trig_path = workspace.join("data/triggers/fixture/fixture.md");
        assert!(kn_path.is_file(), "knowhow/fixture.md missing");
        assert!(trig_path.is_file(), "triggers/fixture/fixture.md missing");
        assert_eq!(std::fs::read_to_string(&kn_path).unwrap(), "v1");

        let events = bus.emitted_events();
        assert_eq!(events.len(), 1, "exactly one event after first install");
        match &events[0] {
            BusEvent::System(SystemEvent::PluginInstalled {
                manifest,
                files,
                source_type,
                installed_at,
                ..
            }) => {
                assert_eq!(source_type, "archive");
                assert!(!installed_at.is_empty());
                let mut sorted = files.clone();
                sorted.sort();
                assert_eq!(
                    sorted,
                    vec![
                        "knowhow/fixture.md".to_string(),
                        "triggers/fixture/fixture.md".to_string()
                    ]
                );
                let payload_id = manifest
                    .get("manifest")
                    .and_then(|m| m.get("id"))
                    .and_then(|v| v.as_str());
                assert_eq!(payload_id, Some("fixture-plugin"));
                let payload_summary = manifest
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                assert!(
                    payload_summary.starts_with("Installed Fixture Plugin v0.1.0 from "),
                    "summary was: {}",
                    payload_summary
                );
            }
            other => panic!("expected PluginInstalled, got {:?}", other),
        }

        // 2. Re-install without overwrite returns the conflict error and emits nothing new.
        let v2_archive_dir = scratch.join("archive_v2");
        std::fs::create_dir_all(&v2_archive_dir).unwrap();
        let archive_v2 = build_fixture_archive(&v2_archive_dir, "v2");
        let unpacked_v2 = extract_to(&v2_archive_dir, &archive_v2);

        let err = install_from_unpacked_with_bus(&workspace, &bus, &unpacked_v2, SourceType::Archive, false)
            .await
            .expect_err("second install must hit conflict");
        assert!(
            err.contains("would overwrite"),
            "conflict message was: {}",
            err
        );
        assert!(
            err.contains("knowhow/fixture.md"),
            "conflict message must list the file: {}",
            err
        );
        assert_eq!(
            bus.emitted_events().len(),
            1,
            "conflict path must not emit a second event"
        );
        // File on disk unchanged — still v1.
        assert_eq!(std::fs::read_to_string(&kn_path).unwrap(), "v1");

        // 3. Re-install with overwrite=true succeeds and the file content updates.
        let msg2 =
            install_from_unpacked_with_bus(&workspace, &bus, &unpacked_v2, SourceType::Archive, true)
                .await
                .expect("overwrite install should succeed");
        assert_eq!(msg2, "Installed Fixture Plugin v0.1.0 (2 files).");
        assert_eq!(
            std::fs::read_to_string(&kn_path).unwrap(),
            "v2",
            "overwrite must replace file content"
        );
        assert_eq!(
            bus.emitted_events().len(),
            2,
            "overwrite must emit a new PluginInstalled"
        );

        // 4. Uninstall via the helper (mirrors what execute_uninstall_plugin would do
        //    after fetching the install record). Verify the event payload + the
        //    user-facing message lists every file the install recorded.
        let installed_files = match &bus.emitted_events()[1] {
            BusEvent::System(SystemEvent::PluginInstalled { files, .. }) => files.clone(),
            other => panic!("expected PluginInstalled at index 1, got {:?}", other),
        };
        let uninstall_msg = uninstall_with_bus(&bus, "fixture-plugin", "0.1.0", installed_files.clone())
            .await
            .expect("uninstall should succeed");
        assert!(
            uninstall_msg.contains("Plugin \"fixture-plugin\" v0.1.0 marked uninstalled."),
            "uninstall message: {}",
            uninstall_msg
        );
        assert!(uninstall_msg.contains("knowhow/fixture.md"));
        assert!(uninstall_msg.contains("triggers/fixture/fixture.md"));
        // Guide-only: files are NOT deleted.
        assert!(kn_path.is_file(), "uninstall must NOT delete files (v1 is guide-only)");

        let final_events = bus.emitted_events();
        assert_eq!(final_events.len(), 3);
        match &final_events[2] {
            BusEvent::System(SystemEvent::PluginUninstalled {
                id, version, files, ..
            }) => {
                assert_eq!(id, "fixture-plugin");
                assert_eq!(version, "0.1.0");
                let mut sorted = files.clone();
                sorted.sort();
                assert_eq!(
                    sorted,
                    vec![
                        "knowhow/fixture.md".to_string(),
                        "triggers/fixture/fixture.md".to_string()
                    ]
                );
            }
            other => panic!("expected PluginUninstalled, got {:?}", other),
        }

        let _ = std::fs::remove_dir_all(&scratch);
    }
}
