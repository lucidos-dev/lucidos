use std::collections::BTreeSet;
use std::path::Path;

use crate::core::git_auth::GitCredentials;
use crate::core::plugin_marketplaces::InstalledPluginSummary;
use crate::core::plugin_media::{self, MediaSource, PluginMedia};
use crate::core::plugins::{self, compare_versions, PluginManifest, UpdateDecision};
use crate::core::DATA_DIR;
use crate::engine::release_notices::running_release;
use crate::triggers::definition::TriggerDefinition;

use super::source::{credentials_for_source, detect_source, fetch_source};

fn install_canonical_id(payload: &serde_json::Value, aggregate_id: &str) -> String {
    payload
        .pointer("/data/manifest/manifest/id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(aggregate_id)
        .to_string()
}

fn uninstall_canonical_id(payload: &serde_json::Value, aggregate_id: &str) -> String {
    payload
        .pointer("/data/id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(aggregate_id)
        .to_string()
}

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
                engine_requirement: rec.manifest().and_then(plugins::engine_requirement_of),
                media: installed_media(workspace_path, &id),
            }
        })
        .collect())
}

fn installed_media(workspace_path: &Path, plugin_id: &str) -> PluginMedia {
    let copy = plugin_media::installed_dir(workspace_path, plugin_id);
    match plugin_media::load_index(&copy) {
        Ok(Some(index)) => MediaSource::Installed { plugin_id }.media(&index),
        Ok(None) => PluginMedia::default(),
        Err(e) => {
            log_legacy!("[Plugins] could not read the media of '{plugin_id}': {e}");
            PluginMedia {
                problems: vec![plugin_media::MediaProblem {
                    path: plugin_media::MEDIA_DIR.to_string(),
                    reason: format!("could not be read: {e}"),
                }],
                ..PluginMedia::default()
            }
        }
    }
}

pub(crate) async fn event_hook_plugin_ids(
    pool: &sqlx::PgPool,
) -> Result<std::collections::BTreeMap<String, String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut hooks = std::collections::BTreeMap::new();
    for (id, payload) in project_installs(pool).await? {
        for hook in (InstalledRecord { payload }).hooks()? {
            hooks.insert(hook.slug, id.clone());
        }
    }
    Ok(hooks)
}

pub(crate) async fn latest_install(
    pool: &sqlx::PgPool,
    id: &str,
) -> Result<Option<InstalledRecord>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(project_installs(pool)
        .await?
        .remove(id)
        .map(|payload| InstalledRecord { payload }))
}

#[derive(Debug, Clone)]
pub(crate) struct PluginBaseline {
    pub(crate) commit: String,
    pub(crate) files: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct InstalledPlugins {
    pub(crate) files: std::collections::BTreeMap<String, Vec<String>>,
    pub(crate) baselines: std::collections::BTreeMap<String, PluginBaseline>,
    pub(crate) setups: std::collections::BTreeMap<String, PriorSetup>,
    pub(crate) hooks:
        std::collections::BTreeMap<String, Vec<crate::engine::pending_plugins::PluginHook>>,
}

impl InstalledPlugins {
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.files.contains_key(id)
    }

    pub(crate) fn recorded_by_others(&self, id: &str) -> BTreeSet<&str> {
        self.files
            .iter()
            .filter(|(other, _)| other.as_str() != id)
            .flat_map(|(_, files)| files.iter().map(String::as_str))
            .collect()
    }
}

pub(crate) async fn project_installed_plugins(
    pool: &sqlx::PgPool,
) -> Result<InstalledPlugins, Box<dyn std::error::Error + Send + Sync>> {
    let mut installed = InstalledPlugins::default();
    for (id, payload) in project_installs(pool).await? {
        let rec = InstalledRecord { payload };
        let files = rec.files();
        if let Some(commit) = rec.commit() {
            let baseline = PluginBaseline {
                commit: commit.to_string(),
                files: files.clone(),
            };
            installed.baselines.insert(id.clone(), baseline);
        }
        let hooks = rec.hooks()?;
        if !hooks.is_empty() {
            installed.hooks.insert(id.clone(), hooks);
        }
        installed.setups.insert(id.clone(), rec.prior_setup());
        installed.files.insert(id, files);
    }
    Ok(installed)
}

pub(crate) struct InstalledRecord {
    payload: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PriorSetup {
    pub(crate) text: Option<String>,
    pub(crate) revision: u64,
    pub(crate) version: Option<String>,
}

impl InstalledRecord {
    pub(super) fn hooks(
        &self,
    ) -> Result<Vec<crate::engine::pending_plugins::PluginHook>, serde_json::Error> {
        self.payload
            .pointer("/data/manifest/hooks")
            .map(|hooks| serde_json::from_value(hooks.clone()))
            .transpose()
            .map(Option::unwrap_or_default)
    }

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
    pub(crate) fn manifest(&self) -> Option<&serde_json::Value> {
        self.payload.pointer("/data/manifest/manifest")
    }
    pub(crate) fn files(&self) -> Vec<String> {
        self.payload
            .pointer("/data/files")
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    }
    pub(crate) fn setup_thread_id(&self) -> Option<&str> {
        self.payload
            .pointer("/data/manifest/setup_thread_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
    }
    pub(crate) fn setup_revision(&self) -> u64 {
        self.payload
            .pointer("/data/manifest/manifest/setup_revision")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    }
    pub(crate) fn prior_setup(&self) -> PriorSetup {
        PriorSetup {
            text: crate::engine::pending_plugins::normalize_setup(
                self.payload
                    .pointer("/data/manifest/manifest/setup")
                    .and_then(|v| v.as_str()),
            ),
            revision: self.setup_revision(),
            version: self.version().map(str::to_string),
        }
    }
    pub(crate) fn commit(&self) -> Option<&str> {
        self.payload
            .pointer("/data/manifest/commit")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
    }
}

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

pub(crate) fn normalize_plugin_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = true;
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

#[derive(Debug, Clone)]
struct InstalledIndex {
    plugin_id: String,
    plugin_name: String,
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

pub(crate) fn not_installed_error(query: &str) -> String {
    format!(
        "Error: plugin '{}' is not currently installed (no PluginInstalled event, or already uninstalled)",
        query
    )
}

pub(crate) async fn resolve_plugin_query(
    pool: &sqlx::PgPool,
    query: &str,
) -> Result<String, String> {
    if query.is_empty() {
        return Err("Error: plugin id/name is required".to_string());
    }

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
        return Err(not_installed_error(query));
    }

    let matches: Vec<&InstalledIndex> = installed
        .iter()
        .filter(|p| p.aliases.contains(&needle))
        .collect();

    match matches.as_slice() {
        [] => Err(not_installed_error(query)),
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

#[derive(Debug, Clone)]
pub(crate) struct PluginOwner {
    pub(crate) plugin_id: String,
    pub(crate) plugin_name: String,
}

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

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ModificationStatus {
    pub(crate) modified: bool,
    pub(crate) modified_paths: Vec<String>,
}

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

pub(crate) fn plugin_modification_status(
    workspace_path: &Path,
    record: &InstalledRecord,
) -> ModificationStatus {
    let Some(commit_sha) = record.commit() else {
        return ModificationStatus::default();
    };
    modification_status_for(workspace_path, commit_sha, &record.files())
}

pub(crate) fn modification_status_for(
    workspace_path: &Path,
    commit_sha: &str,
    files: &[String],
) -> ModificationStatus {
    let unmodified = ModificationStatus::default();

    let mut app_dirs: BTreeSet<String> = BTreeSet::new();
    let mut flat_files: Vec<String> = Vec::new();
    let mut trigger_files: Vec<String> = Vec::new();
    for f in files {
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
                            if !rel.is_empty() && !crate::core::is_build_output_path(rel) {
                                changed.insert(rel.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    for tf in &trigger_files {
        let shipped = tree
            .get_path(Path::new(&format!("data/{}", tf)))
            .ok()
            .and_then(|entry| repo.find_blob(entry.id()).ok())
            .map(|blob| blob.content().to_vec())
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| TriggerDefinition::from_toml(&text).ok());
        let Some(shipped) = shipped else {
            continue;
        };
        let current = std::fs::read_to_string(workspace_path.join(DATA_DIR).join(tf))
            .ok()
            .and_then(|text| TriggerDefinition::from_toml(&text).ok());
        match current {
            Some(current) if trigger_functional_eq(&shipped, &current) => {}
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
        report.push(check_one(workspace_path, pool, &id, rec).await);
    }

    serde_json::to_string_pretty(&report)
        .unwrap_or_else(|e| format!("Error: serialize report: {}", e))
}

fn all_recorded_files_missing(data_dir: &Path, files: &[String]) -> bool {
    if files.is_empty() {
        return false;
    }
    files.iter().all(|rel| !data_dir.join(rel).exists())
}

async fn check_one(
    workspace_path: &Path,
    pool: &sqlx::PgPool,
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

    match fetch_remote_manifest(workspace_path, pool, &source).await {
        Ok(remote) => {
            let changed =
                compare_versions(&installed_version, &remote.version) == UpdateDecision::Update;
            let mismatch = plugins::check_engine_requirement(&remote.raw, &running_release()).err();
            serde_json::json!({
                "id": id,
                "installed_version": installed_version,
                "latest_version": remote.version,
                "changed": changed,
                "engine_requirement": plugins::engine_requirement_of(&remote.raw),
                "engine_compatible": mismatch.is_none(),
                "engine_incompatible_reason": mismatch
                    .map(|m| m.refusal(&remote.name, &remote.version)),
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

/// INVARIANT: Fetch a plugin source and read its `manifest.toml`.
///
/// Runs on the blocking pool, like the install path's
/// [`super::stage_install_request`]: the body contains no `.await` at all, and
/// `fetch_source` performs a SYNCHRONOUS `git2` clone over the network with no
/// fetch timeout. Called directly on a tokio worker it pins that worker for as
/// long as the remote takes to answer, which for an unreachable host is
/// unbounded, and the blocking syscall cannot be freed by dropping the future,
/// so a user cancel does not release it either. `check_plugin_updates` with no
/// `id` clones EVERY installed plugin in sequence, so one bad remote parks a
/// worker for the rest of the sweep.
pub(crate) async fn fetch_remote_manifest(
    workspace_path: &Path,
    pool: &sqlx::PgPool,
    source_str: &str,
) -> Result<PluginManifest, String> {
    let workspace_path = workspace_path.to_path_buf();
    let credentials = credentials_for_source(pool, source_str).await;
    let source_str = source_str.to_string();
    tokio::task::spawn_blocking(move || {
        fetch_remote_manifest_blocking(&workspace_path, &source_str, &credentials)
    })
    .await
    .map_err(|e| format!("manifest fetch task panicked: {}", e))?
}

/// INVARIANT: The synchronous body of [`fetch_remote_manifest`]. Never call this from an
/// async context directly.
fn fetch_remote_manifest_blocking(
    workspace_path: &Path,
    source_str: &str,
    credentials: &GitCredentials,
) -> Result<PluginManifest, String> {
    let source = detect_source(source_str)?;
    let (_scratch, plugin_root, _source_type) = fetch_source(workspace_path, &source, credentials)?;
    plugins::read_manifest(&plugin_root).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::installed_plugin_summaries;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use uuid::Uuid;

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
        assert_eq!(demo.content, vec!["apps", "knowhow", "triggers"]);
        assert_eq!(demo.files.len(), 3);
        assert_eq!(demo.app_id.as_deref(), Some("demo"));

        teardown_test_db(&db_name).await;
    }

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

#[cfg(test)]
mod modification_status_tests {
    use super::{plugin_modification_status, InstalledRecord, PriorSetup};
    use crate::triggers::config::TriggerRun;
    use crate::triggers::definition::TriggerDefinition;
    use std::path::{Path, PathBuf};

    fn temp_workspace() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().to_path_buf();
        std::fs::create_dir_all(p.join("data")).unwrap();
        git2::Repository::init(&p).unwrap();
        (tmp, p)
    }

    fn write_data(ws: &Path, rel: &str, content: &str) {
        let p = ws.join("data").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn install_commit(ws: &Path, files: &[&str]) -> String {
        let rels: Vec<String> = files.iter().map(|s| s.to_string()).collect();
        crate::core::commit_data_paths_added(ws, &rels, "Install plugin: demo v1.0.0").unwrap()
    }

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

    fn record_with_manifest(manifest: serde_json::Value) -> InstalledRecord {
        InstalledRecord {
            payload: serde_json::json!({
                "data": { "manifest": { "manifest": manifest }, "files": [] }
            }),
        }
    }

    #[test]
    fn prior_setup_reads_text_revision_and_version_from_the_raw_manifest() {
        let rec = record_with_manifest(serde_json::json!({
            "version": "0.2.0",
            "setup": "  copy the skill  ",
            "setup_revision": 3,
        }));
        assert_eq!(
            rec.prior_setup(),
            PriorSetup {
                text: Some("copy the skill".into()),
                revision: 3,
                version: Some("0.2.0".into()),
            }
        );
    }

    #[test]
    fn a_record_with_no_revision_counts_as_revision_zero() {
        let rec = record_with_manifest(serde_json::json!({ "version": "0.1.0" }));
        assert_eq!(rec.setup_revision(), 0);
        assert_eq!(rec.prior_setup().text, None);
    }

    const TRIGGER_TOML: &str = "name = \"Watcher\"\n\n[[on]]\nevent_type = \"FooHappened\"\n\n[run]\ntype = \"intent\"\nintent = \"do the thing\"\n";

    #[test]
    fn clean_install_is_not_modified() {
        let (_tmp, ws) = temp_workspace();
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
    }

    #[test]
    fn editing_an_app_file_marks_modified() {
        let (_tmp, ws) = temp_workspace();
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
    }

    #[test]
    fn adding_a_file_in_app_dir_marks_modified() {
        let (_tmp, ws) = temp_workspace();
        write_data(&ws, "apps/demo/index.html", "x");
        let sha = install_commit(&ws, &["apps/demo/index.html"]);
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
    }

    #[test]
    fn build_output_in_app_dir_is_not_modified() {
        let (_tmp, ws) = temp_workspace();
        write_data(&ws, "apps/demo/index.html", "x");
        let sha = install_commit(&ws, &["apps/demo/index.html"]);
        write_data(&ws, "apps/demo/__pycache__/run.cpython-314.pyc", "bytecode");
        write_data(&ws, "apps/demo/node_modules/dep/index.js", "dep");
        write_data(&ws, "apps/demo/helper.pyc", "bytecode");
        let status =
            plugin_modification_status(&ws, &record(Some(&sha), &["apps/demo/index.html"]));
        assert!(
            !status.modified,
            "build output is not a local edit: {:?}",
            status.modified_paths
        );
    }

    #[test]
    fn unrelated_new_file_in_shared_root_does_not_mark() {
        let (_tmp, ws) = temp_workspace();
        write_data(&ws, "knowhow/demo.md", "notes");
        let sha = install_commit(&ws, &["knowhow/demo.md"]);
        write_data(&ws, "knowhow/my-own.md", "mine");
        let status = plugin_modification_status(&ws, &record(Some(&sha), &["knowhow/demo.md"]));
        assert!(
            !status.modified,
            "an unrelated new knowhow file must not flag the plugin: {:?}",
            status.modified_paths
        );
    }

    #[test]
    fn editing_recorded_knowhow_marks_modified() {
        let (_tmp, ws) = temp_workspace();
        write_data(&ws, "knowhow/demo.md", "notes");
        let sha = install_commit(&ws, &["knowhow/demo.md"]);
        write_data(&ws, "knowhow/demo.md", "EDITED notes");
        let status = plugin_modification_status(&ws, &record(Some(&sha), &["knowhow/demo.md"]));
        assert!(status.modified);
        assert_eq!(status.modified_paths, vec!["knowhow/demo.md".to_string()]);
    }

    #[test]
    fn reverting_an_edit_self_heals() {
        let (_tmp, ws) = temp_workspace();
        write_data(&ws, "knowhow/demo.md", "notes");
        let sha = install_commit(&ws, &["knowhow/demo.md"]);
        write_data(&ws, "knowhow/demo.md", "EDITED");
        write_data(&ws, "knowhow/demo.md", "notes");
        assert!(
            !plugin_modification_status(&ws, &record(Some(&sha), &["knowhow/demo.md"])).modified
        );
    }

    #[test]
    fn legacy_record_without_commit_is_not_modified() {
        let (_tmp, ws) = temp_workspace();
        write_data(&ws, "knowhow/demo.md", "notes");
        install_commit(&ws, &["knowhow/demo.md"]);
        write_data(&ws, "knowhow/demo.md", "EDITED");
        assert!(!plugin_modification_status(&ws, &record(None, &["knowhow/demo.md"])).modified);
    }

    #[test]
    fn trigger_reserialized_projection_is_not_modified() {
        let (_tmp, ws) = temp_workspace();
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
    }

    #[test]
    fn editing_trigger_definition_marks_modified() {
        let (_tmp, ws) = temp_workspace();
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
    }
}
