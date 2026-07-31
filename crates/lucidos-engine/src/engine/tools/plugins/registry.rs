//! Installed-plugin registry: project `PluginInstalled` / `PluginUninstalled`
//! events into the current installed set, resolve free-form plugin queries to
//! canonical ids, find the plugin owning a given file, and run the
//! `check_plugin_updates` survey. Read-only over the event store.

use std::collections::BTreeSet;
use std::path::Path;

use crate::core::plugin_marketplaces::InstalledPluginSummary;
use crate::core::plugins::{self, compare_versions, PluginManifest, UpdateDecision};
use crate::core::DATA_DIR;
use crate::triggers::definition::TriggerDefinition;

use super::source::{detect_source, fetch_source};

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
) -> Result<
    std::collections::BTreeMap<String, serde_json::Value>,
    Box<dyn std::error::Error + Send + Sync>,
> {
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

pub(crate) async fn installed_plugin_summaries(
    pool: &sqlx::PgPool,
    workspace_path: &Path,
) -> Result<Vec<InstalledPluginSummary>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(project_installs(pool)
        .await?
        .into_iter()
        .map(|(id, payload)| {
            let rec = InstalledRecord { payload };
            let files = rec.files();
            let app_id = primary_app_id(&files, &id);
            let content = crate::core::plugin_marketplaces::content_dirs_from_files(
                files.iter().map(String::as_str),
            );
            let status = plugin_modification_status(workspace_path, &rec);
            InstalledPluginSummary {
                id: id.clone(),
                name: rec.name().unwrap_or(&id).to_string(),
                version: rec.version().unwrap_or("unknown").to_string(),
                source: rec.source().map(ToOwned::to_owned),
                setup_thread_id: rec.setup_thread_id().map(ToOwned::to_owned),
                app_id,
                content,
                files,
                modified: status.modified,
                modified_paths: status.modified_paths,
            }
        })
        .collect())
}

/// Read the latest known install record for a plugin id by scanning the
/// `events` table. Returns `None` if the plugin is not installed (never
/// installed, or installed then uninstalled).
pub(crate) async fn latest_install(
    pool: &sqlx::PgPool,
    id: &str,
) -> Result<Option<InstalledRecord>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(project_installs(pool)
        .await?
        .remove(id)
        .map(|payload| InstalledRecord { payload }))
}

pub(crate) struct InstalledRecord {
    payload: serde_json::Value,
}

impl InstalledRecord {
    // Raw manifest sits at `payload.data.manifest.manifest.*` — serde's
    // `tag/content` adds one wrapper, the payload-map-inside-SystemEvent.manifest
    // assignment adds the other. See `system-knowhow/building-a-plugin.md`.
    pub(crate) fn version(&self) -> Option<&str> {
        self.payload
            .pointer("/data/manifest/manifest/version")
            .and_then(|v| v.as_str())
    }
    pub(crate) fn source(&self) -> Option<&str> {
        self.payload
            .pointer("/data/manifest/manifest/source")
            .and_then(|v| v.as_str())
    }
    pub(crate) fn name(&self) -> Option<&str> {
        self.payload
            .pointer("/data/manifest/manifest/name")
            .and_then(|v| v.as_str())
    }
    pub(crate) fn files(&self) -> Vec<String> {
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
    // `setup_thread_id` sits alongside `summary`/`files`/`installed_at` in the
    // event's `manifest` payload map (one level above the raw toml, which is
    // the nested `manifest/manifest`). Absent on installs that shipped no
    // `setup` field and on every pre-feature legacy row.
    pub(crate) fn setup_thread_id(&self) -> Option<&str> {
        self.payload
            .pointer("/data/manifest/setup_thread_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
    }
    /// The installed version's raw manifest `setup` instructions (the nested
    /// `manifest/manifest/setup`, i.e. as the author wrote them — one level
    /// below the payload map that holds `setup_thread_id`). Read at confirm
    /// time to decide whether an update introduces *new* setup work or just
    /// re-runs identical instructions. `None` when the installed version
    /// shipped no `setup` field.
    pub(crate) fn setup(&self) -> Option<&str> {
        self.payload
            .pointer("/data/manifest/manifest/setup")
            .and_then(|v| v.as_str())
    }
    /// The workspace-repo commit sha captured when this version was installed
    /// (the "Install plugin: <id> v<version>" commit). The baseline
    /// [`plugin_modification_status`] diffs the plugin's current on-disk content
    /// against to decide whether the user has locally modified it. `None` for
    /// legacy rows installed before the sha was recorded — those degrade to
    /// "not modified" (no baseline to diff against).
    pub(crate) fn commit(&self) -> Option<&str> {
        self.payload
            .pointer("/data/manifest/commit")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
    }
}

/// The app a plugin's "Open" button should launch: the first `apps/<app-id>/`
/// directory among the installed files, preferring an app whose id equals the
/// plugin id (the common one-app-named-after-the-plugin case). Returns `None`
/// for plugins that install no app (knowhow/triggers/scripts only).
pub(crate) fn primary_app_id(files: &[String], plugin_id: &str) -> Option<String> {
    let mut app_ids: Vec<&str> = files
        .iter()
        .filter_map(|f| f.strip_prefix("apps/"))
        .filter_map(|rest| rest.split('/').next())
        .filter(|seg| !seg.is_empty())
        .collect();
    app_ids.sort_unstable();
    app_ids.dedup();
    if app_ids.contains(&plugin_id) {
        return Some(plugin_id.to_string());
    }
    app_ids.first().map(|s| s.to_string())
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

/// Find the currently-installed plugin (if any) that installed the app
/// `app_id` — i.e. recorded any `apps/<app_id>/...` path in its
/// `PluginInstalled.files` list. The app-deletion mirror of
/// [`find_plugin_owning_file`]: the HTTP `delete_app` handler calls this to
/// refuse a raw delete of a plugin-installed app and route the user to the
/// plugin uninstall confirm panel — the single removal authority — instead of
/// `rm -rf`-ing the app dir and orphaning the plugin record. Standalone apps
/// (no `PluginInstalled` record) resolve to `None` and delete as before.
pub(crate) async fn find_plugin_owning_app(
    pool: &sqlx::PgPool,
    app_id: &str,
) -> Result<Option<PluginOwner>, Box<dyn std::error::Error + Send + Sync>> {
    if app_id.is_empty() {
        return Ok(None);
    }
    let prefix = format!("apps/{}/", app_id);
    for (id, payload) in project_installs(pool).await? {
        let rec = InstalledRecord { payload };
        if rec.files().iter().any(|f| f.starts_with(&prefix)) {
            let plugin_name = rec.name().unwrap_or(&id).to_string();
            return Ok(Some(PluginOwner {
                plugin_id: id,
                plugin_name,
            }));
        }
    }
    Ok(None)
}

/// Whether a plugin's shipped content currently differs from what it installed,
/// and which `data/`-relative paths differ. Derived on read (git + disk); never
/// stored.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ModificationStatus {
    pub(crate) modified: bool,
    pub(crate) modified_paths: Vec<String>,
}

/// Compare two trigger definitions on their *functional* fields only — ignoring
/// `slug` (normalized to the directory name at registration), `plugin_id`
/// (stamped at registration, absent in the shipped authored file), and
/// `group_id` (user-organizational, not a content edit). Used for the trigger
/// arm of [`plugin_modification_status`], because `trigger.toml` is a gitignored,
/// re-serialized projection (ADR 0019) whose bytes drift from the shipped form on
/// every rebuild — only a semantic compare is meaningful.
fn trigger_functional_eq(a: &TriggerDefinition, b: &TriggerDefinition) -> bool {
    let normalize = |d: &TriggerDefinition| {
        let mut d = d.clone();
        d.slug = String::new();
        d.plugin_id = None;
        d.group_id = None;
        d
    };
    normalize(a) == normalize(b)
}

/// Decide whether the user has locally modified a plugin's shipped content since
/// it was installed, and list the changed `data/`-relative paths. Pure read over
/// the workspace git repo + disk — no stored state, so it self-heals when an edit
/// is reverted and resets when the plugin is updated (the new install commit is
/// the fresh baseline).
///
/// Per content type, against the plugin's install commit:
/// - **app dirs** (`apps/<id>/`): a tree→workdir diff (including untracked files)
///   scoped to the dir — catches edits, deletes, and user-ADDED files in the app.
/// - **flat files** (`knowhow/` / `scripts/` / `auth-modules/` and any non-
///   `trigger.toml` file the plugin recorded): the same diff scoped to each
///   recorded path — edits/deletes only (a brand-new file in those shared roots
///   isn't attributable to the plugin).
/// - **triggers** (`triggers/<slug>/trigger.toml`): NOT byte-diffed — the file is
///   a gitignored, re-serialized projection, so the shipped definition (parsed
///   from the install-commit blob) is compared field-by-field against the current
///   on-disk definition via [`trigger_functional_eq`].
///
/// Returns not-modified for a legacy record with no recorded install commit, or
/// when the repo / commit can't be opened (best-effort — a badge is not worth
/// erroring a list load over).
pub(crate) fn plugin_modification_status(
    workspace_path: &Path,
    record: &InstalledRecord,
) -> ModificationStatus {
    let unmodified = ModificationStatus::default();
    let Some(commit_sha) = record.commit() else {
        return unmodified;
    };
    let files = record.files();

    // Partition recorded files by how each content type must be compared.
    let mut app_dirs: BTreeSet<String> = BTreeSet::new();
    let mut flat_files: Vec<String> = Vec::new();
    let mut trigger_files: Vec<String> = Vec::new();
    for f in &files {
        if let Some(rest) = f.strip_prefix("apps/") {
            if let Some(seg) = rest.split('/').next().filter(|s| !s.is_empty()) {
                app_dirs.insert(format!("apps/{}", seg));
                continue;
            }
        }
        if f.starts_with("triggers/") && f.ends_with("/trigger.toml") {
            trigger_files.push(f.clone());
            continue;
        }
        flat_files.push(f.clone());
    }

    let repo = match git2::Repository::open(workspace_path) {
        Ok(r) => r,
        Err(_) => return unmodified,
    };
    let tree = match repo
        .revparse_single(commit_sha)
        .and_then(|obj| obj.peel_to_commit())
        .and_then(|c| c.tree())
    {
        Ok(t) => t,
        Err(_) => return unmodified,
    };

    let mut changed: BTreeSet<String> = BTreeSet::new();

    // App dirs + flat files: one tree→workdir diff scoped to their pathspecs.
    // Skip entirely when there are none, since an empty pathspec set would diff
    // the whole tree.
    if !app_dirs.is_empty() || !flat_files.is_empty() {
        let mut opts = git2::DiffOptions::new();
        opts.include_untracked(true).recurse_untracked_dirs(true);
        for dir in &app_dirs {
            opts.pathspec(format!("data/{}", dir));
        }
        for f in &flat_files {
            opts.pathspec(format!("data/{}", f));
        }
        if let Ok(diff) = repo.diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts)) {
            for delta in diff.deltas() {
                for path in [delta.old_file().path(), delta.new_file().path()]
                    .into_iter()
                    .flatten()
                {
                    if let Ok(rel) = path.strip_prefix("data") {
                        if let Some(rel) = rel.to_str() {
                            if !rel.is_empty() {
                                changed.insert(rel.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // Triggers: semantic compare of the shipped vs current definition.
    for tf in &trigger_files {
        let shipped = tree
            .get_path(Path::new(&format!("data/{}", tf)))
            .ok()
            .and_then(|entry| repo.find_blob(entry.id()).ok())
            .map(|blob| blob.content().to_vec())
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| TriggerDefinition::from_toml(&text).ok());
        let Some(shipped) = shipped else {
            // No baseline blob (legacy / not committed) → can't judge; skip.
            continue;
        };
        let current = std::fs::read_to_string(workspace_path.join(DATA_DIR).join(tf))
            .ok()
            .and_then(|text| TriggerDefinition::from_toml(&text).ok());
        match current {
            Some(current) if trigger_functional_eq(&shipped, &current) => {}
            // Differs, or the projection is gone (trigger removed) → modified.
            _ => {
                changed.insert(tf.clone());
            }
        }
    }

    ModificationStatus {
        modified: !changed.is_empty(),
        modified_paths: changed.into_iter().collect(),
    }
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

pub(crate) async fn fetch_remote_manifest(
    workspace_path: &Path,
    source_str: &str,
) -> Result<PluginManifest, String> {
    let source = detect_source(source_str)?;
    let (_scratch, plugin_root, _source_type) = fetch_source(workspace_path, &source)?;
    let manifest_path = plugin_root.join("manifest.toml");
    let text =
        std::fs::read_to_string(&manifest_path).map_err(|e| format!("read manifest: {}", e))?;
    plugins::parse_manifest(&text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::installed_plugin_summaries;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use uuid::Uuid;

    /// A `PluginInstalled` projection must carry the plugin's `files` and the
    /// derived `content` kinds so the Plugins → Installed row can list what was
    /// shipped and link to it — even for a plugin with no app.
    #[tokio::test]
    async fn installed_summary_carries_content_and_files() {
        let (pool, db_name) = setup_test_db().await;
        let payload = serde_json::json!({
            "data": {
                "manifest": { "manifest": { "id": "demo", "name": "Demo", "version": "1.2.0" } },
                "files": [
                    "apps/demo/index.html",
                    "knowhow/demo-notes.md",
                    "triggers/demo-watch/trigger.toml",
                ],
            }
        });
        sqlx::query(
            "INSERT INTO events (id, event_type, aggregate, aggregate_id, payload) \
             VALUES ($1, 'PluginInstalled', 'plugin', 'demo', $2)",
        )
        .bind(Uuid::new_v4())
        .bind(&payload)
        .execute(&pool)
        .await
        .expect("seed PluginInstalled");

        let summaries = installed_plugin_summaries(&pool, std::path::Path::new("."))
            .await
            .expect("summaries");
        let demo = summaries
            .iter()
            .find(|s| s.id == "demo")
            .expect("demo plugin present");
        // content_dirs_from_files dedups + sorts (BTreeSet) → alphabetical.
        assert_eq!(demo.content, vec!["apps", "knowhow", "triggers"]);
        assert_eq!(demo.files.len(), 3);
        assert_eq!(demo.app_id.as_deref(), Some("demo"));

        teardown_test_db(&db_name).await;
    }

    /// A non-app plugin (knowhow/triggers only) still projects with its content
    /// and files, and `app_id` is `None` — it must be manageable from the
    /// Plugins → Installed tab without an app.
    #[tokio::test]
    async fn installed_summary_non_app_plugin_has_no_app_id() {
        let (pool, db_name) = setup_test_db().await;
        let payload = serde_json::json!({
            "data": {
                "manifest": { "manifest": { "id": "notes-only", "name": "Notes", "version": "0.1.0" } },
                "files": ["knowhow/a.md", "knowhow/b.md"],
            }
        });
        sqlx::query(
            "INSERT INTO events (id, event_type, aggregate, aggregate_id, payload) \
             VALUES ($1, 'PluginInstalled', 'plugin', 'notes-only', $2)",
        )
        .bind(Uuid::new_v4())
        .bind(&payload)
        .execute(&pool)
        .await
        .expect("seed PluginInstalled");

        let summaries = installed_plugin_summaries(&pool, std::path::Path::new("."))
            .await
            .expect("summaries");
        let notes = summaries
            .iter()
            .find(|s| s.id == "notes-only")
            .expect("notes-only plugin present");
        assert_eq!(notes.content, vec!["knowhow"]);
        assert_eq!(notes.files.len(), 2);
        assert!(notes.app_id.is_none());

        teardown_test_db(&db_name).await;
    }
}

/// Tests for [`plugin_modification_status`] — pure over git + disk, no DB.
#[cfg(test)]
mod modification_status_tests {
    use super::{plugin_modification_status, InstalledRecord};
    use crate::triggers::config::TriggerRun;
    use crate::triggers::definition::TriggerDefinition;
    use std::path::{Path, PathBuf};

    fn temp_workspace() -> PathBuf {
        let p = std::env::temp_dir().join(format!("lucidos_modstatus_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(p.join("data")).unwrap();
        git2::Repository::init(&p).unwrap();
        p
    }

    fn write_data(ws: &Path, rel: &str, content: &str) {
        let p = ws.join("data").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    /// Stage shipped files on disk and make the install commit; return its sha.
    fn install_commit(ws: &Path, files: &[&str]) -> String {
        let rels: Vec<String> = files.iter().map(|s| s.to_string()).collect();
        crate::core::commit_data_paths_added(ws, &rels, "Install plugin: demo v1.0.0").unwrap()
    }

    /// An `InstalledRecord` whose payload carries `commit` (when `Some`) + `files`,
    /// mirroring the real `PluginInstalled` payload shape `InstalledRecord` reads.
    fn record(commit: Option<&str>, files: &[&str]) -> InstalledRecord {
        let mut manifest = serde_json::json!({
            "manifest": { "id": "demo", "name": "Demo", "version": "1.0.0" },
        });
        if let Some(c) = commit {
            manifest
                .as_object_mut()
                .unwrap()
                .insert("commit".into(), serde_json::json!(c));
        }
        InstalledRecord {
            payload: serde_json::json!({ "data": { "manifest": manifest, "files": files } }),
        }
    }

    const TRIGGER_TOML: &str = "name = \"Watcher\"\n\n[[on]]\nevent_type = \"FooHappened\"\n\n[run]\ntype = \"intent\"\nintent = \"do the thing\"\n";

    #[test]
    fn clean_install_is_not_modified() {
        let ws = temp_workspace();
        write_data(&ws, "apps/demo/index.html", "<h1>hi</h1>");
        write_data(&ws, "knowhow/demo.md", "notes");
        let sha = install_commit(&ws, &["apps/demo/index.html", "knowhow/demo.md"]);
        let rec = record(Some(&sha), &["apps/demo/index.html", "knowhow/demo.md"]);
        let status = plugin_modification_status(&ws, &rec);
        assert!(
            !status.modified,
            "a freshly installed plugin must not be modified: {:?}",
            status.modified_paths
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn editing_an_app_file_marks_modified() {
        let ws = temp_workspace();
        write_data(&ws, "apps/demo/index.html", "<h1>hi</h1>");
        let sha = install_commit(&ws, &["apps/demo/index.html"]);
        write_data(&ws, "apps/demo/index.html", "<h1>EDITED</h1>");
        let status =
            plugin_modification_status(&ws, &record(Some(&sha), &["apps/demo/index.html"]));
        assert!(status.modified);
        assert_eq!(
            status.modified_paths,
            vec!["apps/demo/index.html".to_string()]
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn adding_a_file_in_app_dir_marks_modified() {
        let ws = temp_workspace();
        write_data(&ws, "apps/demo/index.html", "x");
        let sha = install_commit(&ws, &["apps/demo/index.html"]);
        // User adds a new (untracked) file inside the plugin's app dir.
        write_data(&ws, "apps/demo/extra.js", "console.log(1)");
        let status =
            plugin_modification_status(&ws, &record(Some(&sha), &["apps/demo/index.html"]));
        assert!(
            status.modified,
            "an added file in the plugin app dir must count"
        );
        assert!(status
            .modified_paths
            .iter()
            .any(|p| p == "apps/demo/extra.js"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn unrelated_new_file_in_shared_root_does_not_mark() {
        let ws = temp_workspace();
        write_data(&ws, "knowhow/demo.md", "notes");
        let sha = install_commit(&ws, &["knowhow/demo.md"]);
        // A brand-new knowhow file the plugin never shipped → not attributable.
        write_data(&ws, "knowhow/my-own.md", "mine");
        let status = plugin_modification_status(&ws, &record(Some(&sha), &["knowhow/demo.md"]));
        assert!(
            !status.modified,
            "an unrelated new knowhow file must not flag the plugin: {:?}",
            status.modified_paths
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn editing_recorded_knowhow_marks_modified() {
        let ws = temp_workspace();
        write_data(&ws, "knowhow/demo.md", "notes");
        let sha = install_commit(&ws, &["knowhow/demo.md"]);
        write_data(&ws, "knowhow/demo.md", "EDITED notes");
        let status = plugin_modification_status(&ws, &record(Some(&sha), &["knowhow/demo.md"]));
        assert!(status.modified);
        assert_eq!(status.modified_paths, vec!["knowhow/demo.md".to_string()]);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn reverting_an_edit_self_heals() {
        let ws = temp_workspace();
        write_data(&ws, "knowhow/demo.md", "notes");
        let sha = install_commit(&ws, &["knowhow/demo.md"]);
        write_data(&ws, "knowhow/demo.md", "EDITED");
        write_data(&ws, "knowhow/demo.md", "notes"); // revert to installed content
        assert!(
            !plugin_modification_status(&ws, &record(Some(&sha), &["knowhow/demo.md"])).modified
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn legacy_record_without_commit_is_not_modified() {
        let ws = temp_workspace();
        write_data(&ws, "knowhow/demo.md", "notes");
        install_commit(&ws, &["knowhow/demo.md"]);
        write_data(&ws, "knowhow/demo.md", "EDITED"); // even with an edit on disk...
                                                      // ...a record with no baseline commit cannot be judged → not modified.
        assert!(!plugin_modification_status(&ws, &record(None, &["knowhow/demo.md"])).modified);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn trigger_reserialized_projection_is_not_modified() {
        // The shipped authored trigger.toml is committed; the on-disk projection
        // is re-serialized (different bytes) AND carries the registration stamps
        // (slug + plugin_id). A byte diff would false-positive; the semantic
        // compare (ignoring slug/plugin_id/group_id) must read as not-modified.
        let ws = temp_workspace();
        write_data(&ws, "triggers/watch/trigger.toml", TRIGGER_TOML);
        let sha = install_commit(&ws, &["triggers/watch/trigger.toml"]);
        let mut def = TriggerDefinition::from_toml(TRIGGER_TOML).unwrap();
        def.slug = "watch".to_string();
        def.plugin_id = Some("demo".to_string());
        let reserialized = def.to_toml().unwrap();
        assert_ne!(reserialized, TRIGGER_TOML, "test premise: bytes differ");
        write_data(&ws, "triggers/watch/trigger.toml", &reserialized);
        let status =
            plugin_modification_status(&ws, &record(Some(&sha), &["triggers/watch/trigger.toml"]));
        assert!(
            !status.modified,
            "a re-serialized-but-equivalent trigger must not flag: {:?}",
            status.modified_paths
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn editing_trigger_definition_marks_modified() {
        let ws = temp_workspace();
        write_data(&ws, "triggers/watch/trigger.toml", TRIGGER_TOML);
        let sha = install_commit(&ws, &["triggers/watch/trigger.toml"]);
        let mut def = TriggerDefinition::from_toml(TRIGGER_TOML).unwrap();
        def.run = TriggerRun::Intent {
            intent: "do something ELSE".to_string(),
        };
        write_data(&ws, "triggers/watch/trigger.toml", &def.to_toml().unwrap());
        let status =
            plugin_modification_status(&ws, &record(Some(&sha), &["triggers/watch/trigger.toml"]));
        assert!(
            status.modified,
            "an edited trigger intent must flag modified"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }
}
