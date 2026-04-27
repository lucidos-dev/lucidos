use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::memory::{cosine_similarity, EmbeddingProvider};

/// Bundles the user-curated knowhow search directories.
/// Priority (highest wins): local > shared.
///
/// Engine-shipped reference knowhow lives in `<repo>/system-knowhow/` and is
/// loaded separately via [`crate::core::SystemKnowhowStore`] — it cannot be
/// overridden by a workspace.
pub struct KnowhowDirs {
    pub shared: Option<PathBuf>,
    pub local: PathBuf,
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
    /// Priority (highest wins): local > shared.
    pub fn load_with_fallback(dirs: &KnowhowDirs, id: &str) -> Option<Knowhow> {
        Self::load(&dirs.local, id).or_else(|| {
            dirs.shared
                .as_deref()
                .and_then(|shared| Self::load(shared, id))
        })
    }

    /// Load full content for a specific know-how file.
    /// The id may contain forward slashes for files in subdirectories (e.g., "lucidos/cross-workspace").
    pub fn load(knowhow_dir: &Path, id: &str) -> Option<Knowhow> {
        // Validate: no path traversal
        if id.contains("..") || id.starts_with('/') || id.starts_with('\\') {
            log!("[Knowhow] Invalid id (path traversal): {}", id);
            return None;
        }
        let path = knowhow_dir.join(format!("{}.md", id));
        let text = match std::fs::read_to_string(&path) {
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
}

/// Load referenced know-how from shared and local directories, with local taking priority.
/// Returns formatted sections for LLM context, or empty string if none found.
pub fn load_knowhow_sections_merged(dirs: &KnowhowDirs, ids: &[String]) -> String {
    if ids.is_empty() {
        return String::new();
    }
    let mut sections = Vec::new();
    for id in ids {
        if let Some(kh) = KnowhowStore::load_with_fallback(dirs, id) {
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
        let sections = load_knowhow_sections_merged(&dirs(Some(&shared), &local), &ids);
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
        let sections = load_knowhow_sections_merged(&dirs(Some(&shared), &local), &ids);
        assert!(
            sections.contains("Local Version"),
            "local should win over shared"
        );
        assert!(
            !sections.contains("Shared Version"),
            "shared should not appear when local exists"
        );
    }
}
