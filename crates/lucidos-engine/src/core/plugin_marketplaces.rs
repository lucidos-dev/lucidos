use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::core::plugins::{self, compare_versions, validate_tree, UpdateDecision};
use crate::core::DATA_DIR;

pub const MARKETPLACES_DATA_PATH: &str = "config/plugin-marketplaces.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginMarketplace {
    pub id: String,
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PluginMarketplaceRegistry {
    #[serde(default)]
    pub marketplaces: Vec<PluginMarketplace>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InstalledPluginSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The Lucidos Agent setup thread spawned at install time, when the plugin
    /// shipped `setup` instructions. Recorded in the `PluginInstalled` payload;
    /// drives the App Store card's Setup→Open button across reloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_thread_id: Option<String>,
    /// The plugin's primary app (`data/apps/<id>/`), if it installs one. The
    /// card's "Open" button launches it; plugins with no app have `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketplacePluginStatus {
    Available,
    Installed,
    UpdateAvailable,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketplacePlugin {
    pub marketplace_id: String,
    pub marketplace_name: String,
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub source: String,
    pub manifest: serde_json::Value,
    pub content: Vec<String>,
    pub files_count: usize,
    pub status: MarketplacePluginStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    /// Setup thread spawned for this plugin at install (installed plugins that
    /// shipped a `setup` field only). The card's "Setup" button opens it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_thread_id: Option<String>,
    /// True once the setup thread has finished (no longer running or waiting on
    /// the user) — flips the card button from "Setup" to "Open". Filled by the
    /// catalog handler from thread state; always `false` for available plugins
    /// and for installs that spawned no setup thread.
    pub setup_complete: bool,
    /// The plugin's primary app (`data/apps/<id>/`), if it ships one. The card's
    /// "Open" button launches it; `None` → nothing to open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketplaceScanError {
    pub marketplace_id: String,
    pub marketplace_name: String,
    pub source: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketplaceCatalog {
    pub marketplaces: Vec<PluginMarketplace>,
    pub plugins: Vec<MarketplacePlugin>,
    pub errors: Vec<MarketplaceScanError>,
}

#[derive(Debug, Clone)]
struct MarketplaceSource {
    clone_url: String,
    github_owner: Option<String>,
    github_repo: Option<String>,
    branch: Option<String>,
    subpath: Option<String>,
}

pub fn load_registry(
    workspace_path: &Path,
) -> Result<PluginMarketplaceRegistry, Box<dyn std::error::Error + Send + Sync>> {
    let path = workspace_path.join(DATA_DIR).join(MARKETPLACES_DATA_PATH);
    if !path.exists() {
        return Ok(PluginMarketplaceRegistry::default());
    }
    let text = std::fs::read_to_string(&path)?;
    let mut registry: PluginMarketplaceRegistry = serde_json::from_str(&text)?;
    registry.marketplaces.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(registry)
}

pub fn save_registry(
    workspace_path: &Path,
    registry: &PluginMarketplaceRegistry,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let path = workspace_path.join(DATA_DIR).join(MARKETPLACES_DATA_PATH);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(registry)?;
    std::fs::write(&path, format!("{text}\n"))?;
    Ok(path)
}

pub fn add_marketplace(
    registry: &mut PluginMarketplaceRegistry,
    source: &str,
    name: Option<&str>,
) -> Result<(PluginMarketplace, bool), String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("source is required".to_string());
    }
    let parsed = parse_marketplace_source(source)?;

    let id = marketplace_id(&parsed, source);
    let name = name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| display_name_for_source(source));

    if let Some(existing) = registry.marketplaces.iter_mut().find(|m| m.id == id) {
        existing.name = name;
        existing.source = source.to_string();
        return Ok((existing.clone(), false));
    }

    let marketplace = PluginMarketplace {
        id,
        name,
        source: source.to_string(),
    };
    registry.marketplaces.push(marketplace.clone());
    registry.marketplaces.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((marketplace, true))
}

pub fn remove_marketplace(registry: &mut PluginMarketplaceRegistry, id: &str) -> bool {
    let before = registry.marketplaces.len();
    registry.marketplaces.retain(|m| m.id != id);
    before != registry.marketplaces.len()
}

pub fn scan_catalog(
    workspace_path: &Path,
    registry: &PluginMarketplaceRegistry,
    installed: &[InstalledPluginSummary],
) -> MarketplaceCatalog {
    let installed_by_id: BTreeMap<String, InstalledPluginSummary> = installed
        .iter()
        .cloned()
        .map(|p| (p.id.clone(), p))
        .collect();

    let mut plugins = Vec::new();
    let mut errors = Vec::new();

    for marketplace in &registry.marketplaces {
        match scan_one_marketplace(workspace_path, marketplace, &installed_by_id) {
            Ok(mut found) => plugins.append(&mut found),
            Err(error) => errors.push(MarketplaceScanError {
                marketplace_id: marketplace.id.clone(),
                marketplace_name: marketplace.name.clone(),
                source: marketplace.source.clone(),
                error,
            }),
        }
    }

    plugins.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then(a.id.cmp(&b.id))
            .then(a.marketplace_name.cmp(&b.marketplace_name))
    });

    MarketplaceCatalog {
        marketplaces: registry.marketplaces.clone(),
        plugins,
        errors,
    }
}

pub fn update_candidates(catalog: &MarketplaceCatalog) -> Vec<MarketplacePlugin> {
    let mut by_plugin_id: BTreeMap<String, MarketplacePlugin> = BTreeMap::new();

    for plugin in catalog
        .plugins
        .iter()
        .filter(|p| p.status == MarketplacePluginStatus::UpdateAvailable)
    {
        match by_plugin_id.get(&plugin.id) {
            Some(existing)
                if compare_versions(&existing.version, &plugin.version)
                    != UpdateDecision::Update => {}
            _ => {
                by_plugin_id.insert(plugin.id.clone(), plugin.clone());
            }
        }
    }

    by_plugin_id.into_values().collect()
}

fn scan_one_marketplace(
    workspace_path: &Path,
    marketplace: &PluginMarketplace,
    installed_by_id: &BTreeMap<String, InstalledPluginSummary>,
) -> Result<Vec<MarketplacePlugin>, String> {
    let parsed = parse_marketplace_source(&marketplace.source)?;
    let (scratch, root, actual_branch) = clone_marketplace(workspace_path, &parsed)?;
    let manifest_roots = find_manifest_roots(&root)?;
    if manifest_roots.is_empty() {
        drop(scratch);
        return Err("no plugin manifest.toml files found".to_string());
    }

    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for plugin_root in manifest_roots {
        let rel = plugin_root
            .strip_prefix(&root)
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();

        let (manifest, planned) = match validate_tree(&plugin_root) {
            Ok(ok) => ok,
            Err(e) => {
                log!(
                    "[PluginMarketplaces] Skipping invalid plugin in {} at {}: {}",
                    marketplace.source,
                    rel,
                    e
                );
                continue;
            }
        };

        if !seen.insert(manifest.id.clone()) {
            log!(
                "[PluginMarketplaces] Skipping duplicate plugin id '{}' in {}",
                manifest.id,
                marketplace.source
            );
            continue;
        }

        let source = install_source(&parsed, &actual_branch, &rel)
            .ok_or_else(|| format!("cannot construct install source for {}", rel))?;
        let content = content_dirs_from_files(planned.iter().map(|p| p.data_relative.as_str()));
        let installed = installed_by_id.get(&manifest.id);
        let status = match installed {
            Some(inst)
                if compare_versions(&inst.version, &manifest.version) == UpdateDecision::Update =>
            {
                MarketplacePluginStatus::UpdateAvailable
            }
            Some(_) => MarketplacePluginStatus::Installed,
            None => MarketplacePluginStatus::Available,
        };

        out.push(MarketplacePlugin {
            marketplace_id: marketplace.id.clone(),
            marketplace_name: marketplace.name.clone(),
            id: manifest.id,
            name: manifest.name,
            description: manifest.description,
            version: manifest.version,
            source,
            manifest: manifest.raw,
            content,
            files_count: planned.len(),
            status,
            installed_version: installed.map(|p| p.version.clone()),
            setup_thread_id: installed.and_then(|p| p.setup_thread_id.clone()),
            // Completion is a thread-state lookup the pure scan can't do; the
            // catalog handler fills it in after the scan returns.
            setup_complete: false,
            app_id: installed.and_then(|p| p.app_id.clone()),
        });
    }

    if out.is_empty() {
        drop(scratch);
        return Err("no valid plugins found".to_string());
    }

    drop(scratch);
    Ok(out)
}

fn parse_marketplace_source(source: &str) -> Result<MarketplaceSource, String> {
    let trimmed = source.trim();
    if trimmed.ends_with(plugins::PLUGIN_ARCHIVE_EXT) {
        return Err("marketplaces must be git repositories, not .lucidos-plugin archives".into());
    }

    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        if let Some(parsed) = parse_github_tree(rest)? {
            return Ok(parsed);
        }
        if let Some((owner, repo)) = parse_github_path(rest) {
            return Ok(MarketplaceSource {
                clone_url: format!("https://github.com/{owner}/{repo}.git"),
                github_owner: Some(owner),
                github_repo: Some(repo),
                branch: None,
                subpath: None,
            });
        }
    }

    if let Some(rest) = trimmed.strip_prefix("git@github.com:") {
        if let Some((owner, repo)) = parse_github_path(rest) {
            return Ok(MarketplaceSource {
                clone_url: trimmed.to_string(),
                github_owner: Some(owner),
                github_repo: Some(repo),
                branch: None,
                subpath: None,
            });
        }
    }

    if trimmed.starts_with("file://") || plugins::is_git_url(trimmed) {
        return Ok(MarketplaceSource {
            clone_url: trimmed.to_string(),
            github_owner: None,
            github_repo: None,
            branch: None,
            subpath: None,
        });
    }

    Err("expected a git URL or GitHub repository/tree URL".to_string())
}

fn parse_github_tree(rest: &str) -> Result<Option<MarketplaceSource>, String> {
    let parts: Vec<&str> = rest.splitn(5, '/').collect();
    if parts.len() < 4 || parts[2] != "tree" {
        return Ok(None);
    }
    let owner = parts[0].to_string();
    let repo = parts[1].trim_end_matches(".git").to_string();
    let branch = parts[3].to_string();
    if owner.trim().is_empty() || repo.trim().is_empty() || branch.trim().is_empty() {
        return Err("GitHub tree URL must include owner, repo, and branch".to_string());
    }
    let subpath = if parts.len() == 5 && !parts[4].is_empty() {
        normalize_tree_subpath(parts[4])?
    } else {
        None
    };
    Ok(Some(MarketplaceSource {
        clone_url: format!("https://github.com/{owner}/{repo}.git"),
        github_owner: Some(owner),
        github_repo: Some(repo),
        branch: Some(branch),
        subpath,
    }))
}

fn normalize_tree_subpath(raw: &str) -> Result<Option<String>, String> {
    let trimmed = raw.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.contains('\\') || trimmed.split('/').any(|part| {
        part.is_empty() || part == "." || part == ".."
    }) {
        return Err("GitHub tree subpath must stay inside the repository".to_string());
    }
    Ok(Some(trimmed.to_string()))
}

fn parse_github_path(path: &str) -> Option<(String, String)> {
    let mut parts = path.trim_matches('/').split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim().trim_end_matches(".git");
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

fn clone_marketplace(
    workspace_path: &Path,
    source: &MarketplaceSource,
) -> Result<(tempfile::TempDir, PathBuf, Option<String>), String> {
    let parent = workspace_path
        .join(".lucidos")
        .join("tmp")
        .join("plugin-marketplaces");
    std::fs::create_dir_all(&parent).map_err(|e| format!("create scratch dir: {e}"))?;
    let scratch =
        tempfile::TempDir::new_in(&parent).map_err(|e| format!("create scratch dir: {e}"))?;
    let clone_target = scratch.path().join("repo");

    let repo = {
        let mut builder = git2::build::RepoBuilder::new();
        let mut fetch_opts = git2::FetchOptions::new();
        if !source.clone_url.starts_with("file://") {
            fetch_opts.depth(1);
        }
        if let Some(branch) = &source.branch {
            builder.branch(branch);
        }
        builder.fetch_options(fetch_opts);
        builder
            .clone(&source.clone_url, &clone_target)
            .map_err(|e| format!("git clone failed: {e}"))?
    };

    let actual_branch = source.branch.clone().or_else(|| {
        repo.head()
            .ok()
            .and_then(|head| head.shorthand().map(str::to_string))
            .filter(|branch| branch != "HEAD")
    });
    drop(repo);
    let _ = std::fs::remove_dir_all(clone_target.join(".git"));

    let root = match &source.subpath {
        Some(subpath) => clone_target.join(subpath),
        None => clone_target.clone(),
    };
    if !root.is_dir() {
        return Err(format!(
            "subpath not found inside repo: {}",
            source.subpath.as_deref().unwrap_or("")
        ));
    }
    Ok((scratch, root, actual_branch))
}

fn find_manifest_roots(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    collect_manifest_roots(root, root, 0, &mut roots)?;
    roots.sort();
    Ok(roots)
}

fn collect_manifest_roots(
    root: &Path,
    dir: &Path,
    depth: usize,
    roots: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if dir.join("manifest.toml").is_file() {
        roots.push(dir.to_path_buf());
        return Ok(());
    }
    if depth >= 3 {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        let is_content_dir = depth == 0 && plugins::CONTENT_DIRS.contains(&name);
        if path != root && is_content_dir {
            continue;
        }
        collect_manifest_roots(root, &path, depth + 1, roots)?;
    }
    Ok(())
}

fn install_source(
    source: &MarketplaceSource,
    actual_branch: &Option<String>,
    relative_plugin_root: &str,
) -> Option<String> {
    if relative_plugin_root.is_empty() {
        return Some(source_to_registration_url(source));
    }
    let owner = source.github_owner.as_ref()?;
    let repo = source.github_repo.as_ref()?;
    let branch = source.branch.as_ref().or(actual_branch.as_ref())?;
    let mut pieces = Vec::new();
    if let Some(subpath) = &source.subpath {
        if !subpath.is_empty() {
            pieces.push(subpath.trim_matches('/'));
        }
    }
    pieces.push(relative_plugin_root.trim_matches('/'));
    Some(format!(
        "https://github.com/{owner}/{repo}/tree/{branch}/{}",
        pieces.join("/")
    ))
}

fn source_to_registration_url(source: &MarketplaceSource) -> String {
    match (&source.github_owner, &source.github_repo, &source.branch) {
        (Some(owner), Some(repo), Some(branch)) => {
            let suffix = source
                .subpath
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| format!("/{s}"))
                .unwrap_or_default();
            format!("https://github.com/{owner}/{repo}/tree/{branch}{suffix}")
        }
        _ => source.clone_url.clone(),
    }
}

fn content_dirs_from_files<'a>(files: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut out = BTreeSet::new();
    for file in files {
        if let Some(first) = file.split('/').next() {
            if plugins::CONTENT_DIRS.contains(&first) {
                out.insert(first.to_string());
            }
        }
    }
    out.into_iter().collect()
}

fn marketplace_id(source: &MarketplaceSource, raw_source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical_source_key(source).as_bytes());
    let hash = hasher.finalize();
    let short = hash[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("{}-{}", slug_for_source(raw_source), short)
}

fn canonical_source_key(source: &MarketplaceSource) -> String {
    let branch = source.branch.as_deref().unwrap_or("");
    let subpath = source.subpath.as_deref().unwrap_or("");
    match (&source.github_owner, &source.github_repo) {
        (Some(owner), Some(repo)) => format!(
            "github:{}/{}#{branch}:{}",
            owner.to_ascii_lowercase(),
            repo.to_ascii_lowercase(),
            subpath.trim_matches('/')
        ),
        _ => format!(
            "git:{}#{branch}:{}",
            source.clone_url.trim_end_matches('/'),
            subpath.trim_matches('/')
        ),
    }
}

fn slug_for_source(source: &str) -> String {
    let trimmed = source.trim();
    let label = display_name_for_source(trimmed);
    let mut out = String::new();
    let mut prev_dash = true;
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "marketplace".to_string()
    } else {
        out
    }
}

fn display_name_for_source(source: &str) -> String {
    if let Ok(parsed) = parse_marketplace_source(source) {
        if let Some(repo) = parsed.github_repo {
            return repo.replace('-', " ");
        }
    }
    source
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("Marketplace")
        .trim_end_matches(".git")
        .replace('-', " ")
}

#[cfg(test)]
#[path = "plugin_marketplaces_tests.rs"]
mod tests;
