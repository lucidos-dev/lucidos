//! Engine-shipped reference knowhow about how Lucidos itself works.
//!
//! Sourced exclusively from `<repo>/system-knowhow/` — never overrideable by a
//! workspace's local `data/knowhow/` or the shared `~/.lucidos/knowhow/`. The
//! LLM sees these with a `[SYSTEM-KNOWHOW: ...]` tag (vs. `[KNOW-HOW: ...]`
//! for user-curated knowhow) so it knows the source is authoritative.
//!
//! On-disk format and loading match knowhow exactly, so this module reuses
//! `KnowhowStore` for parsing and only adds the system-knowhow tag + the
//! [`is_system_knowhow_path`] predicate that gates read-only enforcement.

use std::path::Path;

use crate::core::knowhow::{Knowhow, KnowhowStore, KnowhowSummary};

pub struct SystemKnowhowStore;

impl SystemKnowhowStore {
    pub fn load_summaries(dir: &Path) -> Vec<KnowhowSummary> {
        KnowhowStore::load_summaries(dir)
    }

    pub fn load(dir: &Path, id: &str) -> Option<Knowhow> {
        KnowhowStore::load(dir, id)
    }

    /// Format a knowhow entry with the `[SYSTEM-KNOWHOW: ...]` tag for LLM context injection.
    pub fn format_section(doc: &Knowhow) -> String {
        format!(
            "[SYSTEM-KNOWHOW: {}]\n{}\n[END SYSTEM-KNOWHOW]",
            doc.name, doc.content
        )
    }
}

/// Whether a workspace-relative data path refers to engine-shipped read-only knowhow.
pub fn is_system_knowhow_path(data_path: &str) -> bool {
    data_path.starts_with("system-knowhow/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_doc(path: &std::path::Path, name: &str, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("---\nname: {}\n---\n{}", name, body)).unwrap();
    }

    #[test]
    fn load_summaries_lists_all_docs() {
        let tmp = tempfile::tempdir().unwrap();
        write_doc(&tmp.path().join("best-practices.md"), "Best Practices", "Body.");
        write_doc(&tmp.path().join("lucidos-cli.md"), "Lucidos CLI", "Body.");

        let ids: Vec<String> = SystemKnowhowStore::load_summaries(tmp.path())
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert!(ids.contains(&"best-practices".to_string()));
        assert!(ids.contains(&"lucidos-cli".to_string()));
    }

    #[test]
    fn load_returns_full_doc() {
        let tmp = tempfile::tempdir().unwrap();
        write_doc(&tmp.path().join("guide.md"), "Guide", "Full body content.");

        let doc = SystemKnowhowStore::load(tmp.path(), "guide").expect("doc should load");
        assert_eq!(doc.id, "guide");
        assert_eq!(doc.name, "Guide");
        assert_eq!(doc.content, "Full body content.");
    }

    #[test]
    fn format_section_uses_system_knowhow_tag() {
        let doc = Knowhow {
            id: "x".into(),
            name: "Lucidos CLI".into(),
            description: String::new(),
            content: "Body content.".into(),
        };
        let s = SystemKnowhowStore::format_section(&doc);
        assert!(s.starts_with("[SYSTEM-KNOWHOW: Lucidos CLI]\n"));
        assert!(s.ends_with("\n[END SYSTEM-KNOWHOW]"));
        assert!(s.contains("Body content."));
        assert!(!s.contains("KNOW-HOW"));
    }

    #[test]
    fn is_system_knowhow_path_detects_prefix() {
        assert!(is_system_knowhow_path("system-knowhow/best-practices.md"));
        assert!(is_system_knowhow_path("system-knowhow/scripts/list.sh"));
        assert!(!is_system_knowhow_path("artifacts/notes.md"));
        assert!(!is_system_knowhow_path("knowhow/lucidos/best-practices.md"));
    }

    /// Files without `---\nname: ...\n---` are silently dropped at load time,
    /// so `load_knowhow("system-knowhow/<id>")` returns missing and the LLM
    /// concludes the file doesn't exist.
    #[test]
    fn shipped_system_knowhow_files_all_parse() {
        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
        let dir = repo.join("system-knowhow");
        let summary_ids: std::collections::HashSet<String> =
            SystemKnowhowStore::load_summaries(&dir)
                .into_iter()
                .map(|s| s.id)
                .collect();
        let missing: Vec<String> = crate::core::knowhow::collect_md_files(&dir)
            .into_iter()
            .filter_map(|path| crate::core::knowhow::id_from_path(&dir, &path))
            .filter(|id| !summary_ids.contains(id))
            .collect();
        assert!(
            missing.is_empty(),
            "system-knowhow files missing valid `---\\nname: ...\\n---` frontmatter \
             (load_knowhow returns missing for these): {:?}",
            missing
        );
    }
}
