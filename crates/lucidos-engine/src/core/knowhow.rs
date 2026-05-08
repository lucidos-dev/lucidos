use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::core::system_knowhow::SystemKnowhowStore;
use crate::memory::{cosine_similarity, EmbeddingProvider};

/// Prefix that routes a knowhow ID to engine-shipped reference knowhow at
/// `<repo>/system-knowhow/`. IDs without this prefix come from the
/// workspace-local or shared user-curated knowhow dirs.
pub const SYSTEM_KNOWHOW_PREFIX: &str = "system-knowhow/";

/// Bundles the user-curated knowhow search directories.
/// Priority (highest wins): local > shared > apps.
///
/// `apps` is the workspace's `data/apps/` root. App-scoped knowhow ids of the
/// form `<app_id>/<rest>` resolve to `<apps>/<app_id>/knowhow/<rest>.md` as the
/// last fallback — bare-id local + shared still win when both exist. The
/// engine surfaces app-scoped ids in the system prompt's Know-how list, so
/// validators and loaders that work with `KnowhowDirs` must accept them.
///
/// Engine-shipped reference knowhow lives in `<repo>/system-knowhow/` and is
/// loaded separately via [`crate::core::SystemKnowhowStore`] — it cannot be
/// overridden by a workspace.
pub struct KnowhowDirs {
    pub shared: Option<PathBuf>,
    pub local: PathBuf,
    pub apps: Option<PathBuf>,
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
fn parse_frontmatter(text: &str) -> Option<(String, String, String)> {
    if !text.starts_with("---") {
        return None;
    }

    let parts: Vec<&str> = text.splitn(3, "---").collect();
    if parts.len() < 3 {
        return None;
    }

    let frontmatter = parts[1].trim();
    let body = parts[2].trim_start_matches('\n').to_string();

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

fn collect_md_files(dir: &Path) -> Vec<PathBuf> {
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

/// e.g., `knowhow/lucidos/cross-workspace.md` → `lucidos/cross-workspace`
fn id_from_path(root: &Path, path: &Path) -> Option<String> {
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
    /// Priority (highest wins): local > shared > app-scoped.
    /// App-scoped: an id of the form `<app_id>/<rest>` falls back to
    /// `<apps>/<app_id>/knowhow/<rest>.md` — only consulted if the bare id
    /// matched neither local nor shared, so a top-level file with the same
    /// id-shape always wins.
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
    }

    /// Load full content for a specific know-how file.
    /// The id may contain forward slashes for files in subdirectories (e.g., "lucidos/cross-workspace").
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

/// Format the user-facing error for a "knowhow id doesn't resolve" failure.
/// Shared between the HTTP trigger handler and the LLM `create_trigger` /
/// `update_trigger` tool so the wording stays in lockstep across surfaces.
pub fn format_missing_knowhow_message(missing: &[String]) -> String {
    format!(
        "Knowhow not found: {}. IDs are paths under data/knowhow/ (or system-knowhow/) without .md \
         — include subdirectories (e.g. lucidos-ops/nightly-pipeline-trigger).",
        missing.join(", ")
    )
}

/// Returns the subset of `ids` that do NOT resolve to an existing knowhow
/// file. An empty Vec means every id resolves. Resolution mirrors
/// [`load_knowhow_sections_merged`]: `system-knowhow/X` looks in `system_dir`,
/// bare ids fall back local → shared, then `<app>/<rest>` ids fall back to
/// the workspace's `apps/<app>/knowhow/<rest>.md`.
///
/// Order is preserved so callers can show users the offending ids verbatim.
/// Existence-only — never reads file bodies — so it's cheap on the trigger
/// fire path and the LLM tool turn.
pub fn missing_knowhow_ids(
    dirs: &KnowhowDirs,
    system_dir: Option<&Path>,
    ids: &[String],
) -> Vec<String> {
    let mut missing = Vec::new();
    for id in ids {
        if !id_resolves(dirs, system_dir, id) {
            missing.push(id.clone());
        }
    }
    missing
}

/// Same path-traversal guard as [`KnowhowStore::load`]; ids that fail this
/// can never resolve to a real file regardless of which dir we look in.
fn is_safe_id(id: &str) -> bool {
    !id.contains("..") && !id.starts_with('/') && !id.starts_with('\\')
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

fn id_resolves(dirs: &KnowhowDirs, system_dir: Option<&Path>, id: &str) -> bool {
    if !is_safe_id(id) {
        return false;
    }
    if let Some(sys_id) = id.strip_prefix(SYSTEM_KNOWHOW_PREFIX) {
        return system_dir
            .map(|dir| dir.join(format!("{}.md", sys_id)).is_file())
            .unwrap_or(false);
    }
    let filename = format!("{}.md", id);
    if dirs.local.join(&filename).is_file() {
        return true;
    }
    if dirs
        .shared
        .as_deref()
        .map(|s| s.join(&filename).is_file())
        .unwrap_or(false)
    {
        return true;
    }
    dirs.apps
        .as_deref()
        .and_then(|apps| app_scoped_knowhow_path(apps, id))
        .map(|p| p.is_file())
        .unwrap_or(false)
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

/// Semantic similarity threshold for knowhow discovery.
/// TODO(2026-04-21): recalibrate against real workspace data — orthogonal
/// English text scores ~0.75-0.85 with MultilingualE5Small, so 0.5 filters
/// nothing; `DISCOVERY_MAX_RESULTS` currently caps the noise.
const DISCOVERY_THRESHOLD: f32 = 0.5;
/// Maximum number of semantically matched knowhow results.
const DISCOVERY_MAX_RESULTS: usize = 5;

/// Discover relevant know-how using semantic similarity.
/// Embeds the message and knowhow descriptions, then returns the top matches
/// above a similarity threshold. Also includes any explicitly referenced IDs.
pub async fn discover_knowhow(
    message: &str,
    summaries: &[KnowhowSummary],
    explicit_refs: &[String],
    embedder: &dyn EmbeddingProvider,
) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();

    // Always include explicit refs
    for id in explicit_refs {
        if !result.contains(id) {
            result.push(id.clone());
        }
    }

    if message.is_empty() || summaries.is_empty() {
        return result;
    }

    // Filter out summaries already included via explicit refs
    let candidates: Vec<&KnowhowSummary> = summaries
        .iter()
        .filter(|s| !result.contains(&s.id))
        .collect();

    if candidates.is_empty() {
        return result;
    }

    let mut texts: Vec<&str> = Vec::with_capacity(1 + candidates.len());
    texts.push(message);
    for s in &candidates {
        texts.push(&s.description);
    }

    let embeddings = match embedder.embed_batch(&texts).await {
        Ok(e) => e,
        Err(e) => {
            log!("[Knowhow] Embedding failed, falling back to empty: {}", e);
            return result;
        }
    };

    let message_embedding = &embeddings[0];

    // Score each candidate by cosine similarity
    let mut scored: Vec<(&str, f32)> = candidates
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (
                s.id.as_str(),
                cosine_similarity(message_embedding, &embeddings[i + 1]),
            )
        })
        .filter(|(_, sim)| *sim >= DISCOVERY_THRESHOLD)
        .collect();

    // Sort by similarity descending
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    for (id, _) in scored.into_iter().take(DISCOVERY_MAX_RESULTS) {
        result.push(id.to_string());
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared_provider() -> &'static dyn EmbeddingProvider {
        crate::test_util::shared_embedder()
    }

    #[test]
    fn parse_knowhow_with_description() {
        let text = "---\nname: Panasonic\ndescription: Controls Panasonic heatpumps via Comfort Cloud API\n---\n## API\nBase URL...";
        let (name, description, _) = parse_frontmatter(text).unwrap();
        assert_eq!(name, "Panasonic");
        assert_eq!(
            description,
            "Controls Panasonic heatpumps via Comfort Cloud API"
        );
    }

    #[test]
    fn parse_knowhow_derives_description_from_body() {
        let text = "---\nname: Panasonic\n---\n# Heatpump API\nControls and monitors heatpumps.\nMore details.";
        let (name, description, _) = parse_frontmatter(text).unwrap();
        assert_eq!(name, "Panasonic");
        assert_eq!(description, "Panasonic: Controls and monitors heatpumps.");
    }

    #[test]
    fn parse_knowhow_derives_description_skips_headings() {
        let text = "---\nname: Calendar\n---\n# Google Calendar\n\n## Purpose\n- Show events from imported calendars";
        let (name, description, _) = parse_frontmatter(text).unwrap();
        assert_eq!(name, "Calendar");
        assert_eq!(
            description,
            "Calendar: - Show events from imported calendars"
        );
    }

    #[test]
    fn parse_knowhow_name_only_fallback() {
        let text = "---\nname: Empty Doc\n---\n# Just a heading\n";
        let (name, description, _) = parse_frontmatter(text).unwrap();
        assert_eq!(name, "Empty Doc");
        assert_eq!(description, "Empty Doc");
    }

    #[test]
    fn parse_ignores_legacy_domains_and_keywords() {
        let text = "---\nname: Panasonic\ndomains:\n  - heatpump\n  - panasonic\n---\n# Heatpump API\nControls heatpumps.";
        let (name, description, _) = parse_frontmatter(text).unwrap();
        assert_eq!(name, "Panasonic");
        // Legacy domains are ignored; description is derived from body
        assert_eq!(description, "Panasonic: Controls heatpumps.");
    }

    #[test]
    fn summary_excludes_content() {
        let summary = KnowhowSummary {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: "Test description".to_string(),
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(!json.contains("content"));
    }

    #[tokio::test]
    async fn discover_semantic_match() {
        let provider = shared_provider();
        let summaries = vec![KnowhowSummary {
            id: "heatpump".to_string(),
            name: "Heatpump".to_string(),
            description: "Controls and monitors Panasonic heatpumps via Comfort Cloud API"
                .to_string(),
        }];
        let result = discover_knowhow(
            "What is the heatpump temperature?",
            &summaries,
            &[],
            provider,
        )
        .await;
        assert!(
            result.contains(&"heatpump".to_string()),
            "should match heatpump semantically"
        );
    }

    #[tokio::test]
    async fn discover_ranks_related_above_unrelated() {
        let provider = shared_provider();
        let description = "Controls and monitors Panasonic heatpumps via Comfort Cloud API";
        let related = "What is the heatpump temperature?";
        let unrelated = "How to bake a chocolate cake with dark chocolate ganache";

        let embeddings = provider
            .embed_batch(&[description, related, unrelated])
            .await
            .expect("embedding should succeed");

        let related_sim = cosine_similarity(&embeddings[0], &embeddings[1]);
        let unrelated_sim = cosine_similarity(&embeddings[0], &embeddings[2]);

        assert!(
            related_sim > unrelated_sim + 0.02,
            "embedder should rank related above unrelated by a clear margin: \
             related={related_sim}, unrelated={unrelated_sim}"
        );
    }

    #[tokio::test]
    async fn discover_includes_explicit_refs() {
        let provider = shared_provider();
        let summaries = vec![KnowhowSummary {
            id: "heatpump".to_string(),
            name: "Heatpump".to_string(),
            description: "Controls Panasonic heatpumps".to_string(),
        }];
        let result = discover_knowhow(
            "unrelated message about cooking",
            &summaries,
            &["heatpump".to_string()],
            provider,
        )
        .await;
        assert_eq!(result, vec!["heatpump"]);
    }

    #[tokio::test]
    async fn discover_deduplicates_explicit_and_semantic() {
        let provider = shared_provider();
        let summaries = vec![KnowhowSummary {
            id: "heatpump".to_string(),
            name: "Heatpump".to_string(),
            description: "Controls and monitors Panasonic heatpumps via Comfort Cloud API"
                .to_string(),
        }];
        let result = discover_knowhow(
            "Check the heatpump temperature",
            &summaries,
            &["heatpump".to_string()],
            provider,
        )
        .await;
        assert_eq!(result, vec!["heatpump"]);
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn discover_ranks_by_relevance() {
        let provider = shared_provider();
        let summaries = vec![
            KnowhowSummary {
                id: "heatpump".to_string(),
                name: "Heatpump".to_string(),
                description: "Controls and monitors Panasonic heatpumps via Comfort Cloud API"
                    .to_string(),
            },
            KnowhowSummary {
                id: "calendar".to_string(),
                name: "Google Calendar".to_string(),
                description:
                    "Google Calendar integration for viewing and managing events and schedules"
                        .to_string(),
            },
        ];
        let result = discover_knowhow(
            "What is the heatpump temperature set to?",
            &summaries,
            &[],
            provider,
        )
        .await;
        // Heatpump should be the top match
        assert!(!result.is_empty(), "should have at least one match");
        assert_eq!(result[0], "heatpump", "heatpump should be ranked first");
    }

    #[tokio::test]
    async fn discover_empty_message_returns_only_explicit() {
        let provider = shared_provider();
        let summaries = vec![KnowhowSummary {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: "Test description".to_string(),
        }];
        let result = discover_knowhow("", &summaries, &["explicit".to_string()], provider).await;
        assert_eq!(result, vec!["explicit"]);
    }

    fn write_knowhow_file(path: &std::path::Path, name: &str, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("---\nname: {}\n---\n{}", name, body)).unwrap();
    }

    #[test]
    fn load_summaries_discovers_files_in_subdirectories() {
        let tmp = tempfile::tempdir().unwrap();
        let kh = tmp.path().join("knowhow");

        write_knowhow_file(&kh.join("top.md"), "Top Level", "Top-level content.");
        write_knowhow_file(
            &kh.join("lucidos").join("nested.md"),
            "Nested",
            "Nested content.",
        );
        write_knowhow_file(
            &kh.join("lucidos").join("deep").join("deep-file.md"),
            "Deep",
            "Deep content.",
        );

        let summaries = KnowhowStore::load_summaries(&kh);
        let ids: Vec<&str> = summaries.iter().map(|s| s.id.as_str()).collect();

        assert!(
            ids.contains(&"top"),
            "should find top-level file, got: {:?}",
            ids
        );
        assert!(
            ids.iter().any(|id| id.contains("nested")),
            "should find nested file, got: {:?}",
            ids
        );
        assert!(
            ids.iter().any(|id| id.contains("deep-file")),
            "should find deeply nested file, got: {:?}",
            ids
        );
        assert_eq!(summaries.len(), 3);
    }

    #[test]
    fn load_by_id_finds_file_in_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        let kh = tmp.path().join("knowhow");

        write_knowhow_file(
            &kh.join("lucidos").join("nested.md"),
            "Nested Doc",
            "Nested doc content.",
        );

        let summaries = KnowhowStore::load_summaries(&kh);
        assert_eq!(summaries.len(), 1);
        let id = &summaries[0].id;

        let loaded = KnowhowStore::load(&kh, id);
        assert!(loaded.is_some(), "should load nested file by id '{}'", id);
        assert_eq!(loaded.unwrap().name, "Nested Doc");
    }

    #[test]
    fn load_summary_has_description() {
        let tmp = tempfile::tempdir().unwrap();
        let kh = tmp.path().join("knowhow");

        write_knowhow_file(
            &kh.join("test.md"),
            "Test Doc",
            "This is the first paragraph.",
        );

        let summaries = KnowhowStore::load_summaries(&kh);
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].description,
            "Test Doc: This is the first paragraph."
        );
    }

    #[test]
    fn load_summary_uses_frontmatter_description() {
        let tmp = tempfile::tempdir().unwrap();
        let kh = tmp.path().join("knowhow");

        std::fs::create_dir_all(&kh).unwrap();
        std::fs::write(
            kh.join("test.md"),
            "---\nname: Test\ndescription: Custom description from frontmatter\n---\nBody content.",
        )
        .unwrap();

        let summaries = KnowhowStore::load_summaries(&kh);
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].description,
            "Custom description from frontmatter"
        );
    }

    fn dirs(shared: Option<&std::path::Path>, local: &std::path::Path) -> KnowhowDirs {
        KnowhowDirs {
            shared: shared.map(|p| p.to_path_buf()),
            local: local.to_path_buf(),
            apps: None,
        }
    }

    fn dirs_with_apps(
        shared: Option<&std::path::Path>,
        local: &std::path::Path,
        apps: &std::path::Path,
    ) -> KnowhowDirs {
        KnowhowDirs {
            shared: shared.map(|p| p.to_path_buf()),
            local: local.to_path_buf(),
            apps: Some(apps.to_path_buf()),
        }
    }

    // --- Task 1: load_merged_summaries ---

    #[test]
    fn load_merged_summaries_local_overrides_shared() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("shared");
        let local = tmp.path().join("local");

        write_knowhow_file(
            &shared.join("google-calendar.md"),
            "Google Calendar (shared)",
            "Shared version.",
        );
        write_knowhow_file(
            &shared.join("lucidos.md"),
            "Lucidos (shared)",
            "Shared Lucidos knowhow.",
        );

        write_knowhow_file(
            &local.join("google-calendar.md"),
            "Google Calendar (local)",
            "Local version.",
        );
        write_knowhow_file(&local.join("heatpump.md"), "Heatpump", "Heatpump content.");

        let summaries = KnowhowStore::load_merged_summaries(&dirs(Some(&shared), &local));
        let ids: Vec<&str> = summaries.iter().map(|s| s.id.as_str()).collect();

        assert_eq!(
            summaries.len(),
            3,
            "should have 3 unique entries: {:?}",
            ids
        );
        assert!(ids.contains(&"google-calendar"));
        assert!(ids.contains(&"lucidos"));
        assert!(ids.contains(&"heatpump"));

        let gc = summaries
            .iter()
            .find(|s| s.id == "google-calendar")
            .unwrap();
        assert!(
            gc.name.contains("local"),
            "local should override shared, got: {}",
            gc.name
        );
    }

    #[test]
    fn load_merged_summaries_shared_none_is_local_only() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        write_knowhow_file(&local.join("test.md"), "Test", "Content.");

        let summaries = KnowhowStore::load_merged_summaries(&dirs(None, &local));
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "test");
    }

    // --- load_with_fallback ---

    #[test]
    fn load_with_fallback_prefers_local() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("shared");
        let local = tmp.path().join("local");

        write_knowhow_file(&shared.join("test.md"), "Shared Test", "Shared content.");
        write_knowhow_file(&local.join("test.md"), "Local Test", "Local content.");

        let kh = KnowhowStore::load_with_fallback(&dirs(Some(&shared), &local), "test").unwrap();
        assert_eq!(kh.name, "Local Test");
    }

    #[test]
    fn load_with_fallback_falls_back_to_shared() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("shared");
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();

        write_knowhow_file(
            &shared.join("only-shared.md"),
            "Only Shared",
            "Shared content.",
        );

        let kh = KnowhowStore::load_with_fallback(&dirs(Some(&shared), &local), "only-shared")
            .unwrap();
        assert_eq!(kh.name, "Only Shared");
    }

    #[test]
    fn load_with_fallback_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();

        let kh = KnowhowStore::load_with_fallback(&dirs(None, &local), "missing");
        assert!(kh.is_none());
    }

    // --- load_knowhow_sections_merged ---

    #[test]
    fn load_knowhow_sections_merged_uses_both_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("shared");
        let local = tmp.path().join("local");

        write_knowhow_file(
            &shared.join("shared-ref.md"),
            "Shared Ref",
            "Shared reference content.",
        );
        write_knowhow_file(
            &local.join("local-ref.md"),
            "Local Ref",
            "Local reference content.",
        );

        let ids = vec!["shared-ref".to_string(), "local-ref".to_string()];
        let sections = load_knowhow_sections_merged(&dirs(Some(&shared), &local), None, &ids);
        assert!(
            sections.contains("Shared Ref"),
            "should include shared knowhow"
        );
        assert!(
            sections.contains("Local Ref"),
            "should include local knowhow"
        );
    }

    #[test]
    fn load_knowhow_sections_merged_local_overrides_shared() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("shared");
        let local = tmp.path().join("local");

        write_knowhow_file(
            &shared.join("overlap.md"),
            "Shared Version",
            "Shared content.",
        );
        write_knowhow_file(&local.join("overlap.md"), "Local Version", "Local content.");

        let ids = vec!["overlap".to_string()];
        let sections = load_knowhow_sections_merged(&dirs(Some(&shared), &local), None, &ids);
        assert!(
            sections.contains("Local Version"),
            "local should win over shared"
        );
        assert!(
            !sections.contains("Shared Version"),
            "shared should not appear when local exists"
        );
    }

    #[test]
    fn load_knowhow_sections_merged_loads_system_knowhow_with_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        let system = tmp.path().join("system");

        write_knowhow_file(
            &system.join("best-practices.md"),
            "Best Practices",
            "System body content.",
        );

        let ids = vec!["system-knowhow/best-practices".to_string()];
        let sections =
            load_knowhow_sections_merged(&dirs(None, &local), Some(&system), &ids);
        assert!(
            sections.contains("[SYSTEM-KNOWHOW: Best Practices]"),
            "should tag with SYSTEM-KNOWHOW, got: {}",
            sections
        );
        assert!(sections.contains("System body content."));
    }

    #[test]
    fn load_knowhow_sections_merged_mixes_system_and_user_knowhow() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        let system = tmp.path().join("system");

        write_knowhow_file(&local.join("user-doc.md"), "User Doc", "User body.");
        write_knowhow_file(&system.join("sys-doc.md"), "Sys Doc", "Sys body.");

        let ids = vec![
            "system-knowhow/sys-doc".to_string(),
            "user-doc".to_string(),
        ];
        let sections = load_knowhow_sections_merged(&dirs(None, &local), Some(&system), &ids);
        assert!(sections.contains("[SYSTEM-KNOWHOW: Sys Doc]"));
        assert!(sections.contains("[KNOW-HOW: User Doc]"));
    }

    #[test]
    fn load_knowhow_sections_merged_handles_missing_system_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();

        let ids = vec!["system-knowhow/anything".to_string()];
        let sections = load_knowhow_sections_merged(&dirs(None, &local), None, &ids);
        assert_eq!(sections, "", "missing system_dir should drop system ids silently");
    }

    // --- missing_knowhow_ids ---

    #[test]
    fn missing_knowhow_ids_returns_empty_when_all_resolve() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        let system = tmp.path().join("system");
        write_knowhow_file(&local.join("nested").join("foo.md"), "Foo", "Body.");
        write_knowhow_file(&system.join("guide.md"), "Guide", "Sys body.");

        let ids = vec![
            "nested/foo".to_string(),
            "system-knowhow/guide".to_string(),
        ];
        let missing = missing_knowhow_ids(&dirs(None, &local), Some(&system), &ids);
        assert!(missing.is_empty(), "all ids should resolve, got: {:?}", missing);
    }

    #[test]
    fn missing_knowhow_ids_flags_bare_id_in_subdirectory() {
        // The reported bug: a knowhow at `lucidos-ops/nightly-pipeline-trigger.md`
        // is referenced as `nightly-pipeline-trigger` (bare). The validator must
        // catch this so the trigger save fails instead of 404ing later.
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        write_knowhow_file(
            &local.join("lucidos-ops").join("nightly-pipeline-trigger.md"),
            "Nightly",
            "Body.",
        );

        let ids = vec!["nightly-pipeline-trigger".to_string()];
        let missing = missing_knowhow_ids(&dirs(None, &local), None, &ids);
        assert_eq!(missing, vec!["nightly-pipeline-trigger".to_string()]);
    }

    #[test]
    fn missing_knowhow_ids_flags_missing_system_knowhow() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        let system = tmp.path().join("system");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(&system).unwrap();

        let ids = vec!["system-knowhow/does-not-exist".to_string()];
        let missing = missing_knowhow_ids(&dirs(None, &local), Some(&system), &ids);
        assert_eq!(missing, vec!["system-knowhow/does-not-exist".to_string()]);
    }

    #[test]
    fn missing_knowhow_ids_preserves_input_order() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        write_knowhow_file(&local.join("real.md"), "Real", "Body.");

        let ids = vec![
            "ghost-1".to_string(),
            "real".to_string(),
            "ghost-2".to_string(),
        ];
        let missing = missing_knowhow_ids(&dirs(None, &local), None, &ids);
        assert_eq!(missing, vec!["ghost-1".to_string(), "ghost-2".to_string()]);
    }

    #[test]
    fn missing_knowhow_ids_falls_back_to_shared() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("shared");
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        write_knowhow_file(&shared.join("only-shared.md"), "Only Shared", "Body.");

        let ids = vec!["only-shared".to_string()];
        let missing = missing_knowhow_ids(&dirs(Some(&shared), &local), None, &ids);
        assert!(missing.is_empty(), "shared fallback should resolve, got: {:?}", missing);
    }

    // --- App-scoped knowhow resolution ---
    //
    // The reported bug: triggers reference knowhow ids of the form `<app>/<rest>`
    // (e.g. `finn-jobs/finn-search-workflow`) which the engine surfaces in the
    // system prompt's Know-how list. The validator and trigger fire path must
    // resolve these against `<workspace>/data/apps/<app>/knowhow/<rest>.md` —
    // not just the top-level local/shared dirs — or the trigger save fails and
    // pre-existing rows error at fire time.

    #[test]
    fn missing_knowhow_ids_resolves_app_scoped_id() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        let apps = tmp.path().join("apps");
        write_knowhow_file(
            &apps.join("finn-jobs").join("knowhow").join("finn-search-workflow.md"),
            "Finn search workflow",
            "How to search Finn.",
        );

        let ids = vec!["finn-jobs/finn-search-workflow".to_string()];
        let missing = missing_knowhow_ids(&dirs_with_apps(None, &local, &apps), None, &ids);
        assert!(
            missing.is_empty(),
            "app-scoped id should resolve, got: {:?}",
            missing
        );
    }

    #[test]
    fn missing_knowhow_ids_flags_unknown_app_scoped_id() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        let apps = tmp.path().join("apps");
        std::fs::create_dir_all(&apps).unwrap();

        let ids = vec!["finn-jobs/does-not-exist".to_string()];
        let missing = missing_knowhow_ids(&dirs_with_apps(None, &local, &apps), None, &ids);
        assert_eq!(missing, vec!["finn-jobs/does-not-exist".to_string()]);
    }

    #[test]
    fn load_knowhow_sections_merged_loads_app_scoped_knowhow() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        let apps = tmp.path().join("apps");
        write_knowhow_file(
            &apps.join("morning-log").join("knowhow").join("morning-log-data.md"),
            "Morning log data",
            "Data layout for morning log.",
        );

        let ids = vec!["morning-log/morning-log-data".to_string()];
        let sections = load_knowhow_sections_merged(
            &dirs_with_apps(None, &local, &apps),
            None,
            &ids,
        );
        assert!(
            !sections.is_empty(),
            "app-scoped section should be loaded, got empty"
        );
        assert!(
            sections.contains("Morning log data"),
            "section should reference the file's name, got: {}",
            sections
        );
        assert!(
            sections.contains("Data layout for morning log."),
            "section should include body, got: {}",
            sections
        );
    }

    #[test]
    fn load_with_fallback_loads_app_scoped_knowhow() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        let apps = tmp.path().join("apps");
        write_knowhow_file(
            &apps.join("foo").join("knowhow").join("bar.md"),
            "Foo Bar",
            "Body.",
        );

        let kh =
            KnowhowStore::load_with_fallback(&dirs_with_apps(None, &local, &apps), "foo/bar")
                .expect("app-scoped id should load");
        assert_eq!(kh.name, "Foo Bar");
    }

    #[test]
    fn load_with_fallback_prefers_local_over_app_scoped() {
        // If a top-level knowhow file shares the same id-shape as an app-scoped
        // one (e.g. local has `foo/bar.md`, apps also has `foo/knowhow/bar.md`),
        // the bare-id local match wins per the documented lookup order.
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        let apps = tmp.path().join("apps");
        write_knowhow_file(&local.join("foo").join("bar.md"), "Local Foo Bar", "Local.");
        write_knowhow_file(
            &apps.join("foo").join("knowhow").join("bar.md"),
            "App Foo Bar",
            "App.",
        );

        let kh =
            KnowhowStore::load_with_fallback(&dirs_with_apps(None, &local, &apps), "foo/bar")
                .expect("should load");
        assert_eq!(kh.name, "Local Foo Bar", "local must win over app-scoped");
    }

    #[test]
    fn id_resolves_rejects_traversal_in_app_scoped() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        let apps = tmp.path().join("apps");
        // Even if the file existed, the path-traversal guard must reject `..`.
        write_knowhow_file(
            &apps.join("foo").join("knowhow").join("bar.md"),
            "Foo Bar",
            "Body.",
        );

        let ids = vec!["../escape/bar".to_string()];
        let missing = missing_knowhow_ids(&dirs_with_apps(None, &local, &apps), None, &ids);
        assert_eq!(missing, vec!["../escape/bar".to_string()]);
    }

    // app_scoped_knowhow_path is the security boundary — an absolute or
    // backslash-prefixed `rest` segment would let `Path::join("/etc/passwd.md")`
    // replace the apps_dir prefix and escape to the filesystem root. The
    // outer is_safe_id only sees the full id (which doesn't start with `/`),
    // so the splitter has to re-validate `rest`.

    #[test]
    fn app_scoped_path_rejects_double_slash_escape() {
        // `foo//bar` splits to ("foo", "/bar"); `/bar.md` is absolute on Unix.
        let apps = std::path::PathBuf::from("/tmp/lucidos-test/apps");
        assert!(
            app_scoped_knowhow_path(&apps, "foo//bar").is_none(),
            "double-slash id must not produce a path",
        );
    }

    #[test]
    fn app_scoped_path_rejects_backslash_escape() {
        let apps = std::path::PathBuf::from("/tmp/lucidos-test/apps");
        assert!(
            app_scoped_knowhow_path(&apps, "foo/\\escape").is_none(),
            "backslash-prefixed rest must not produce a path",
        );
    }

    #[test]
    fn app_scoped_path_rejects_traversal_in_rest() {
        // Caught by the outer is_safe_id (`..` anywhere), but assert the
        // contract directly so a future refactor doesn't lose this behavior.
        let apps = std::path::PathBuf::from("/tmp/lucidos-test/apps");
        assert!(app_scoped_knowhow_path(&apps, "foo/../bar").is_none());
    }

    #[test]
    fn app_scoped_path_builds_well_formed_path_for_safe_id() {
        let apps = std::path::PathBuf::from("/ws/data/apps");
        let p = app_scoped_knowhow_path(&apps, "finn-jobs/finn-search-workflow")
            .expect("safe id should produce a path");
        assert_eq!(
            p,
            std::path::PathBuf::from(
                "/ws/data/apps/finn-jobs/knowhow/finn-search-workflow.md"
            )
        );
    }
}
