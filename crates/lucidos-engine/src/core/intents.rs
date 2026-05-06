use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    pub id: String,
    pub name: String,
    pub knowhow: Vec<String>,
    pub content: String,
}

pub struct IntentStore;

fn parse_frontmatter(text: &str) -> Option<(String, Vec<String>, String)> {
    super::parse_md_frontmatter(text, "knowhow")
}

/// Subdirectories within each app that contain intents.
const APP_INTENT_SUBDIRS: &[&str] = &["intents", "triggers"];

impl IntentStore {
    /// Load all intents from the workspace data directory.
    ///
    /// Discovers intents from:
    /// - `data/apps/*/intents/` → id: `{app}/{name}` (app intents)
    /// - `data/apps/*/triggers/` → id: `{app}/{name}` (app trigger intents)
    /// - `data/triggers/*/` → id: `{name}` (standalone trigger intents)
    pub fn load_all(data_dir: &Path) -> Vec<Intent> {
        let mut intents = Vec::new();

        // App-scoped intents and triggers
        let apps_dir = data_dir.join("apps");
        if apps_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&apps_dir) {
                for entry in entries.flatten() {
                    let app_id = entry.file_name().to_string_lossy().to_string();
                    for subdir in APP_INTENT_SUBDIRS {
                        let dir = entry.path().join(subdir);
                        if dir.is_dir() {
                            Self::collect_md_files(&dir, &app_id, &mut intents);
                        }
                    }
                }
            }
        }

        // Standalone triggers
        let triggers_dir = data_dir.join("triggers");
        if triggers_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&triggers_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        Self::collect_md_files(&path, "", &mut intents);
                    }
                }
            }
        }

        intents
    }

    /// Load a single intent by ID.
    ///
    /// For app-scoped intents (id contains '/'), searches:
    /// - `data/apps/{app}/intents/{name}.md`
    /// - `data/apps/{app}/triggers/{name}.md`
    ///
    /// For standalone intents, searches:
    /// - `data/triggers/{id}/{id}.md`
    /// - First `.md` in `data/triggers/{id}/`
    pub fn load(data_dir: &Path, id: &str) -> Option<Intent> {
        if id.contains("..") || id.starts_with('/') || id.starts_with('\\') {
            log!("[Intents] Rejected invalid intent id: {}", id);
            return None;
        }

        // App-scoped: id = "app-name/intent-name"
        if let Some((app_id, intent_name)) = id.split_once('/') {
            for subdir in APP_INTENT_SUBDIRS {
                let path = data_dir
                    .join("apps")
                    .join(app_id)
                    .join(subdir)
                    .join(format!("{}.md", intent_name));
                if let Some(intent) = Self::load_from_path(&path, id) {
                    return Some(intent);
                }
            }
            return None;
        }

        // Standalone trigger: data/triggers/{id}/{id}.md
        let trigger_dir = data_dir.join("triggers").join(id);
        if trigger_dir.is_dir() {
            let exact = trigger_dir.join(format!("{}.md", id));
            if let Some(intent) = Self::load_from_path(&exact, id) {
                return Some(intent);
            }
            // Fallback: first .md in the directory
            if let Ok(entries) = std::fs::read_dir(&trigger_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("md") {
                        if let Some(intent) = Self::load_from_path(&path, id) {
                            return Some(intent);
                        }
                    }
                }
            }
        }

        None
    }

    /// Collect .md files from a directory, prefixing IDs with app_id if non-empty.
    fn collect_md_files(dir: &Path, app_id: &str, intents: &mut Vec<Intent>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                log!(
                    "[Intents] Failed to read directory {}: {}",
                    dir.display(),
                    e
                );
                return;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let id = if app_id.is_empty() {
                    stem.to_string()
                } else {
                    format!("{}/{}", app_id, stem)
                };
                if let Some(intent) = Self::load_from_path(&path, &id) {
                    intents.push(intent);
                }
            }
        }
    }

    fn load_from_path(path: &Path, id: &str) -> Option<Intent> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                log!("[Intents] Failed to read {}: {}", path.display(), e);
                return None;
            }
        };

        let (name, knowhow, content) = match parse_frontmatter(&text) {
            Some(parsed) => parsed,
            None => {
                log!(
                    "[Intents] Missing or invalid frontmatter in {}",
                    path.display()
                );
                return None;
            }
        };

        Some(Intent {
            id: id.to_string(),
            name,
            knowhow,
            content,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_intent_with_single_knowhow() {
        let text = "---\nname: Job Scoring\nknowhow: job-board\n---\nCheck the job board.";
        let (name, knowhow, _) = parse_frontmatter(text).unwrap();
        assert_eq!(name, "Job Scoring");
        assert_eq!(knowhow, vec!["job-board"]);
    }

    #[test]
    fn parse_intent_with_list_knowhow() {
        let text =
            "---\nname: Heatpump Check\nknowhow:\n  - heatpump\n  - panasonic\n---\nCheck status.";
        let (name, knowhow, _) = parse_frontmatter(text).unwrap();
        assert_eq!(name, "Heatpump Check");
        assert_eq!(knowhow, vec!["heatpump", "panasonic"]);
    }

    #[test]
    fn parse_intent_no_knowhow() {
        let text = "---\nname: Simple Intent\n---\nDo something.";
        let (name, knowhow, _) = parse_frontmatter(text).unwrap();
        assert_eq!(name, "Simple Intent");
        assert!(knowhow.is_empty());
    }

    #[test]
    fn parse_intent_body_extracted() {
        let text = "---\nname: Test\n---\nThis is the body.\nWith multiple lines.";
        let (_, _, content) = parse_frontmatter(text).unwrap();
        assert_eq!(content, "This is the body.\nWith multiple lines.");
    }

    #[test]
    fn load_returns_none_for_missing() {
        let dir = std::env::temp_dir().join("lucidos_test_intents_missing");
        let _ = std::fs::create_dir_all(&dir);
        let result = IntentStore::load(&dir, "nonexistent");
        assert!(result.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_finds_standalone_trigger() {
        let dir = std::env::temp_dir().join("lucidos_test_standalone_trigger_intent");
        let _ = std::fs::remove_dir_all(&dir);
        let trigger_dir = dir.join("triggers").join("my-trigger");
        std::fs::create_dir_all(&trigger_dir).unwrap();
        std::fs::write(
            trigger_dir.join("my-trigger.md"),
            "---\nname: My Trigger\n---\nDo the thing.",
        )
        .unwrap();

        let intent = IntentStore::load(&dir, "my-trigger").unwrap();
        assert_eq!(intent.id, "my-trigger");
        assert_eq!(intent.name, "My Trigger");
        assert_eq!(intent.content, "Do the thing.");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_finds_app_intent() {
        let dir = std::env::temp_dir().join("lucidos_test_app_intent");
        let _ = std::fs::remove_dir_all(&dir);
        let intents_dir = dir.join("apps").join("my-app").join("intents");
        std::fs::create_dir_all(&intents_dir).unwrap();
        std::fs::write(
            intents_dir.join("my-recipe.md"),
            "---\nname: My Recipe\n---\nStep 1.",
        )
        .unwrap();

        let intent = IntentStore::load(&dir, "my-app/my-recipe").unwrap();
        assert_eq!(intent.id, "my-app/my-recipe");
        assert_eq!(intent.name, "My Recipe");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_finds_app_trigger() {
        let dir = std::env::temp_dir().join("lucidos_test_app_trigger_intent");
        let _ = std::fs::remove_dir_all(&dir);
        let triggers_dir = dir.join("apps").join("my-app").join("triggers");
        std::fs::create_dir_all(&triggers_dir).unwrap();
        std::fs::write(
            triggers_dir.join("daily-check.md"),
            "---\nname: Daily Check\n---\nCheck things.",
        )
        .unwrap();

        let intent = IntentStore::load(&dir, "my-app/daily-check").unwrap();
        assert_eq!(intent.id, "my-app/daily-check");
        assert_eq!(intent.name, "Daily Check");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_all_discovers_from_all_locations() {
        let dir = std::env::temp_dir().join("lucidos_test_load_all_intents");
        let _ = std::fs::remove_dir_all(&dir);

        // App intent
        let app_intents = dir.join("apps").join("app1").join("intents");
        std::fs::create_dir_all(&app_intents).unwrap();
        std::fs::write(app_intents.join("recipe.md"), "---\nname: Recipe\n---\nDo.").unwrap();

        // App trigger
        let app_triggers = dir.join("apps").join("app1").join("triggers");
        std::fs::create_dir_all(&app_triggers).unwrap();
        std::fs::write(app_triggers.join("daily.md"), "---\nname: Daily\n---\nRun.").unwrap();

        // Standalone trigger
        let trigger = dir.join("triggers").join("reminder");
        std::fs::create_dir_all(&trigger).unwrap();
        std::fs::write(
            trigger.join("reminder.md"),
            "---\nname: Reminder\n---\nRemind.",
        )
        .unwrap();

        let all = IntentStore::load_all(&dir);
        assert_eq!(all.len(), 3);

        let ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"app1/recipe"));
        assert!(ids.contains(&"app1/daily"));
        assert!(ids.contains(&"reminder"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
