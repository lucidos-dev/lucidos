//! *Plugin media*: a plugin's presentation files in its `media/` folder, named
//! by the plugin manifest (ADR 0414).
//!
//! Media is never workspace content. A scan keeps a copy per marketplace,
//! plugin and version under [`CACHE_DIR`], and install keeps one per plugin
//! under `data/`[`INSTALLED_DIR`], which is gitignored. Each copy is a *media
//! copy*: an `index.json` naming what it holds, plus the files under `files/`
//! at their plugin-relative paths. The media route serves a path only when the
//! index lists it.
//!
//! A media problem never blocks a scan or an install. The entry is dropped and
//! named in [`MediaIndex::problems`], with the limit it broke.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::apps::{AppKind, AppManifest, APP_ICON_EXTENSIONS, APP_ICON_MAX_BYTES};
use super::format_byte_size;

/// The plugin root folder that holds media. Never merged into `data/`.
pub const MEDIA_DIR: &str = "media";
/// The long description, rendered as markdown on the plugin detail page.
pub const README_PATH: &str = "media/README.md";
/// Installed media, under `data/`. Engine-managed and gitignored.
pub const INSTALLED_DIR: &str = "plugin-media";
/// Catalog media, workspace-relative. Rebuilt by the next scan.
pub const CACHE_DIR: &str = ".lucidos/plugin-media";
/// The media route's two mounts, under `/api/v1`. The router and the URLs a
/// catalog row carries both read these.
pub const CATALOG_ROUTE: &str = "/plugins/media/catalog";
pub const INSTALLED_ROUTE: &str = "/plugins/media/installed";

pub const MAX_SCREENSHOT_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_VIDEO_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_README_BYTES: u64 = 256 * 1024;
pub const MAX_SCREENSHOTS: usize = 8;
pub const MAX_VIDEOS: usize = 3;
/// Every kept file of one plugin together.
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

const SCREENSHOT_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];
const VIDEO_EXTENSIONS: [&str; 2] = ["mp4", "webm"];
const INDEX_FILE: &str = "index.json";
const FILES_DIR: &str = "files";

/// One dropped media entry, and why, in words an author can act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaProblem {
    /// The plugin-relative path, or the manifest key for a value of the wrong
    /// type.
    pub path: String,
    pub reason: String,
}

impl MediaProblem {
    fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            reason: reason.into(),
        }
    }
}

/// What the plugin manifest names, before any file is checked.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaDeclaration {
    pub icon: Option<String>,
    pub screenshots: Vec<String>,
    pub videos: Vec<String>,
    /// Values of the wrong type: the key is named, and the value ignored.
    pub problems: Vec<MediaProblem>,
}

impl MediaDeclaration {
    /// Read `icon`, `screenshots` and `videos` from a parsed manifest table.
    pub fn from_manifest(table: &toml::Table) -> Self {
        let mut out = Self::default();
        match table.get("icon") {
            None => {}
            Some(toml::Value::String(s)) => out.icon = Some(s.clone()),
            Some(_) => out.problems.push(MediaProblem::new(
                "icon",
                "must be a path such as \"media/icon.svg\"",
            )),
        }
        out.screenshots = string_list(table, "screenshots", &mut out.problems);
        out.videos = string_list(table, "videos", &mut out.problems);
        out
    }
}

fn string_list(table: &toml::Table, key: &str, problems: &mut Vec<MediaProblem>) -> Vec<String> {
    let Some(value) = table.get(key) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        problems.push(MediaProblem::new(key, "must be a list of paths"));
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        match item.as_str() {
            Some(s) => out.push(s.to_string()),
            None => problems.push(MediaProblem::new(key, "holds a value that is not a path")),
        }
    }
    out
}

/// What a media copy holds. Every path is plugin-relative, and the media route
/// serves exactly these.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaIndex {
    /// The plugin icon. Falls back to the icon of the plugin's single app.
    pub icon: Option<String>,
    pub screenshots: Vec<String>,
    pub videos: Vec<String>,
    pub readme: Option<String>,
    pub problems: Vec<MediaProblem>,
    /// SHA-256 per kept path, so a rescan of unchanged media writes nothing.
    pub digests: BTreeMap<String, String>,
}

impl MediaIndex {
    /// Whether the media route may serve `path` from this copy.
    pub fn lists(&self, path: &str) -> bool {
        self.icon.as_deref() == Some(path)
            || self.readme.as_deref() == Some(path)
            || self.screenshots.iter().any(|p| p == path)
            || self.videos.iter().any(|p| p == path)
    }
}

/// The checked media of one plugin tree, ready to copy.
#[derive(Debug)]
pub struct ResolvedMedia {
    pub index: MediaIndex,
    sources: Vec<(String, PathBuf)>,
}

#[derive(Clone, Copy)]
enum Kind {
    Icon,
    Screenshot,
    Video,
    Readme,
}

impl Kind {
    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Icon => &APP_ICON_EXTENSIONS,
            Self::Screenshot => &SCREENSHOT_EXTENSIONS,
            Self::Video => &VIDEO_EXTENSIONS,
            Self::Readme => &["md"],
        }
    }

    fn max_bytes(self) -> u64 {
        match self {
            Self::Icon => APP_ICON_MAX_BYTES,
            Self::Screenshot => MAX_SCREENSHOT_BYTES,
            Self::Video => MAX_VIDEO_BYTES,
            Self::Readme => MAX_README_BYTES,
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Icon => "icon",
            Self::Screenshot => "screenshot",
            Self::Video => "video",
            Self::Readme => "README",
        }
    }
}

/// Collects the kept files and the running total for one plugin.
struct Resolver<'a> {
    root: &'a Path,
    index: MediaIndex,
    sources: Vec<(String, PathBuf)>,
    total: u64,
}

impl Resolver<'_> {
    /// Check one entry and keep it, or record why not.
    fn keep(&mut self, raw: &str, kind: Kind) -> Option<String> {
        match self.check(raw, kind) {
            Ok(path) => Some(path),
            Err(reason) => {
                self.index.problems.push(MediaProblem::new(raw, reason));
                None
            }
        }
    }

    fn check(&mut self, raw: &str, kind: Kind) -> Result<String, String> {
        let source = media_file(self.root, raw)?;
        let ext = Path::new(raw)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !kind.extensions().contains(&ext.as_str()) {
            return Err(format!(
                "a {} must be one of: {}",
                kind.noun(),
                kind.extensions().join(", ")
            ));
        }
        self.admit(raw, source, kind)
    }

    /// Size, total and digest checks shared by media entries and the app icon
    /// fallback, which lives outside `media/`.
    fn admit(&mut self, path: &str, source: PathBuf, kind: Kind) -> Result<String, String> {
        if self.sources.iter().any(|(p, _)| p == path) {
            return Ok(path.to_string());
        }
        let meta = std::fs::symlink_metadata(&source).map_err(|_| "is missing".to_string())?;
        if !meta.is_file() {
            return Err("is not a regular file".to_string());
        }
        let len = meta.len();
        if len > kind.max_bytes() {
            return Err(format!(
                "is {}, over the {} {} limit",
                format_byte_size(len as usize),
                format_byte_size(kind.max_bytes() as usize),
                kind.noun()
            ));
        }
        if self.total + len > MAX_TOTAL_BYTES {
            return Err(format!(
                "would take the plugin's media past the {} total limit",
                format_byte_size(MAX_TOTAL_BYTES as usize)
            ));
        }
        let digest = sha256_of(&source).map_err(|e| format!("could not be read: {e}"))?;
        self.total += len;
        self.index.digests.insert(path.to_string(), digest);
        self.sources.push((path.to_string(), source));
        Ok(path.to_string())
    }

    fn keep_list(&mut self, raw: &[String], kind: Kind, cap: usize) -> Vec<String> {
        let mut kept: Vec<String> = Vec::new();
        for entry in raw {
            if kept.len() == cap {
                self.index.problems.push(MediaProblem::new(
                    entry.as_str(),
                    format!("is past the {cap} {} limit", kind.noun()),
                ));
                continue;
            }
            if let Some(path) = self.keep(entry, kind) {
                if !kept.contains(&path) {
                    kept.push(path);
                }
            }
        }
        kept
    }
}

/// The SHA-256 of a file, read in chunks so a video never sits whole in memory.
fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(crate::api::hex::hex_lower(&hasher.finalize()))
}

/// One path segment that names an entry inside its parent, and nothing else.
/// Every folder name a media copy is joined from passes it.
pub fn is_folder_name(segment: &str) -> bool {
    !segment.is_empty() && segment != "." && segment != ".." && !segment.contains(['/', '\\'])
}

/// The real file a plugin-relative media path names, refusing anything that
/// leaves `media/` or passes through a symlink.
fn media_file(root: &Path, raw: &str) -> Result<PathBuf, String> {
    // Checked as written, segment by segment: `Path::components` drops a
    // `.` in the middle, and the URL would then name another path.
    let mut segments = raw.split('/');
    let under_media = segments.next() == Some(MEDIA_DIR);
    let rest: Vec<&str> = segments.collect();
    if !under_media || rest.is_empty() || !rest.iter().all(|s| is_folder_name(s)) {
        return Err("must name a file inside media/".to_string());
    }
    let relative = Path::new(raw);
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component);
        let meta = std::fs::symlink_metadata(&path).map_err(|_| "is missing".to_string())?;
        if meta.file_type().is_symlink() {
            return Err("is a symlink, which media may not be".to_string());
        }
    }
    Ok(path)
}

/// Check the declared media of the plugin tree at `root`. Never fails: every
/// problem lands in the index.
pub fn resolve(root: &Path, declared: &MediaDeclaration) -> ResolvedMedia {
    let mut r = Resolver {
        root,
        index: MediaIndex {
            problems: declared.problems.clone(),
            ..MediaIndex::default()
        },
        sources: Vec::new(),
        total: 0,
    };
    r.index.icon = declared
        .icon
        .as_deref()
        .and_then(|raw| r.keep(raw, Kind::Icon));
    if r.index.icon.is_none() {
        r.index.icon = single_app_icon(root).and_then(|(path, source)| {
            r.admit(&path, source, Kind::Icon)
                .map_err(|reason| r.index.problems.push(MediaProblem::new(&path, reason)))
                .ok()
        });
    }
    r.index.screenshots = r.keep_list(&declared.screenshots, Kind::Screenshot, MAX_SCREENSHOTS);
    r.index.videos = r.keep_list(&declared.videos, Kind::Video, MAX_VIDEOS);
    if root.join(README_PATH).exists() || root.join(README_PATH).is_symlink() {
        r.index.readme = r.keep(README_PATH, Kind::Readme);
    }
    ResolvedMedia {
        index: r.index,
        sources: r.sources,
    }
}

/// The icon of the plugin's one app, as `(plugin-relative path, file)`. A
/// widget is not an app here, so a plugin with one app and its widgets still
/// gets the app's icon.
fn single_app_icon(root: &Path) -> Option<(String, PathBuf)> {
    let entries = std::fs::read_dir(root.join("apps")).ok()?;
    let mut apps = entries.flatten().filter_map(|entry| {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            return None;
        }
        let id = entry.file_name().to_str()?.to_string();
        let text = std::fs::read_to_string(entry.path().join("manifest.json")).ok()?;
        let manifest: AppManifest = serde_json::from_str(&text).ok()?;
        (AppKind::from_manifest(&id, manifest.kind.as_deref()) == AppKind::App).then_some((
            id,
            entry.path(),
            manifest,
        ))
    });
    let (id, dir, manifest) = apps.next()?;
    if apps.next().is_some() {
        return None;
    }
    let icon = super::apps::resolve_app_icon(&dir, manifest.icon.as_deref())?;
    let path = format!("apps/{id}/{icon}");
    is_plain_path(&path).then(|| (path, dir.join(icon)))
}

/// Where a scan keeps one plugin version's catalog media.
pub fn cache_dir(
    workspace: &Path,
    marketplace_id: &str,
    plugin_id: &str,
    version: &str,
) -> PathBuf {
    workspace
        .join(CACHE_DIR)
        .join(marketplace_id)
        .join(plugin_id)
        .join(version)
}

/// Where install keeps a plugin's media.
pub fn installed_dir(workspace: &Path, plugin_id: &str) -> PathBuf {
    workspace
        .join(super::DATA_DIR)
        .join(INSTALLED_DIR)
        .join(plugin_id)
}

/// Read a media copy's index. `None` when there is no copy.
pub fn read_index(copy: &Path) -> Option<MediaIndex> {
    load_index(copy).ok().flatten()
}

/// A media copy's index, telling no copy (`Ok(None)`) from one that cannot be
/// read (`Err`), so a damaged copy can say why it shows nothing.
pub fn load_index(copy: &Path) -> Result<Option<MediaIndex>, String> {
    let text = match std::fs::read_to_string(copy.join(INDEX_FILE)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Whether a plugin-relative path names a file by plain segments only: what
/// a browser sends back unchanged in a URL.
fn is_plain_path(path: &str) -> bool {
    path.split('/').all(is_folder_name)
}

/// The file the media route serves for `path`, when the copy lists it.
pub fn listed_file(copy: &Path, path: &str) -> Option<PathBuf> {
    if !is_plain_path(path) {
        return None;
    }
    read_index(copy)?
        .lists(path)
        .then(|| copy.join(FILES_DIR).join(path))
}

/// Make `copy` hold exactly `media`, replacing whatever it held as a whole.
/// A copy whose index already matches is left alone, so a rescan of
/// unchanged media writes nothing. No media at all leaves no copy.
pub fn write_copy(media: &ResolvedMedia, copy: &Path) -> std::io::Result<()> {
    if media.index == MediaIndex::default() {
        return remove_copy(copy);
    }
    let intact = media
        .sources
        .iter()
        .all(|(path, _)| copy.join(FILES_DIR).join(path).is_file());
    if intact && read_index(copy).as_ref() == Some(&media.index) {
        return Ok(());
    }
    let parent = copy
        .parent()
        .ok_or_else(|| std::io::Error::other("a media copy needs a parent folder"))?;
    std::fs::create_dir_all(parent)?;
    let staging = tempfile::TempDir::new_in(parent)?;
    for (path, source) in &media.sources {
        let dst = staging.path().join(FILES_DIR).join(path);
        if let Some(dir) = dst.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::copy(source, &dst)?;
    }
    std::fs::write(
        staging.path().join(INDEX_FILE),
        serde_json::to_vec_pretty(&media.index).map_err(std::io::Error::other)?,
    )?;
    remove_copy(copy)?;
    std::fs::rename(staging.keep(), copy)
}

/// Delete a media copy. A missing one is fine.
pub fn remove_copy(copy: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(copy) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Delete everything a scan cached for one marketplace. A missing folder is
/// fine, and so is an id no folder could have.
pub fn remove_marketplace_cache(workspace: &Path, marketplace_id: &str) -> std::io::Result<()> {
    if !is_folder_name(marketplace_id) {
        return Ok(());
    }
    remove_copy(&workspace.join(CACHE_DIR).join(marketplace_id))
}

/// One catalog media copy, by its three folder names.
pub type CacheKey = (String, String, String);

/// Drop catalog media no scan row needs any more. A marketplace that failed
/// this scan keeps what it had, and one no longer registered loses it all.
pub fn prune_cache(
    workspace: &Path,
    registered: &BTreeSet<String>,
    scanned: &BTreeSet<String>,
    keep: &BTreeSet<CacheKey>,
) {
    let root = workspace.join(CACHE_DIR);
    let Ok(marketplaces) = std::fs::read_dir(&root) else {
        return;
    };
    for marketplace in marketplaces.flatten() {
        let mp = marketplace.file_name().to_string_lossy().into_owned();
        if !registered.contains(&mp) {
            log_removal(&marketplace.path(), remove_copy(&marketplace.path()));
            continue;
        }
        if !scanned.contains(&mp) {
            continue;
        }
        for plugin in read_dirs(&marketplace.path()) {
            let id = plugin.file_name().to_string_lossy().into_owned();
            for version in read_dirs(&plugin.path()) {
                let v = version.file_name().to_string_lossy().into_owned();
                if !keep.contains(&(mp.clone(), id.clone(), v)) {
                    log_removal(&version.path(), remove_copy(&version.path()));
                }
            }
            let _ = std::fs::remove_dir(plugin.path());
        }
    }
}

fn read_dirs(dir: &Path) -> impl Iterator<Item = std::fs::DirEntry> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
}

fn log_removal(path: &Path, result: std::io::Result<()>) {
    if let Err(e) = result {
        log!("[PluginMedia] could not remove {}: {}", path.display(), e);
    }
}

/// The media a Plugins panel row and the plugin detail page draw, as URLs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMedia {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub screenshots: Vec<String>,
    #[serde(default)]
    pub videos: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readme_url: Option<String>,
    #[serde(default)]
    pub problems: Vec<MediaProblem>,
}

/// Which media copy a URL names.
pub enum MediaSource<'a> {
    Catalog {
        marketplace_id: &'a str,
        plugin_id: &'a str,
        version: &'a str,
    },
    Installed {
        plugin_id: &'a str,
    },
}

impl MediaSource<'_> {
    fn url_prefix(&self) -> String {
        let seg = |s: &str| urlencoding::encode(s).into_owned();
        match self {
            Self::Catalog {
                marketplace_id,
                plugin_id,
                version,
            } => format!(
                "{}{CATALOG_ROUTE}/{}/{}/{}",
                crate::api::API_V1_PREFIX,
                seg(marketplace_id),
                seg(plugin_id),
                seg(version)
            ),
            Self::Installed { plugin_id } => format!(
                "{}{INSTALLED_ROUTE}/{}",
                crate::api::API_V1_PREFIX,
                seg(plugin_id)
            ),
        }
    }

    /// The URLs for everything `index` lists.
    pub fn media(&self, index: &MediaIndex) -> PluginMedia {
        let prefix = self.url_prefix();
        let url = |path: &str| {
            let encoded: Vec<_> = path.split('/').map(urlencoding::encode).collect();
            format!("{prefix}/{}", encoded.join("/"))
        };
        PluginMedia {
            icon_url: index.icon.as_deref().map(url),
            screenshots: index.screenshots.iter().map(|p| url(p)).collect(),
            videos: index.videos.iter().map(|p| url(p)).collect(),
            readme_url: index.readme.as_deref().map(url),
            problems: index.problems.clone(),
        }
    }
}

#[cfg(test)]
#[path = "plugin_media_tests.rs"]
mod tests;
