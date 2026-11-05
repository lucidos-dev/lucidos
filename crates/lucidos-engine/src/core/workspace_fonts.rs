//! Workspace fonts: fonts the user, the agent or a plugin installs beside the
//! font catalog (ADR 0308).
//!
//! A workspace font is a directory, `data/fonts/<slug>/`, holding a `font.json`
//! manifest and the font files it names. Its id is `ws-<slug>`.
//!
//! Two properties hold by construction, and every function here keeps them:
//!
//! - **No third-party request.** The manifest has no URL field, and a face is
//!   a leaf file name in its own directory. Clients build every face URL from
//!   the `/data` path, so the only origin a workspace font reaches is the
//!   local engine.
//! - **No user string reaches CSS.** The family is the id and the stack is a
//!   fixed chain per group. The label is text for a person to read.
//!
//! [`list`] validates every font each time it reads the directory, and that
//! check is the authority: a shell or a git pull writes files too. The data
//! route ([`validate_write`]) and plugin staging ([`list_in`]) run the same
//! rules first, so a writer reads the reason at once.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::fonts::{weight_reaches_bold, FontGroup, FontKind};

/// The `data/` subdirectory, and plugin content folder, that holds fonts.
pub const FONTS_DIR: &str = "fonts";

/// The manifest every font directory carries.
pub const MANIFEST_FILE: &str = "font.json";

/// Every workspace font id starts with this, and no catalog id may.
pub const ID_PREFIX: &str = "ws-";

pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_FACES: usize = 16;
pub const MAX_FONTS: usize = 100;
const MAX_MANIFEST_BYTES: usize = 16 * 1024;
pub const MAX_SLUG_LEN: usize = 40;
pub const MAX_LABEL_CHARS: usize = 60;
const MAX_LICENSE_CHARS: usize = 60;
const MAX_FILE_NAME_LEN: usize = 100;

/// Every group a workspace font can declare, in the order Settings lists them.
pub const GROUPS: [FontGroup; 3] = [FontGroup::Sans, FontGroup::Serif, FontGroup::Mono];

/// The slots a workspace font of `group` fits. A mono font fits UI text and
/// code, as every catalog mono font does.
fn kind_of(group: FontGroup) -> FontKind {
    match group {
        FontGroup::Sans | FontGroup::Serif => FontKind::Ui,
        FontGroup::Mono => FontKind::Both,
    }
}

/// The chain a workspace font's stack falls back to, ending in the generic
/// family that `FontSpec::group` reads. Clients rebuild a stack from it rather
/// than trust one off the wire, through the generated `font-catalog.ts`.
pub fn fallback(group: FontGroup) -> &'static str {
    match group {
        FontGroup::Sans => "system-ui, -apple-system, 'Segoe UI', sans-serif",
        FontGroup::Serif => "Georgia, 'Times New Roman', serif",
        FontGroup::Mono => "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaceStyle {
    #[default]
    Normal,
    Italic,
}

/// A font file format, told apart by its first four bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FontFormat {
    Woff2,
    Woff,
    TrueType,
    OpenType,
}

/// A container, as the extension and the first bytes must agree on it. A
/// `.ttf` and an `.otf` are both sfnt files and may hold either outline type,
/// which browsers load whatever the extension says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FontContainer {
    Woff2,
    Woff,
    Sfnt,
}

impl FontFormat {
    fn container(self) -> FontContainer {
        match self {
            FontFormat::Woff2 => FontContainer::Woff2,
            FontFormat::Woff => FontContainer::Woff,
            FontFormat::TrueType | FontFormat::OpenType => FontContainer::Sfnt,
        }
    }
}

impl FontFormat {
    fn from_extension(file_name: &str) -> Option<Self> {
        let (_, ext) = file_name.rsplit_once('.')?;
        match ext.to_ascii_lowercase().as_str() {
            "woff2" => Some(FontFormat::Woff2),
            "woff" => Some(FontFormat::Woff),
            "ttf" => Some(FontFormat::TrueType),
            "otf" => Some(FontFormat::OpenType),
            _ => None,
        }
    }

    fn from_magic(head: &[u8]) -> Option<Self> {
        match head.get(..4)? {
            b"wOF2" => Some(FontFormat::Woff2),
            b"wOFF" => Some(FontFormat::Woff),
            b"\x00\x01\x00\x00" | b"true" => Some(FontFormat::TrueType),
            b"OTTO" => Some(FontFormat::OpenType),
            _ => None,
        }
    }
}

/// `font.json` as written to disk.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub label: String,
    /// Decides the slots the font fits, the group Settings lists it under,
    /// and the fallback chain its stack ends in.
    pub group: FontGroup,
    #[serde(default)]
    pub ligatures: bool,
    #[serde(default)]
    pub license: Option<String>,
    pub faces: Vec<ManifestFace>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFace {
    pub file: String,
    #[serde(default = "default_weight")]
    pub weight: String,
    #[serde(default)]
    pub style: FaceStyle,
}

fn default_weight() -> String {
    "400".to_string()
}

/// One face, ready for a client to load. `path` is relative to `data/`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkspaceFontFace {
    pub path: String,
    pub weight: String,
    pub style: FaceStyle,
}

/// A workspace font that passed every check.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkspaceFont {
    pub id: String,
    pub label: String,
    /// The CSS family the faces register under. It is the id.
    pub family: String,
    pub stack: String,
    pub kind: FontKind,
    pub group: FontGroup,
    pub license: String,
    pub ligatures: bool,
    pub faces: Vec<WorkspaceFontFace>,
}

impl WorkspaceFont {
    /// Whether an upright face reaches a bold weight. A bold italic alone does
    /// not count: upright bold text would still be smeared. Without one,
    /// clients declare the upright faces over every weight.
    pub fn has_bold(&self) -> bool {
        self.faces
            .iter()
            .any(|face| face.style == FaceStyle::Normal && weight_reaches_bold(&face.weight))
    }
}

/// A font directory that failed a check, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InvalidWorkspaceFont {
    pub id: String,
    pub reason: String,
}

/// Every font directory under one `fonts/` root, sorted by id.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspaceFonts {
    pub fonts: Vec<WorkspaceFont>,
    pub invalid: Vec<InvalidWorkspaceFont>,
}

impl WorkspaceFonts {
    pub fn find(&self, id: &str) -> Option<&WorkspaceFont> {
        self.fonts.iter().find(|font| font.id == id)
    }
}

type Checked<T> = Result<T, String>;

fn validate_slug(slug: &str) -> Checked<()> {
    let well_formed = !slug.is_empty()
        && slug.len() <= MAX_SLUG_LEN
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--");
    if well_formed {
        Ok(())
    } else {
        Err(format!(
            "'{slug}' is not a font directory name: use up to {MAX_SLUG_LEN} lowercase letters and digits joined by single hyphens"
        ))
    }
}

/// The slug a well-formed workspace font id names: `ws-brand-sans` is
/// `brand-sans`.
fn slug_of(id: &str) -> Option<&str> {
    id.strip_prefix(ID_PREFIX)
        .filter(|slug| validate_slug(slug).is_ok())
}

/// Is this a well-formed workspace font id? Says nothing about whether the
/// font exists.
pub fn is_workspace_id(id: &str) -> bool {
    slug_of(id).is_some()
}

/// A face's file: a leaf name, in a font format, with nothing a URL or a
/// path could read as structure.
fn validate_file_name(file: &str) -> Checked<FontFormat> {
    let leaf = !file.is_empty()
        && file.len() <= MAX_FILE_NAME_LEN
        && !file.starts_with('.')
        && file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if !leaf {
        return Err(format!(
            "'{file}' is not a font file name: use letters, digits, '.', '_' and '-' only, with no folder"
        ));
    }
    FontFormat::from_extension(file).ok_or_else(|| {
        format!("'{file}' is not a font file: the extension must be .woff2, .woff, .ttf or .otf")
    })
}

/// A `font-weight` value: one weight, or the `min max` range of a variable
/// file. Each is 1 to 1000.
fn validate_weight(weight: &str) -> Checked<()> {
    // Digits only: `u16::from_str` would also take `+400`, which the client's
    // re-check refuses, and the font would list yet never paint.
    let number = |n: &str| -> Option<u16> {
        (!n.is_empty() && n.len() <= 4 && n.bytes().all(|b| b.is_ascii_digit()))
            .then(|| n.parse().ok())
            .flatten()
    };
    let numbers: Vec<Option<u16>> = weight.split(' ').map(number).collect();
    let in_range = |n: &Option<u16>| matches!(n, Some(1..=1000));
    let valid = match numbers.as_slice() {
        [one] => in_range(one),
        [min, max] => in_range(min) && in_range(max) && min <= max,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "weight '{weight}' must be one number from 1 to 1000, or a 'min max' pair"
        ))
    }
}

fn validate_text(field: &str, text: &str, max: usize) -> Checked<()> {
    if text.chars().count() > max {
        return Err(format!("`{field}` is longer than {max} characters"));
    }
    if text.chars().any(char::is_control) {
        return Err(format!("`{field}` contains a control character"));
    }
    Ok(())
}

/// Parse and check a manifest on its own. The files it names are checked
/// where they can be read, by [`load`].
pub fn parse_manifest(bytes: &[u8]) -> Checked<Manifest> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "{MANIFEST_FILE} is larger than {MAX_MANIFEST_BYTES} bytes"
        ));
    }
    let manifest: Manifest =
        serde_json::from_slice(bytes).map_err(|e| format!("not a valid {MANIFEST_FILE}: {e}"))?;
    if manifest.label.trim().is_empty() {
        return Err("`label` is required".to_string());
    }
    validate_text("label", &manifest.label, MAX_LABEL_CHARS)?;
    if let Some(license) = &manifest.license {
        validate_text("license", license, MAX_LICENSE_CHARS)?;
    }
    if manifest.faces.is_empty() || manifest.faces.len() > MAX_FACES {
        return Err(format!("`faces` must list 1 to {MAX_FACES} font files"));
    }
    for face in &manifest.faces {
        validate_file_name(&face.file)?;
        validate_weight(&face.weight)?;
    }
    Ok(manifest)
}

/// Check a font file's name, size and first bytes, which must be the format
/// its extension claims.
fn validate_font_file(file: &str, len: u64, head: &[u8]) -> Checked<()> {
    let claimed = validate_file_name(file)?;
    if len == 0 || len > MAX_FILE_BYTES {
        return Err(format!(
            "'{file}' must be between 1 byte and {} MiB",
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    match FontFormat::from_magic(head) {
        Some(found) if found.container() == claimed.container() => Ok(()),
        Some(_) => Err(format!(
            "'{file}' holds a different font format than its extension says"
        )),
        None => Err(format!(
            "'{file}' is not a woff2, woff, TrueType or OpenType font (a collection is not accepted)"
        )),
    }
}

/// The `fonts/`-relative part of a `data/`-relative path, if it has one.
fn fonts_relative(data_path: &str) -> Option<&str> {
    data_path.strip_prefix(FONTS_DIR)?.strip_prefix('/')
}

/// Whether a `data/`-relative path lies under `fonts/`.
pub fn is_font_path(data_path: &str) -> bool {
    fonts_relative(data_path).is_some()
}

/// The font half of `data_prefixes::validate_data_write`. A path outside
/// `fonts/` passes. A manifest is what makes a directory a font, so a new one
/// must fit under the cap ([`check_room`]).
pub fn validate_data_write(data_dir: &Path, data_path: &str, bytes: &[u8]) -> Checked<()> {
    let Some(rel) = fonts_relative(data_path) else {
        return Ok(());
    };
    validate_write(rel, bytes)?;
    if let Some(slug) = rel.strip_suffix(&format!("/{MANIFEST_FILE}")) {
        check_room(data_dir, [slug])?;
    }
    Ok(())
}

/// Refuse the fonts named by `slugs` if the new ones among them would take the
/// workspace past [`MAX_FONTS`]. Listing sorts by name, so an extra font would
/// otherwise push an installed one out of every surface.
pub fn check_room<'a>(data_dir: &Path, slugs: impl IntoIterator<Item = &'a str>) -> Checked<()> {
    let installed = list(data_dir);
    let new = slugs
        .into_iter()
        .filter(|slug| installed.find(&format!("{ID_PREFIX}{slug}")).is_none())
        .count();
    let excess = (installed.fonts.len() + new).saturating_sub(MAX_FONTS);
    if excess > 0 {
        return Err(format!(
            "a workspace holds at most {MAX_FONTS} fonts: remove {excess} before installing more"
        ));
    }
    Ok(())
}

/// Check a write of `bytes` to `fonts/<rel>`: a valid manifest at
/// `<slug>/font.json`, or a valid font file at `<slug>/<file>`.
pub fn validate_write(rel: &str, bytes: &[u8]) -> Checked<()> {
    let file = file_in_font_dir(rel)?;
    if file == MANIFEST_FILE {
        return parse_manifest(bytes).map(|_| ());
    }
    validate_font_file(file, bytes.len() as u64, bytes)
}

/// [`validate_write`] for a file bound for `fonts/<rel>` that lies at `path`.
/// It reads a manifest whole, but only a font file's size and first bytes.
pub fn validate_staged(rel: &str, path: &Path) -> Checked<()> {
    let file = file_in_font_dir(rel)?;
    let dir = path.parent().unwrap_or(path);
    if file == MANIFEST_FILE {
        return read_manifest(dir).and_then(|bytes| parse_manifest(&bytes).map(|_| ()));
    }
    check_face_on_disk(dir, file)
}

/// The file name in `<slug>/<file>`, once the slug is valid.
fn file_in_font_dir(rel: &str) -> Checked<&str> {
    let Some((slug, file)) = rel.split_once('/') else {
        return Err(format!(
            "'{FONTS_DIR}/{rel}' is not in a font directory: write {FONTS_DIR}/<slug>/{MANIFEST_FILE} and the font files beside it"
        ));
    };
    validate_slug(slug)?;
    Ok(file)
}

fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut head = Vec::with_capacity(4);
    std::fs::File::open(path)?.take(4).read_to_end(&mut head)?;
    Ok(head)
}

/// Check the face file a manifest names, where it lies on disk. A symlink is
/// refused, so a face is always a file inside its own directory.
fn check_face_on_disk(dir: &Path, file: &str) -> Checked<()> {
    let path = dir.join(file);
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| format!("'{file}' is named in {MANIFEST_FILE} but missing"))?;
    if !meta.file_type().is_file() {
        return Err(format!("'{file}' is not a regular file"));
    }
    let head = read_head(&path).map_err(|e| format!("'{file}' cannot be read: {e}"))?;
    validate_font_file(file, meta.len(), &head)
}

/// Read the manifest in `dir`. Like a face, it must be a regular file, never
/// a symlink, and its size is checked before a byte is read.
fn read_manifest(dir: &Path) -> Checked<Vec<u8>> {
    let path = dir.join(MANIFEST_FILE);
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| format!("the directory has no {MANIFEST_FILE}"))?;
    if !meta.file_type().is_file() {
        return Err(format!("{MANIFEST_FILE} is not a regular file"));
    }
    if meta.len() > MAX_MANIFEST_BYTES as u64 {
        return Err(format!(
            "{MANIFEST_FILE} is larger than {MAX_MANIFEST_BYTES} bytes"
        ));
    }
    std::fs::read(&path).map_err(|e| format!("{MANIFEST_FILE} cannot be read: {e}"))
}

/// Load and fully check the font in `dir`, whose directory name is `slug`.
fn load(dir: &Path, slug: &str) -> Checked<WorkspaceFont> {
    validate_slug(slug)?;
    let manifest = parse_manifest(&read_manifest(dir)?)?;
    for face in &manifest.faces {
        check_face_on_disk(dir, &face.file)?;
    }
    let id = format!("{ID_PREFIX}{slug}");
    Ok(WorkspaceFont {
        stack: format!("'{id}', {}", fallback(manifest.group)),
        family: id.clone(),
        kind: kind_of(manifest.group),
        group: manifest.group,
        label: manifest.label,
        license: manifest.license.unwrap_or_default(),
        ligatures: manifest.ligatures,
        faces: manifest
            .faces
            .into_iter()
            .map(|face| WorkspaceFontFace {
                path: format!("{FONTS_DIR}/{slug}/{}", face.file),
                weight: face.weight,
                style: face.style,
            })
            .collect(),
        id,
    })
}

/// Does this directory hold anything? An emptied directory is what removing
/// a font leaves behind, and it is not a broken font.
fn is_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).map_or(true, |mut entries| entries.next().is_none())
}

/// Every font directory under the `fonts/` root `root`, checked. A root that
/// does not exist holds no fonts.
pub fn list_in(root: &Path) -> WorkspaceFonts {
    let mut dirs: Vec<(String, std::path::PathBuf)> = match std::fs::read_dir(root) {
        Ok(entries) => entries
            .flatten()
            // Not followed: a symlinked directory could reach outside data/.
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .filter(|(name, path)| !name.starts_with('.') && !is_empty_dir(path))
            .collect(),
        Err(_) => Vec::new(),
    };
    dirs.sort();

    let mut out = WorkspaceFonts::default();
    for (slug, dir) in dirs {
        let id = format!("{ID_PREFIX}{slug}");
        if out.fonts.len() == MAX_FONTS {
            out.invalid.push(InvalidWorkspaceFont {
                id,
                reason: format!("a workspace holds at most {MAX_FONTS} fonts"),
            });
            continue;
        }
        match load(&dir, &slug) {
            Ok(font) => out.fonts.push(font),
            Err(reason) => out.invalid.push(InvalidWorkspaceFont { id, reason }),
        }
    }
    out
}

/// Every workspace font in `data_dir`, checked. Read fresh on every call, so
/// the engine holds no font state.
pub fn list(data_dir: &Path) -> WorkspaceFonts {
    list_in(&data_dir.join(FONTS_DIR))
}

#[cfg(test)]
#[path = "workspace_fonts_tests.rs"]
mod tests;
