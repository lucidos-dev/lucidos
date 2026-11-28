use std::path::{Path, PathBuf};

use crate::core::knowhow::{Knowhow, KnowhowListDepth, KnowhowStore, KnowhowSummary};
use crate::core::shipped_dir::{resolve_shipped_dir, ShippedDir};

pub struct SystemKnowhowStore;

impl SystemKnowhowStore {
    pub fn load_summaries(dir: &Path) -> Vec<KnowhowSummary> {
        KnowhowStore::load_summaries(dir, KnowhowListDepth::FilesOnly)
    }

    pub fn load(dir: &Path, id: &str) -> Option<Knowhow> {
        KnowhowStore::load(dir, id)
    }

    pub fn format_section(doc: &Knowhow) -> String {
        format!(
            "[SYSTEM-KNOWHOW: {}]\n{}\n[END SYSTEM-KNOWHOW]",
            doc.name, doc.content
        )
    }
}

pub fn is_system_knowhow_path(data_path: &str) -> bool {
    data_path.starts_with("system-knowhow/")
}

pub const SYSTEM_KNOWHOW_DIR: ShippedDir = ShippedDir {
    name: "system-knowhow",
    env_var: "LUCIDOS_SYSTEM_KNOWHOW_DIR",
    log_tag: "[Knowhow]",
    degrades: "the engine-shipped reference set is missing (load_knowhow('system-knowhow/…'), \
               GET /api/v1/knowhow, and the data-API read path all degrade)",
};

pub fn resolve_system_knowhow_dir(
    env_value: Option<&str>,
    repo_root: &Path,
    is_packaged: bool,
) -> (Option<PathBuf>, Option<String>) {
    resolve_shipped_dir(&SYSTEM_KNOWHOW_DIR, env_value, repo_root, is_packaged)
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
        write_doc(
            &tmp.path().join("best-practices.md"),
            "Best Practices",
            "Body.",
        );
        write_doc(&tmp.path().join("lucidos-cli.md"), "Lucidos CLI", "Body.");

        let ids: Vec<String> = SystemKnowhowStore::load_summaries(tmp.path())
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert!(ids.contains(&"best-practices".to_string()));
        assert!(ids.contains(&"lucidos-cli".to_string()));
    }

    #[test]
    fn load_summaries_lists_top_level_docs_only() {
        let tmp = tempfile::tempdir().unwrap();
        write_doc(&tmp.path().join("guide.md"), "Guide", "Body.");
        write_doc(
            &tmp.path().join("guide").join("section.md"),
            "Section",
            "Body.",
        );

        let ids: Vec<String> = SystemKnowhowStore::load_summaries(tmp.path())
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec!["guide".to_string()]);
        assert!(
            SystemKnowhowStore::load(tmp.path(), "guide/section").is_some(),
            "a reference still loads by its full id"
        );
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

    #[test]
    fn resolve_prefers_the_env_var_when_the_dir_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let staged = tmp.path().join("resources/system-knowhow");
        std::fs::create_dir_all(&staged).unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("system-knowhow")).unwrap();

        let (dir, warning) =
            resolve_system_knowhow_dir(Some(staged.to_str().unwrap()), &repo, true);
        assert_eq!(dir.as_deref(), Some(staged.as_path()));
        assert_eq!(warning, None, "clean resolution warns nothing");
    }

    #[test]
    fn resolve_env_set_but_missing_is_unavailable_and_warns_never_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("system-knowhow")).unwrap();
        let missing = tmp.path().join("resources/system-knowhow");

        let (dir, warning) =
            resolve_system_knowhow_dir(Some(missing.to_str().unwrap()), &repo, true);
        assert_eq!(
            dir, None,
            "a set-but-missing env var never falls back to repo_root"
        );
        let warning = warning.expect("a set-but-missing env dir must warn");
        assert!(warning.contains("LUCIDOS_SYSTEM_KNOWHOW_DIR"));
    }

    #[test]
    fn resolve_env_unset_uses_repo_root_and_is_quiet() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("system-knowhow")).unwrap();

        for packaged in [false, true] {
            let (dir, warning) = resolve_system_knowhow_dir(None, tmp.path(), packaged);
            assert_eq!(
                dir.as_deref(),
                Some(tmp.path().join("system-knowhow").as_path())
            );
            assert_eq!(warning, None, "repo-root hit warns nothing");
        }
    }

    #[test]
    fn resolve_empty_env_is_treated_as_unset() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("system-knowhow")).unwrap();
        let (dir, warning) = resolve_system_knowhow_dir(Some("   "), tmp.path(), false);
        assert_eq!(
            dir.as_deref(),
            Some(tmp.path().join("system-knowhow").as_path())
        );
        assert_eq!(warning, None);
    }

    #[test]
    fn resolve_preserves_whitespace_in_env_path() {
        let tmp = tempfile::tempdir().unwrap();
        let staged = tmp.path().join("staged ");
        std::fs::create_dir_all(&staged).unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();

        let (dir, warning) =
            resolve_system_knowhow_dir(Some(staged.to_str().unwrap()), &repo, true);
        assert_eq!(dir, Some(staged));
        assert_eq!(warning, None);
    }

    #[test]
    fn resolve_unavailable_is_quiet_in_dev_but_loud_when_packaged() {
        let empty = tempfile::tempdir().unwrap();

        let (dev_dir, dev_warning) = resolve_system_knowhow_dir(None, empty.path(), false);
        assert_eq!(dev_dir, None);
        assert_eq!(dev_warning, None, "dev without the dir is expected");

        let (pkg_dir, pkg_warning) = resolve_system_knowhow_dir(None, empty.path(), true);
        assert_eq!(pkg_dir, None);
        let pkg_warning = pkg_warning.expect("packaged + unresolvable must warn");
        assert!(pkg_warning.contains("LUCIDOS_SYSTEM_KNOWHOW_DIR"));
    }

    mod guard_tests {
        use super::*;

        #[test]
        fn system_knowhow_descriptions_stay_routing_sized() {
            const MAX_DESCRIPTION_CHARS: usize = 400;

            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
            let summaries = SystemKnowhowStore::load_summaries(&repo.join("system-knowhow"));
            assert!(
                !summaries.is_empty(),
                "no system-knowhow files loaded, the scan is broken rather than the \
             descriptions being clean"
            );

            let mut oversized = Vec::new();
            for kh in &summaries {
                assert!(
                    !kh.description.trim().is_empty(),
                    "system-knowhow/{} has an empty description, so nothing can route to it",
                    kh.id
                );
                if kh.description.chars().count() > MAX_DESCRIPTION_CHARS {
                    oversized.push(format!(
                        "  {:>5} chars  system-knowhow/{}",
                        kh.description.chars().count(),
                        kh.id
                    ));
                }
            }
            assert!(
                oversized.is_empty(),
                "system-knowhow description(s) over {MAX_DESCRIPTION_CHARS} chars. A \
             description carries coverage plus the phrases that should route to \
             the doc; the doc itself carries the detail:\n{}",
                oversized.join("\n")
            );
        }

        #[test]
        fn shipped_system_knowhow_files_all_parse() {
            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
            let dir = repo.join("system-knowhow");
            let missing: Vec<String> = crate::core::knowhow::collect_md_files(&dir)
                .into_iter()
                .filter_map(|path| crate::core::knowhow::id_from_path(&dir, &path))
                .filter(|id| SystemKnowhowStore::load(&dir, id).is_none())
                .collect();
            assert!(
                missing.is_empty(),
                "system-knowhow files missing valid `---\\nname: ...\\n---` frontmatter \
             (load_knowhow returns missing for these): {:?}",
                missing
            );
        }

        #[test]
        fn the_footprint_page_prompt_routes_to_the_audit_root() {
            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
            let page = std::fs::read_to_string(
                repo.join("crates/lucidos-app/src/components/settings/promptFootprint.ts"),
            )
            .expect("the page helpers exist");
            let prompt = page
                .lines()
                .find_map(|l| l.strip_prefix("export const PROMPT_FOOTPRINT_AUDIT_PROMPT = "))
                .expect("the page names its audit prompt")
                .to_lowercase();
            let root = SystemKnowhowStore::load_summaries(&repo.join("system-knowhow"))
                .into_iter()
                .find(|s| s.id == "workspace-audit")
                .expect("the audit root is listed");
            let description = root.description.to_lowercase();
            for word in ["audit", "workspace", "prompt footprint"] {
                assert!(prompt.contains(word), "the button's prompt drops {word:?}");
                assert!(
                    description.contains(word),
                    "the audit root's description drops {word:?}, so the button may not reach it"
                );
            }
        }

        #[test]
        fn every_nested_shipped_file_is_named_by_its_parent_doc() {
            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
            let dir = repo.join("system-knowhow");
            let unnamed: Vec<String> = crate::core::knowhow::collect_md_files(&dir)
                .into_iter()
                .filter_map(|path| crate::core::knowhow::id_from_path(&dir, &path))
                .filter_map(|id| {
                    id.rsplit_once('/')
                        .map(|(parent, _)| (parent.to_string(), id.clone()))
                })
                .filter(|(parent, id)| {
                    let parent_body = std::fs::read_to_string(dir.join(format!("{parent}.md")))
                        .unwrap_or_default();
                    !parent_body.contains(&format!("system-knowhow/{id}"))
                })
                .map(|(parent, id)| format!("{id} (parent {parent}.md)"))
                .collect();
            assert!(
                unnamed.is_empty(),
                "nested system-knowhow files their parent doc does not name by \
             `system-knowhow/<id>`, so nothing can reach them: {unnamed:?}"
            );
        }

        #[test]
        fn glossary_index_lists_every_term_under_its_part() {
            use std::collections::{BTreeMap, BTreeSet};

            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
            let dir = repo.join("system-knowhow");
            let index = std::fs::read_to_string(dir.join("glossary.md")).expect("glossary reads");
            let mut headings = index.lines().skip_while(|line| *line != "## Parts");
            assert_eq!(
                headings.next(),
                Some("## Parts"),
                "glossary has no Parts section"
            );
            let mut indexed = BTreeMap::new();
            let mut part = None;
            for line in headings.take_while(|line| !line.starts_with("## ")) {
                if let Some(heading) = line.strip_prefix("### ") {
                    let id = heading
                        .strip_prefix("`system-knowhow/glossary/")
                        .and_then(|name| name.strip_suffix('`'))
                        .unwrap_or_else(|| panic!("invalid glossary part heading: {heading}"));
                    assert!(
                        indexed.insert(id, BTreeSet::new()).is_none(),
                        "duplicate glossary part heading: {id}"
                    );
                    part = Some(id);
                } else if let Some(term) = line.strip_prefix("- ") {
                    let id = part.expect("glossary bullet precedes its part heading");
                    assert!(
                        indexed.get_mut(id).unwrap().insert(term),
                        "duplicate glossary bullet in {id}: {term}"
                    );
                }
            }

            let parts_dir = dir.join("glossary");
            let files = crate::core::knowhow::collect_md_files(&parts_dir);
            assert!(!files.is_empty(), "glossary has no part files");
            for path in files {
                let id = crate::core::knowhow::id_from_path(&parts_dir, &path).unwrap();
                let body = std::fs::read_to_string(&path).expect("glossary part reads");
                let mut terms = BTreeSet::new();
                for term in body.lines().filter_map(|line| line.strip_prefix("### ")) {
                    assert!(terms.insert(term), "duplicate term heading in {id}: {term}");
                }
                let bullets = indexed
                    .remove(id.as_str())
                    .unwrap_or_else(|| panic!("glossary index is missing part {id}"));
                let missing: Vec<_> = terms.difference(&bullets).collect();
                let stale: Vec<_> = bullets.difference(&terms).collect();
                assert!(
                    missing.is_empty() && stale.is_empty(),
                    "glossary index for {id}: missing bullets {missing:?}, stale bullets {stale:?}"
                );
            }
            assert!(
                indexed.is_empty(),
                "unknown glossary parts: {:?}",
                indexed.keys()
            );
        }

        #[test]
        fn no_shipped_knowhow_file_exceeds_the_cap() {
            use crate::core::knowhow::{knowhow_is_oversized, KNOWHOW_MAX_BYTES};
            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
            let dir = repo.join("system-knowhow");
            let files = crate::core::knowhow::collect_md_files(&dir);
            assert!(
                !files.is_empty(),
                "no system-knowhow files found, the scan is broken"
            );
            let oversized: Vec<String> = files
                .iter()
                .filter_map(|path| {
                    let body = std::fs::read_to_string(path).expect("shipped knowhow reads");
                    knowhow_is_oversized(&body).then(|| {
                        format!(
                            "  {:>7} bytes  {}",
                            body.len(),
                            path.strip_prefix(&dir).unwrap().display()
                        )
                    })
                })
                .collect();
            assert!(
                oversized.is_empty(),
                "system-knowhow file(s) over {KNOWHOW_MAX_BYTES} bytes. Split each into a \
             short index plus `<doc>/<part>.md` parts (building-knowhow.md § Size):\n{}",
                oversized.join("\n")
            );
        }
    }
}
