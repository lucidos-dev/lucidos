//! Engine-shipped reference documentation about how CognOS itself works.
//!
//! Sourced exclusively from `<repo>/system-docs/` — never overrideable by a
//! workspace's local `data/knowhow/` or the shared `~/.cognos/knowhow/`. The
//! LLM sees these with a `[SYSTEM-DOC: ...]` tag (vs. `[KNOW-HOW: ...]` for
//! user-curated knowhow) so it knows the source is authoritative.
//!
//! On-disk format and loading match knowhow exactly, so this module reuses
//! `KnowhowStore` for parsing and only adds the system-doc tag + the
//! [`is_system_doc_path`] predicate that gates read-only enforcement.

use std::path::Path;

use crate::core::knowhow::{Knowhow, KnowhowStore, KnowhowSummary};

pub struct SystemDocsStore;

impl SystemDocsStore {
    pub fn load_summaries(dir: &Path) -> Vec<KnowhowSummary> {
        KnowhowStore::load_summaries(dir)
    }

    pub fn load(dir: &Path, id: &str) -> Option<Knowhow> {
        KnowhowStore::load(dir, id)
    }

    /// Format a doc with the `[SYSTEM-DOC: ...]` tag for LLM context injection.
    pub fn format_section(doc: &Knowhow) -> String {
        format!(
            "[SYSTEM-DOC: {}]\n{}\n[END SYSTEM-DOC]",
            doc.name, doc.content
        )
    }
}

/// Whether a workspace-relative data path refers to engine-shipped read-only docs.
pub fn is_system_doc_path(data_path: &str) -> bool {
    data_path.starts_with("system-docs/")
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
        write_doc(&tmp.path().join("cognos-cli.md"), "Cognos CLI", "Body.");

        let ids: Vec<String> = SystemDocsStore::load_summaries(tmp.path())
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert!(ids.contains(&"best-practices".to_string()));
        assert!(ids.contains(&"cognos-cli".to_string()));
    }

    #[test]
    fn load_returns_full_doc() {
        let tmp = tempfile::tempdir().unwrap();
        write_doc(&tmp.path().join("guide.md"), "Guide", "Full body content.");

        let doc = SystemDocsStore::load(tmp.path(), "guide").expect("doc should load");
        assert_eq!(doc.id, "guide");
        assert_eq!(doc.name, "Guide");
        assert_eq!(doc.content, "Full body content.");
    }

    #[test]
    fn format_section_uses_system_doc_tag() {
        let doc = Knowhow {
            id: "x".into(),
            name: "Cognos CLI".into(),
            description: String::new(),
            content: "Body content.".into(),
        };
        let s = SystemDocsStore::format_section(&doc);
        assert!(s.starts_with("[SYSTEM-DOC: Cognos CLI]\n"));
        assert!(s.ends_with("\n[END SYSTEM-DOC]"));
        assert!(s.contains("Body content."));
        assert!(!s.contains("KNOW-HOW"));
    }

    #[test]
    fn is_system_doc_path_detects_prefix() {
        assert!(is_system_doc_path("system-docs/best-practices.md"));
        assert!(is_system_doc_path("system-docs/scripts/list.sh"));
        assert!(!is_system_doc_path("artifacts/notes.md"));
        assert!(!is_system_doc_path("knowhow/cognos/best-practices.md"));
    }
}
