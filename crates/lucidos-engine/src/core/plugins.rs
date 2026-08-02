use std::path::{Path, PathBuf};

/// Plugin top-level directory (and `data/` subdirectory) that holds compiled
/// WASM auth signers — `<name>.wasm` plus optional `<name>.manifest.json`
/// sidecar. Naming a single source for the literal so a typo in one place
/// (e.g. the post-install reload trigger) cannot silently break the auto-reload.
pub const AUTH_MODULES_DIR: &str = "auth-modules";

pub(crate) const CONTENT_DIRS: [&str; 5] =
    ["apps", "knowhow", "triggers", "scripts", AUTH_MODULES_DIR];

/// File extension marking a plugin archive (renamed zip). Lowercase — callers
/// that match on filenames should compare against `to_ascii_lowercase()`.
pub const PLUGIN_ARCHIVE_EXT: &str = ".lucidos-plugin";

/// Parsed `manifest.toml` after validation. Fields beyond v1 are kept on the
/// raw `serde_json::Value` so they round-trip into the event payload.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginManifest {
    pub id: String,
    pub version: String,
    pub name: String,
    pub description: String,
    /// Git remote where updates can be fetched. Optional so authors can share
    /// `.lucidos-plugin` archives without first publishing to a public repo --
    /// `update_plugin` and `check_plugin_updates` will refuse a plugin without
    /// one, but install + uninstall work fine.
    pub source: Option<String>,
    pub engine: Option<String>,
    /// Optional post-install instructions for the LLM to act on (e.g. "create a
    /// daily reflection trigger using `knowhow/foo/run.md`"). Surfaced verbatim
    /// in the `install_plugin` tool result so the agent can offer to wire it up
    /// on the same turn as the install. Free-form markdown -- the author writes
    /// it; the engine does not interpret it.
    pub setup: Option<String>,
    /// Topical categories the author tagged this plugin with (e.g. `finance`,
    /// `health`). A *controlled vocabulary* (see [`PLUGIN_CATEGORIES`]): values
    /// here are as authored — unknown ones are filtered + flagged at catalog
    /// scan time (`scan_catalog`), not rejected at parse, so one bad tag never
    /// blocks install. Lowercased + trimmed + de-duplicated on parse.
    pub categories: Vec<String>,
    /// Full manifest as JSON, so future additive fields land in the event.
    pub raw: serde_json::Value,
}

/// The *controlled vocabulary* of plugin topical categories (kebab-case, the
/// public-API value convention). Authors tag a plugin in `manifest.toml`
/// (`categories = ["finance", ...]`); the Plugins → Store tab filters/browses by
/// these. A value outside this set is dropped + flagged at scan time. Keep in
/// sync with `system-knowhow/plugins.md` and the *plugin category*
/// glossary entry.
pub const PLUGIN_CATEGORIES: &[&str] = &[
    "productivity",
    "finance",
    "health",
    "developer-tools",
    "data",
    "communication",
    "automation",
    "lifestyle",
    "research",
    "fun",
];

/// True when `c` is a recognised plugin category (exact, case-sensitive — values
/// are normalised to lowercase on parse).
pub fn is_valid_category(c: &str) -> bool {
    PLUGIN_CATEGORIES.contains(&c)
}

/// Split authored categories into `(known, unknown)`, preserving order and
/// dropping duplicates. `known` is what the catalog surfaces (browsable);
/// `unknown` is surfaced as a non-blocking scan warning.
pub fn partition_categories(categories: &[String]) -> (Vec<String>, Vec<String>) {
    let mut known = Vec::new();
    let mut unknown = Vec::new();
    for c in categories {
        if is_valid_category(c) {
            if !known.contains(c) {
                known.push(c.clone());
            }
        } else if !unknown.contains(c) {
            unknown.push(c.clone());
        }
    }
    (known, unknown)
}

/// Reasons a plugin archive is rejected at validation.
#[derive(Debug, PartialEq)]
pub enum ValidationError {
    MissingManifest,
    ManifestParseError(String),
    MissingField(&'static str),
    InvalidId(String),
    InvalidVersion(String),
    InvalidSource(String),
    EmptyTree,
    UnexpectedTopLevelEntry(String),
    UnsafePath(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingManifest => write!(f, "manifest.toml not found at plugin root"),
            Self::ManifestParseError(e) => write!(f, "manifest.toml parse error: {}", e),
            Self::MissingField(name) => write!(f, "manifest.toml missing required field: {}", name),
            Self::InvalidId(id) => write!(
                f,
                "invalid id '{}': must match [a-z0-9-]+ and be ≤ 64 chars",
                id
            ),
            Self::InvalidVersion(v) => write!(f, "invalid semver version: {}", v),
            Self::InvalidSource(s) => write!(f, "invalid source URL: {}", s),
            Self::EmptyTree => write!(
                f,
                "plugin has no content (none of apps/, knowhow/, triggers/, scripts/, auth-modules/ exist)"
            ),
            Self::UnexpectedTopLevelEntry(e) => write!(
                f,
                "unexpected top-level entry '{}' (only manifest.toml + apps/knowhow/triggers/scripts/auth-modules allowed)",
                e
            ),
            Self::UnsafePath(p) => write!(f, "unsafe path in archive: {}", p),
        }
    }
}

impl std::error::Error for ValidationError {}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Parse a manifest from raw TOML bytes. Validates required fields and
/// constraints. Does not touch the filesystem.
pub fn parse_manifest(toml_text: &str) -> Result<PluginManifest, ValidationError> {
    let value: toml::Value = toml::from_str(toml_text)
        .map_err(|e| ValidationError::ManifestParseError(e.to_string()))?;

    let table = value
        .as_table()
        .ok_or_else(|| ValidationError::ManifestParseError("expected a table".into()))?;

    let id = table
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or(ValidationError::MissingField("id"))?
        .to_string();
    if !is_valid_id(&id) {
        return Err(ValidationError::InvalidId(id));
    }

    let version = table
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or(ValidationError::MissingField("version"))?
        .to_string();
    if semver::Version::parse(&version).is_err() {
        return Err(ValidationError::InvalidVersion(version));
    }

    let name = table
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or(ValidationError::MissingField("name"))?
        .to_string();

    let description = table
        .get("description")
        .and_then(|v| v.as_str())
        .ok_or(ValidationError::MissingField("description"))?
        .to_string();

    let source = match table.get("source").and_then(|v| v.as_str()) {
        Some(s) if is_git_url(s) => Some(s.to_string()),
        Some(s) => return Err(ValidationError::InvalidSource(s.to_string())),
        None => None,
    };

    let engine = table
        .get("engine")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let setup = table
        .get("setup")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Categories are normalised (lowercase + trim) but NOT controlled-set
    // checked here — an unknown tag is filtered + flagged at scan time so it
    // can't block install. Non-array / non-string entries are ignored.
    let mut categories: Vec<String> = Vec::new();
    if let Some(arr) = table.get("categories").and_then(|v| v.as_array()) {
        for entry in arr {
            if let Some(s) = entry.as_str() {
                let norm = s.trim().to_ascii_lowercase();
                if !norm.is_empty() && !categories.contains(&norm) {
                    categories.push(norm);
                }
            }
        }
    }

    let raw = serde_json::to_value(&value)
        .map_err(|e| ValidationError::ManifestParseError(e.to_string()))?;

    Ok(PluginManifest {
        id,
        version,
        name,
        description,
        source,
        engine,
        setup,
        categories,
        raw,
    })
}

/// Loose check for "looks like a git remote" — used to reject plainly bad
/// `source` values in the manifest and to distinguish git URLs from archive
/// paths in `engine::tools::plugins::detect_source`.
pub fn is_git_url(s: &str) -> bool {
    s.starts_with("https://")
        || s.starts_with("http://")
        || s.starts_with("git@")
        || s.ends_with(".git")
}

/// Validate the on-disk structure of an unpacked plugin at `root`. Walks the
/// content directories once and returns the parsed manifest plus the install
/// plan, so the install path doesn't have to walk again.
pub fn validate_tree(root: &Path) -> Result<(PluginManifest, Vec<PlannedFile>), ValidationError> {
    let manifest_path = root.join("manifest.toml");
    if !manifest_path.is_file() {
        return Err(ValidationError::MissingManifest);
    }

    let toml_text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| ValidationError::ManifestParseError(e.to_string()))?;
    let manifest = parse_manifest(&toml_text)?;

    let entries =
        std::fs::read_dir(root).map_err(|e| ValidationError::ManifestParseError(e.to_string()))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == "manifest.toml" {
            continue;
        }
        if !CONTENT_DIRS.contains(&name_str.as_ref()) || !entry.path().is_dir() {
            return Err(ValidationError::UnexpectedTopLevelEntry(name_str.into()));
        }
    }

    let planned = plan_files(root);
    if planned.is_empty() {
        return Err(ValidationError::EmptyTree);
    }

    Ok((manifest, planned))
}

/// Verify that no archive entry path uses `..` or absolute paths (zip-slip), and
/// reject the empty string outright — empty inner names produce an opaque
/// "not found" downstream and are never a real entry name.
pub fn validate_archive_entry_path(path: &str) -> Result<(), ValidationError> {
    if path.is_empty() {
        return Err(ValidationError::UnsafePath(path.into()));
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(ValidationError::UnsafePath(path.into()));
    }
    for component in path.split(['/', '\\']) {
        if component == ".." {
            return Err(ValidationError::UnsafePath(path.into()));
        }
    }
    Ok(())
}

/// One file the plugin would install — source under the plugin tree, dest
/// under the workspace `data/` directory (relative path stored in the event).
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedFile {
    pub source: PathBuf,
    /// Path relative to `data/`, e.g. `knowhow/browser-skills.md`.
    pub data_relative: String,
}

/// Walk the plugin tree under `root` and produce the install plan.
/// Skips dotfiles and any directory not in `CONTENT_DIRS`.
pub fn plan_files(root: &Path) -> Vec<PlannedFile> {
    let mut out = Vec::new();
    for dir in CONTENT_DIRS {
        let sub = root.join(dir);
        if sub.is_dir() {
            walk_into(&sub, dir, &mut out);
        }
    }
    out.sort_by(|a, b| a.data_relative.cmp(&b.data_relative));
    out
}

fn walk_into(dir: &Path, prefix: &str, out: &mut Vec<PlannedFile>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        let next_prefix = format!("{}/{}", prefix, name_str);
        if path.is_dir() {
            walk_into(&path, &next_prefix, out);
        } else if path.is_file() {
            out.push(PlannedFile {
                source: path,
                data_relative: next_prefix,
            });
        }
    }
}

/// Among `planned`, return the data-relative paths that already exist under
/// `data_dir`. Caller decides whether to abort or proceed (overwrite).
pub fn detect_conflicts(planned: &[PlannedFile], data_dir: &Path) -> Vec<String> {
    planned
        .iter()
        .filter(|p| data_dir.join(&p.data_relative).exists())
        .map(|p| p.data_relative.clone())
        .collect()
}

/// Decision returned when `update_plugin` compares installed vs remote.
#[derive(Debug, PartialEq)]
pub enum UpdateDecision {
    /// `installed >= remote` — caller returns "Already at latest".
    AlreadyLatest,
    /// `remote > installed` — caller re-installs from `source` with overwrite.
    Update,
}

/// Compare two semver strings. Treats unparseable versions as needing update —
/// we'd rather attempt the install than silently no-op on a malformed version.
pub fn compare_versions(installed: &str, remote: &str) -> UpdateDecision {
    let inst = semver::Version::parse(installed).ok();
    let rem = semver::Version::parse(remote).ok();
    match (inst, rem) {
        (Some(i), Some(r)) if r > i => UpdateDecision::Update,
        (Some(_), Some(_)) => UpdateDecision::AlreadyLatest,
        _ => UpdateDecision::Update,
    }
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
