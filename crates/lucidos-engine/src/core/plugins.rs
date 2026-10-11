use std::path::{Path, PathBuf};

use semver::{Op, Version, VersionReq};

use super::{is_build_output_file, VENDORED_DIR_NAMES};

/// Plugin top-level directory (and `data/` subdirectory) that holds compiled
/// WASM auth signers — `<name>.wasm` plus optional `<name>.manifest.json`
/// sidecar. Naming a single source for the literal so a typo in one place
/// (e.g. the post-install reload trigger) cannot silently break the auto-reload.
pub const AUTH_MODULES_DIR: &str = "auth-modules";

pub(crate) const CONTENT_DIRS: [&str; 7] = [
    "apps",
    "knowhow",
    "triggers",
    "scripts",
    AUTH_MODULES_DIR,
    super::themes::THEMES_DIR,
    super::workspace_fonts::FONTS_DIR,
];

/// File extension marking a plugin archive (renamed zip). Lowercase — callers
/// that match on filenames should compare against `to_ascii_lowercase()`.
pub const PLUGIN_ARCHIVE_EXT: &str = ".lucidos-plugin";

/// The longest plugin `id`, in bytes. Every id character is ASCII, so it is
/// also the limit in characters.
pub(crate) const MAX_ID_LEN: usize = 64;

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
    /// The *plugin media* the manifest names. A bad entry never fails the
    /// parse: `plugin_media::resolve` drops it and says why.
    pub media: super::plugin_media::MediaDeclaration,
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
    /// A file under `themes/` that is not a valid theme: `(path, reason)`.
    InvalidTheme(String, String),
    /// A file or directory under `fonts/` that is not a valid workspace font:
    /// `(path, reason)`.
    InvalidFont(String, String),
    /// An item directory (`apps/<id>`, `triggers/<slug>`, `fonts/<slug>`)
    /// named in [`VENDORED_DIR_NAMES`], which [`plan_files`] would skip.
    ItemNamedLikeBuildOutput(String),
    /// A widget folder a plugin may not ship: `(apps/<id>, reason)`.
    InvalidWidget(String, String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingManifest => write!(f, "manifest.toml not found at plugin root"),
            Self::ManifestParseError(e) => write!(f, "manifest.toml parse error: {}", e),
            Self::MissingField(name) => write!(f, "manifest.toml missing required field: {}", name),
            Self::InvalidId(id) => write!(
                f,
                "invalid id '{id}': must match [a-z0-9-]+ and be ≤ {MAX_ID_LEN} chars"
            ),
            Self::InvalidVersion(v) => write!(f, "invalid semver version: {}", v),
            Self::InvalidSource(s) => write!(f, "invalid source URL: {}", s),
            Self::EmptyTree => write!(
                f,
                "plugin has no content (none of {}/ exist)",
                CONTENT_DIRS.join("/, ")
            ),
            Self::UnexpectedTopLevelEntry(e) if e == super::themes::LEGACY_THEMES_DIR => write!(
                f,
                "unexpected top-level entry '{e}': themes now ship in {}/, so rename the folder",
                super::themes::THEMES_DIR
            ),
            Self::UnexpectedTopLevelEntry(e) => write!(
                f,
                "unexpected top-level entry '{}' (only manifest.toml, {}/ + {} allowed)",
                e,
                super::plugin_media::MEDIA_DIR,
                CONTENT_DIRS.join("/")
            ),
            Self::UnsafePath(p) => write!(f, "unsafe path in archive: {}", p),
            Self::InvalidTheme(path, reason) => write!(f, "{path} is not a valid theme: {reason}"),
            Self::InvalidFont(path, reason) => {
                write!(f, "{path} is not a valid workspace font: {reason}")
            }
            Self::InvalidWidget(path, reason) => {
                write!(f, "{path}/ is not a widget a plugin can ship: {reason}")
            }
            Self::ItemNamedLikeBuildOutput(path) => write!(
                f,
                "{path}/ has a build-output folder name, so install would skip it: rename the folder"
            ),
        }
    }
}

impl std::error::Error for ValidationError {}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
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
    if Version::parse(&version).is_err() {
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

    let media = super::plugin_media::MediaDeclaration::from_manifest(table);

    let raw = serde_json::to_value(&value)
        .map_err(|e| ValidationError::ManifestParseError(e.to_string()))?;

    Ok(PluginManifest {
        id,
        version,
        name,
        description,
        source,
        setup,
        categories,
        media,
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
        // No-follow: a symlink named as a content dir would pass `is_dir()` and
        // let the walk below escape the plugin tree. Reject any top-level entry
        // that is not a real directory named in CONTENT_DIRS, or `media/`,
        // which holds presentation files and is never installed as content.
        let is_real_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let allowed =
            CONTENT_DIRS.contains(&name_str.as_ref()) || name_str == super::plugin_media::MEDIA_DIR;
        if !allowed || !is_real_dir {
            return Err(ValidationError::UnexpectedTopLevelEntry(name_str.into()));
        }
    }
    reject_items_named_like_build_output(root)?;
    validate_widgets(&manifest.id, root)?;

    let planned = plan_files(root);
    if planned.is_empty() {
        return Err(ValidationError::EmptyTree);
    }
    let fonts = validate_fonts(root, &planned)?;
    validate_themes(&planned, &fonts)?;

    Ok((manifest, planned))
}

/// Content dirs that hold one directory per item.
const ITEM_DIRS: [&str; 3] = ["apps", "triggers", super::workspace_fonts::FONTS_DIR];

/// Refuse an item whose directory name [`plan_files`] skips as build output,
/// so a real app, trigger or font is never dropped without a word. Hidden
/// names stay out of scope: the walk skips every one of those anyway.
fn reject_items_named_like_build_output(root: &Path) -> Result<(), ValidationError> {
    for dir in ITEM_DIRS {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let is_real_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if is_real_dir
                && !name_str.starts_with('.')
                && VENDORED_DIR_NAMES.contains(&name_str.as_ref())
            {
                return Err(ValidationError::ItemNamedLikeBuildOutput(format!(
                    "{dir}/{name_str}"
                )));
            }
        }
    }
    Ok(())
}

/// A plugin ships a widget only as a reusable one that names this plugin as
/// its origin (ADR 0414). A thread-owned widget belongs to a thread in the
/// author's workspace, which no other workspace has.
fn validate_widgets(plugin_id: &str, root: &Path) -> Result<(), ValidationError> {
    use super::apps::{AppKind, AppManifest};
    let Ok(entries) = std::fs::read_dir(root.join("apps")) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path().join("manifest.json")) else {
            continue;
        };
        let Ok(app) = serde_json::from_str::<AppManifest>(&text) else {
            continue;
        };
        if AppKind::from_manifest(&name, app.kind.as_deref()) != AppKind::Widget {
            continue;
        }
        let invalid =
            |reason: String| ValidationError::InvalidWidget(format!("apps/{name}"), reason);
        if app.origin_thread_id.is_some() {
            return Err(invalid(
                "it names an origin_thread_id, and a shipped widget names its plugin instead"
                    .into(),
            ));
        }
        if app.origin_plugin_id.as_deref() != Some(plugin_id) {
            return Err(invalid(format!(
                "its manifest.json must set \"origin_plugin_id\": \"{plugin_id}\""
            )));
        }
        if !app.reusable {
            return Err(invalid(
                "its manifest.json must set \"reusable\": true".into(),
            ));
        }
    }
    Ok(())
}

/// Every file a plugin installs under `fonts/` must belong to a valid
/// workspace font. Checked here, so a broken font fails staging rather than
/// dropping out of the list after install. Answers the plugin's fonts.
fn validate_fonts(
    root: &Path,
    planned: &[PlannedFile],
) -> Result<super::workspace_fonts::WorkspaceFonts, ValidationError> {
    use super::workspace_fonts::{self, FONTS_DIR};
    let prefix = format!("{FONTS_DIR}/");
    for file in planned {
        let Some(rel) = file.data_relative.strip_prefix(&prefix) else {
            continue;
        };
        let invalid =
            |reason: String| ValidationError::InvalidFont(file.data_relative.clone(), reason);
        workspace_fonts::validate_staged(rel, &file.source).map_err(invalid)?;
    }
    let fonts = workspace_fonts::list_in(&root.join(FONTS_DIR));
    if let Some(broken) = fonts.invalid.first() {
        // Every listed id is the prefix and the directory name.
        let dir = broken
            .id
            .strip_prefix(workspace_fonts::ID_PREFIX)
            .unwrap_or(&broken.id);
        return Err(ValidationError::InvalidFont(
            format!("{prefix}{dir}"),
            broken.reason.clone(),
        ));
    }
    Ok(fonts)
}

/// Every file a plugin installs under `themes/` must be a valid theme with a
/// fresh id. A theme may name a workspace font only if this plugin ships it.
/// So a plugin never depends on a font the user happened to install. Checked
/// here, so a broken theme fails staging rather than install.
fn validate_themes(
    planned: &[PlannedFile],
    fonts: &super::workspace_fonts::WorkspaceFonts,
) -> Result<(), ValidationError> {
    let prefix = format!("{}/", super::themes::THEMES_DIR);
    for file in planned {
        let Some(file_name) = file.data_relative.strip_prefix(&prefix) else {
            continue;
        };
        let invalid =
            |reason: String| ValidationError::InvalidTheme(file.data_relative.clone(), reason);
        let bytes = std::fs::read(&file.source).map_err(|e| invalid(e.to_string()))?;
        super::themes::validate_workspace_write(file_name, &bytes, || fonts.clone())
            .map_err(|e| invalid(e.to_string()))?;
    }
    Ok(())
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
/// Skips dotfiles, any directory not in `CONTENT_DIRS`, and build output
/// (a [`VENDORED_DIR_NAMES`] directory or a bytecode file). Build output is
/// machine-specific, so it must never reach the install record.
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
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        // `file_type()` reads the directory entry without following a symlink,
        // unlike `Path::is_dir`/`is_file`. Skip symlinks so a plugin cannot plan
        // a path outside its own tree. A git-sourced plugin ships whatever
        // symlinks its author committed, and following one would copy an
        // arbitrary host file into `data/` on install.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        let next_prefix = format!("{}/{}", prefix, name_str);
        if file_type.is_dir() {
            if !VENDORED_DIR_NAMES.contains(&name_str.as_ref()) {
                walk_into(&path, &next_prefix, out);
            }
        } else if file_type.is_file() && !is_build_output_file(&name_str) {
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

/// Refuse a plugin whose fonts would take the workspace past its font cap.
/// Install copies files straight into `data_dir`, so the write-time check
/// never sees them.
pub fn check_font_room(planned: &[PlannedFile], data_dir: &Path) -> Result<(), String> {
    use super::workspace_fonts::{self, FONTS_DIR, MANIFEST_FILE};
    let prefix = format!("{FONTS_DIR}/");
    let suffix = format!("/{MANIFEST_FILE}");
    let slugs = planned
        .iter()
        .filter_map(|p| p.data_relative.strip_prefix(&prefix)?.strip_suffix(&suffix));
    workspace_fonts::check_room(data_dir, slugs)
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
    let inst = Version::parse(installed).ok();
    let rem = Version::parse(remote).ok();
    match (inst, rem) {
        (Some(i), Some(r)) if r > i => UpdateDecision::Update,
        (Some(_), Some(_)) => UpdateDecision::AlreadyLatest,
        _ => UpdateDecision::Update,
    }
}

/// Why the running Lucidos cannot take a plugin, judged from the manifest's
/// optional `engine` requirement (a semver requirement such as `">=0.46.1"`).
#[derive(Debug, Clone, PartialEq)]
pub enum EngineMismatch {
    /// The requirement is valid and `running`, a release triple, misses it.
    Unsatisfied {
        requirement: String,
        running: Version,
    },
    /// The value is not a semver requirement. Held as written, quoted.
    Invalid { value: String },
    /// The plugin has a requirement and Lucidos could not read its release.
    UnknownRelease { error: String },
}

impl EngineMismatch {
    /// The phrase a catalog row shows beside its disabled button.
    pub fn short_reason(&self) -> String {
        match self {
            Self::Unsatisfied { requirement, .. } => {
                format!("Needs Lucidos {}", describe_requirement(requirement))
            }
            Self::Invalid { value } => format!("Its engine requirement {value} is not valid"),
            Self::UnknownRelease { .. } => {
                "Lucidos could not read its own version to check this plugin".to_string()
            }
        }
    }

    /// The sentence staging refuses with, naming the plugin and both versions.
    pub fn refusal(&self, plugin_name: &str, plugin_version: &str) -> String {
        match self {
            Self::Unsatisfied {
                requirement,
                running,
            } => format!(
                "{plugin_name} {plugin_version} needs Lucidos {}. This is Lucidos {running}. \
                 Update Lucidos first.",
                describe_requirement(requirement)
            ),
            Self::Invalid { value } => format!(
                "{plugin_name} {plugin_version} declares engine = {value} in its manifest, \
                 which is not a valid version requirement (for example \">=0.46.1\"). \
                 Nothing was installed. Ask the plugin's author to fix the manifest."
            ),
            Self::UnknownRelease { error } => format!(
                "{plugin_name} {plugin_version} declares an engine requirement, and Lucidos \
                 could not read its own version to check it: {error}"
            ),
        }
    }
}

/// The `engine` value as the author wrote it, for display. A string comes back
/// bare; any other TOML value comes back in its JSON form.
pub fn engine_requirement_of(manifest: &serde_json::Value) -> Option<String> {
    manifest.get("engine").map(|v| match v.as_str() {
        Some(s) => s.to_string(),
        None => v.to_string(),
    })
}

/// The one check every install and update path runs. `manifest` is the raw
/// manifest JSON. `running` is `release_notices::running_release()`, which
/// already counts a dirty dev build as the next patch.
///
/// Matching uses the release triple only. Plain semver never lets a
/// pre-release satisfy `>=0.46.1`, which would refuse every dev build.
/// No `engine` field means any release will do, readable or not. Any value
/// that is not a valid requirement string is refused, so a typo never installs.
pub fn check_engine_requirement(
    manifest: &serde_json::Value,
    running: &Result<Version, String>,
) -> Result<(), EngineMismatch> {
    let Some(value) = manifest.get("engine") else {
        return Ok(());
    };
    let invalid = || EngineMismatch::Invalid {
        value: value.to_string(),
    };
    let text = value.as_str().ok_or_else(invalid)?;
    let requirement = VersionReq::parse(text).map_err(|_| invalid())?;
    let running =
        running
            .as_ref()
            .map(release_triple)
            .map_err(|error| EngineMismatch::UnknownRelease {
                error: error.clone(),
            })?;
    if requirement.matches(&running) {
        Ok(())
    } else {
        Err(EngineMismatch::Unsatisfied {
            requirement: text.trim().to_string(),
            running,
        })
    }
}

fn release_triple(version: &Version) -> Version {
    Version::new(version.major, version.minor, version.patch)
}

/// `>=0.46.1` reads as "0.46.1 or later". Any other shape is quoted as written.
fn describe_requirement(requirement: &str) -> String {
    match VersionReq::parse(requirement)
        .ok()
        .as_ref()
        .map(|r| r.comparators.as_slice())
    {
        Some([floor]) if floor.op == Op::GreaterEq && floor.pre.is_empty() => format!(
            "{}.{}.{} or later",
            floor.major,
            floor.minor.unwrap_or(0),
            floor.patch.unwrap_or(0)
        ),
        _ => requirement.to_string(),
    }
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
