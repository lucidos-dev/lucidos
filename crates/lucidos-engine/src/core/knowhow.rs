use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::core::system_knowhow::SystemKnowhowStore;

/// Prefix that routes a knowhow ID to engine-shipped reference knowhow at
/// `<repo>/system-knowhow/`. IDs without this prefix come from the
/// workspace-local or shared user-curated knowhow dirs.
pub const SYSTEM_KNOWHOW_PREFIX: &str = "system-knowhow/";

/// Test-only helper: write a single knowhow file with the YAML frontmatter
/// (`name: ...`) the loader expects. Shared by `core::knowhow::tests` so all
/// knowhow fixtures share one definition.
#[cfg(test)]
pub(crate) fn write_knowhow_file(path: &std::path::Path, name: &str, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("---\nname: {}\n---\n{}", name, body)).unwrap();
}

/// Bundles the user-curated knowhow search directories.
/// Priority (highest wins): local > shared > apps > triggers.
///
/// `apps` is the workspace's `data/apps/` root. App-scoped knowhow ids of the
/// form `<app_id>/<rest>` resolve to `<apps>/<app_id>/knowhow/<rest>.md` as the
/// last fallback — bare-id local + shared still win when both exist. The
/// engine surfaces app-scoped ids in the system prompt's Know-how list, so
/// validators and loaders that work with `KnowhowDirs` must accept them.
///
/// `triggers` is the workspace's `data/triggers/` root. Ids of the form
/// `triggers/<slug>/<rest>` resolve to `<triggers>/<slug>/knowhow/<rest>.md`,
/// mirroring the app-scoped namespace one level down. Listed in the system
/// prompt only for threads of the matching trigger; resolution is global so
/// `load_knowhow` can fetch by id from any caller that knows it.
///
/// Engine-shipped reference knowhow lives in `<repo>/system-knowhow/` and is
/// loaded separately via [`crate::core::SystemKnowhowStore`] — it cannot be
/// overridden by a workspace.
pub struct KnowhowDirs {
    pub shared: Option<PathBuf>,
    pub local: PathBuf,
    pub apps: Option<PathBuf>,
    pub triggers: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Knowhow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
}

impl Knowhow {
    /// Format as a tagged section for LLM context injection.
    pub fn format_section(&self) -> String {
        format!(
            "[KNOW-HOW: {}]\n{}\n[END KNOW-HOW]",
            self.name, self.content
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowhowSummary {
    pub id: String,
    pub name: String,
    pub description: String,
}

pub struct KnowhowStore;

/// Parse knowhow frontmatter: extracts name, description, and body.
/// If `description:` is present in frontmatter, uses that.
/// Otherwise derives a description from the name + first paragraph of body.
///
/// Kept separate from `super::parse_md_frontmatter` because that helper
/// extracts a list field (e.g. `knowhow: [a, b]`), while knowhow files use
/// a scalar `description:` with a derive-from-body fallback. The shared
/// `---\n...\n---\n` header split is factored out into `split_md_frontmatter`.
fn parse_frontmatter(text: &str) -> Option<(String, String, String)> {
    let (frontmatter, body) = super::split_md_frontmatter(text)?;

    let mut name = None;
    let mut description = None;

    for line in frontmatter.lines() {
        if let Some(value) = line.strip_prefix("name:") {
            let v = value.trim().trim_matches('"');
            if !v.is_empty() {
                name = Some(v.to_string());
            }
        } else if let Some(value) = line.strip_prefix("description:") {
            let v = value.trim().trim_matches('"');
            if !v.is_empty() {
                description = Some(v.to_string());
            }
        }
    }

    let name = name?;
    let description = description.unwrap_or_else(|| derive_description(&name, &body));
    Some((name, description, body))
}

/// Derive a semantic description from name + first substantive paragraph of body.
fn derive_description(name: &str, body: &str) -> String {
    let first_line = body
        .lines()
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .unwrap_or("");

    if first_line.is_empty() {
        name.to_string()
    } else {
        let line: String = first_line.chars().take(200).collect();
        format!("{}: {}", name, line)
    }
}

pub(crate) fn collect_md_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            log!(
                "[Knowhow] Failed to read directory {}: {}",
                dir.display(),
                e
            );
            return files;
        }
    };
    let mut sorted = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => sorted.push(e),
            Err(e) => log!("[Knowhow] Failed to read entry in {}: {}", dir.display(), e),
        }
    }
    sorted.sort_by_key(|e| e.file_name());
    for entry in sorted {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_md_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            files.push(path);
        }
    }
    files
}

/// e.g., `knowhow/weather/api.md` → `weather/api`
pub(crate) fn id_from_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let without_ext = rel.with_extension("");
    let s = without_ext.to_str()?;
    Some(s.replace('\\', "/"))
}

impl KnowhowStore {
    /// Load name + description for all know-how files recursively (cheap, loaded at startup)
    pub fn load_summaries(knowhow_dir: &Path) -> Vec<KnowhowSummary> {
        let mut summaries = Vec::new();

        if !knowhow_dir.exists() {
            return summaries;
        }

        for path in collect_md_files(knowhow_dir) {
            let id = match id_from_path(knowhow_dir, &path) {
                Some(id) => id,
                None => continue,
            };

            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    log!("[Knowhow] Failed to read {}: {}", path.display(), e);
                    continue;
                }
            };

            if let Some((name, description, _)) = parse_frontmatter(&text) {
                summaries.push(KnowhowSummary {
                    id,
                    name,
                    description,
                });
            } else {
                log!(
                    "[Knowhow] Missing or invalid frontmatter in {}",
                    path.display()
                );
            }
        }

        summaries
    }

    /// Load know-how summaries for one trigger from
    /// `<triggers_dir>/<slug>/knowhow/`. Returns empty if no knowhow dir
    /// exists for the slug. IDs are bare filenames (no slug prefix in
    /// the id; the trigger system prompt section labels them by slug
    /// separately, mirroring `load_app_summaries`).
    pub fn load_trigger_summaries(triggers_dir: &Path, slug: &str) -> Vec<KnowhowSummary> {
        let kh_dir = triggers_dir.join(slug).join("knowhow");
        Self::load_summaries(&kh_dir)
    }

    /// Load know-how summaries from all app knowhow/ subdirectories.
    /// Returns (app_id, summary) pairs for each app that has knowhow files.
    pub fn load_app_summaries(apps_dir: &Path) -> Vec<(String, KnowhowSummary)> {
        let mut results = Vec::new();
        if !apps_dir.exists() {
            return results;
        }
        let entries = match std::fs::read_dir(apps_dir) {
            Ok(entries) => entries,
            Err(e) => {
                log!(
                    "[Knowhow] Failed to read apps directory {}: {}",
                    apps_dir.display(),
                    e
                );
                return results;
            }
        };
        let mut app_dirs: Vec<_> = entries.flatten().collect();
        app_dirs.sort_by_key(|e| e.file_name());
        for entry in app_dirs {
            let app_id = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let kh_dir = entry.path().join("knowhow");
            let summaries = Self::load_summaries(&kh_dir);
            for s in summaries {
                results.push((app_id.clone(), s));
            }
        }
        results
    }

    /// Load summaries from shared and local directories, deduplicating by ID.
    /// Priority (highest wins): local > shared.
    pub fn load_merged_summaries(dirs: &KnowhowDirs) -> Vec<KnowhowSummary> {
        let mut by_id: HashMap<String, KnowhowSummary> = HashMap::new();

        if let Some(shared) = &dirs.shared {
            for s in Self::load_summaries(shared) {
                by_id.insert(s.id.clone(), s);
            }
        }

        for s in Self::load_summaries(&dirs.local) {
            by_id.insert(s.id.clone(), s);
        }

        let mut result: Vec<KnowhowSummary> = by_id.into_values().collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }

    /// Load full content for a specific know-how file.
    /// Priority (highest wins): local > shared > app-scoped > trigger-scoped.
    /// App-scoped: an id of the form `<app_id>/<rest>` falls back to
    /// `<apps>/<app_id>/knowhow/<rest>.md`.
    /// Trigger-scoped: an id of the form `triggers/<slug>/<rest>` falls back
    /// to `<triggers>/<slug>/knowhow/<rest>.md`. The `triggers/` prefix
    /// disambiguates from the `<app>/<rest>` namespace; both are consulted
    /// only if no top-level file matched, so a same-shaped local entry wins.
    pub fn load_with_fallback(dirs: &KnowhowDirs, id: &str) -> Option<Knowhow> {
        Self::load(&dirs.local, id)
            .or_else(|| {
                dirs.shared
                    .as_deref()
                    .and_then(|shared| Self::load(shared, id))
            })
            .or_else(|| {
                let path = app_scoped_knowhow_path(dirs.apps.as_deref()?, id)?;
                read_knowhow_file(&path, id)
            })
            .or_else(|| {
                let path = trigger_scoped_knowhow_path(dirs.triggers.as_deref()?, id)?;
                read_knowhow_file(&path, id)
            })
    }

    /// Load full content for a specific know-how file.
    /// The id may contain forward slashes for files in subdirectories (e.g., "weather/api").
    pub fn load(knowhow_dir: &Path, id: &str) -> Option<Knowhow> {
        if !is_safe_id(id) {
            log!("[Knowhow] Invalid id (path traversal): {}", id);
            return None;
        }
        let path = knowhow_dir.join(format!("{}.md", id));
        read_knowhow_file(&path, id)
    }
}

/// Read+parse a single knowhow file at `path` and stamp `id` onto the result.
/// Used by both the dir+id path ([`KnowhowStore::load`]) and the app-scoped
/// fallback that resolves to a fully built path with a different id-shape.
fn read_knowhow_file(path: &Path, id: &str) -> Option<Knowhow> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            log!("[Knowhow] Failed to read {}: {}", path.display(), e);
            return None;
        }
    };
    let (name, description, content) = match parse_frontmatter(&text) {
        Some(parsed) => parsed,
        None => {
            log!(
                "[Knowhow] Missing or invalid frontmatter in {}",
                path.display()
            );
            return None;
        }
    };
    Some(Knowhow {
        id: id.to_string(),
        name,
        description,
        content,
    })
}

/// Same path-traversal guard as [`KnowhowStore::load`]; ids that fail this
/// can never resolve to a real file regardless of which dir we look in.
/// Delegates to the canonical [`super::is_path_traversal`] so the rule can
/// never drift from the engine-wide guard.
fn is_safe_id(id: &str) -> bool {
    !super::is_path_traversal(id)
}

/// Resolve `<app_id>/<rest>` to `<apps>/<app_id>/knowhow/<rest>.md`. Returns
/// `None` for ids that aren't app-scoped (no `/`), have empty segments, or
/// fail [`is_safe_id`]. Existence is NOT checked — callers do that.
fn app_scoped_knowhow_path(apps_dir: &Path, id: &str) -> Option<PathBuf> {
    if !is_safe_id(id) {
        return None;
    }
    let (app_id, rest) = id.split_once('/')?;
    // `rest` must also pass the path-traversal guard: an absolute or
    // backslash-prefixed `rest` would let `Path::join` replace the apps_dir
    // prefix and escape to the filesystem root (e.g. `foo//bar` splits to
    // ("foo", "/bar"), and `apps_dir.join("/bar.md")` becomes `/bar.md`).
    if app_id.is_empty() || rest.is_empty() || !is_safe_id(rest) {
        return None;
    }
    Some(
        apps_dir
            .join(app_id)
            .join("knowhow")
            .join(format!("{}.md", rest)),
    )
}

/// Resolve `triggers/<slug>/<rest>` to `<triggers>/<slug>/knowhow/<rest>.md`.
/// Mirrors [`app_scoped_knowhow_path`] one level deeper — the leading
/// `triggers/` prefix disambiguates the trigger namespace from the bare
/// `<app>/<rest>` namespace. Returns `None` for ids that aren't trigger-scoped,
/// have empty segments, or fail the same path-traversal guards.
fn trigger_scoped_knowhow_path(triggers_dir: &Path, id: &str) -> Option<PathBuf> {
    if !is_safe_id(id) {
        return None;
    }
    let stripped = id.strip_prefix("triggers/")?;
    let (slug, rest) = stripped.split_once('/')?;
    // Same guard as the app-scoped helper: an absolute / backslash-prefixed
    // `rest` would let `Path::join` escape the triggers_dir prefix.
    if slug.is_empty() || rest.is_empty() || !is_safe_id(rest) {
        return None;
    }
    Some(
        triggers_dir
            .join(slug)
            .join("knowhow")
            .join(format!("{}.md", rest)),
    )
}

/// Load referenced know-how for LLM context.
///
/// IDs prefixed with `system-knowhow/` resolve against the engine-shipped
/// reference dir (`<repo>/system-knowhow/`) and are tagged `[SYSTEM-KNOWHOW: ...]`.
/// Bare IDs resolve via shared+local user knowhow with local taking priority and
/// are tagged `[KNOW-HOW: ...]`. Returns formatted sections joined for prompt
/// injection, or empty string if nothing matched.
pub fn load_knowhow_sections_merged(
    dirs: &KnowhowDirs,
    system_dir: Option<&Path>,
    ids: &[String],
) -> String {
    if ids.is_empty() {
        return String::new();
    }
    let mut sections = Vec::new();
    for id in ids {
        if let Some(sys_id) = id.strip_prefix(SYSTEM_KNOWHOW_PREFIX) {
            let Some(dir) = system_dir else {
                log!(
                    "[Knowhow] system-knowhow id '{}' requested but system_knowhow_dir is unavailable",
                    id
                );
                continue;
            };
            if let Some(kh) = SystemKnowhowStore::load(dir, sys_id) {
                sections.push(SystemKnowhowStore::format_section(&kh));
            }
        } else if let Some(kh) = KnowhowStore::load_with_fallback(dirs, id) {
            sections.push(kh.format_section());
        }
    }
    if sections.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", sections.join("\n\n"))
    }
}

/// Load and format a single know-how section by id.
///
/// `Some` of a formatted `[KNOW-HOW: ...]` or `[SYSTEM-KNOWHOW: ...]` block,
/// or `None` if the id resolves to nothing. The `load_knowhow` LLM tool
/// handler is implemented on top of this helper so any future callers get
/// identical formatted output.
pub fn load_one_knowhow_section(
    dirs: &KnowhowDirs,
    system_dir: Option<&Path>,
    id: &str,
) -> Option<String> {
    if let Some(sys_id) = id.strip_prefix(SYSTEM_KNOWHOW_PREFIX) {
        let dir = system_dir?;
        return SystemKnowhowStore::load(dir, sys_id)
            .map(|sd| SystemKnowhowStore::format_section(&sd));
    }
    KnowhowStore::load_with_fallback(dirs, id).map(|kh| kh.format_section())
}

/// Format the "knowhow not found" message that the `load_knowhow` LLM tool
/// surfaces to the model when an id doesn't resolve. Centralized so any
/// future callers stay byte-identical with the LLM tool's miss path.
pub fn knowhow_not_found_body(id: &str) -> String {
    if id.starts_with(SYSTEM_KNOWHOW_PREFIX) {
        format!(
            "System knowhow '{}' not found. Check the System Knowhow list in the system prompt for available IDs.",
            id
        )
    } else {
        format!(
            "Know-how '{}' not found. Use the know-how list in the system prompt to see available IDs.",
            id
        )
    }
}

/// True iff `body` is one of the not-found sentinels produced by
/// [`knowhow_not_found_body`]. The `load_knowhow` tool handler uses this to
/// avoid persisting miss responses into the per-thread loaded set — only
/// real docs belong there. Real hit bodies are formatted `[KNOW-HOW: …]` /
/// `[SYSTEM-KNOWHOW: …]` blocks (see [`Knowhow::format_section`] /
/// [`SystemKnowhowStore::format_section`]) and start with `[`, so the
/// distinctive `'<id>' not found.` substring suffices to discriminate.
pub fn is_not_found_body(body: &str) -> bool {
    (body.starts_with("System knowhow '") || body.starts_with("Know-how '"))
        && body.contains("' not found.")
}

/// Load all know-how files from an app's knowhow/ subdirectory (recursively).
/// Returns formatted sections for injection into the system prompt, or empty string if none found.
pub fn load_app_knowhow(knowhow_dir: &Path) -> String {
    if !knowhow_dir.exists() {
        return String::new();
    }

    let mut sections = Vec::new();
    for path in collect_md_files(knowhow_dir) {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                if let Some((name, _, body)) = parse_frontmatter(&text) {
                    sections.push(format!(
                        "[APP KNOW-HOW: {}]\n{}\n[END APP KNOW-HOW]",
                        name, body
                    ));
                } else {
                    log!(
                        "[Knowhow] Missing or invalid frontmatter in {}",
                        path.display()
                    );
                }
            }
            Err(e) => {
                log!("[Knowhow] Failed to read {}: {}", path.display(), e);
            }
        }
    }

    if sections.is_empty() {
        String::new()
    } else {
        format!("\n{}\n", sections.join("\n\n"))
    }
}

#[cfg(test)]
#[path = "knowhow_tests.rs"]
mod tests;
