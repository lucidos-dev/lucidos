//! Engine-shipped reference knowhow about how Lucidos itself works.
//!
//! Sourced from the engine-shipped `system-knowhow/` reference set — the staged
//! `LUCIDOS_SYSTEM_KNOWHOW_DIR` on packaged builds, `<repo>/system-knowhow/` on
//! a dev checkout (see [`resolve_system_knowhow_dir`]) — never overrideable by a
//! workspace's local `data/knowhow/` or the shared `~/.lucidos/knowhow/`. The
//! LLM sees these with a `[SYSTEM-KNOWHOW: ...]` tag (vs. `[KNOW-HOW: ...]`
//! for user-curated knowhow) so it knows the source is authoritative.
//!
//! On-disk format and loading match knowhow exactly, so this module reuses
//! `KnowhowStore` for parsing and only adds the system-knowhow tag + the
//! [`is_system_knowhow_path`] predicate that gates read-only enforcement.

use std::path::{Path, PathBuf};

use crate::core::knowhow::{Knowhow, KnowhowListDepth, KnowhowStore, KnowhowSummary};

pub struct SystemKnowhowStore;

impl SystemKnowhowStore {
    /// Every `.md` in the shipped tree, at any depth. A workspace root lists
    /// docs only, but this corpus is curated in the repo: what ships is the
    /// catalog, and a stray depth is a bug we fix at the source.
    pub fn load_summaries(dir: &Path) -> Vec<KnowhowSummary> {
        KnowhowStore::load_summaries(dir, KnowhowListDepth::Unbounded)
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

/// Resolve the engine-shipped `system-knowhow/` directory at boot, returning the
/// resolved dir (if any) plus at most one loud warning to log.
///
/// Resolution order (INV-3 of the "package system-knowhow" plan):
///   1. `LUCIDOS_SYSTEM_KNOWHOW_DIR` set (non-empty) → **authoritative**: it MUST
///      exist. Set-but-missing is a mis-staged bundle — warn loudly and treat as
///      unavailable; NEVER silently fall back to `repo_root` (bogus in packaged).
///   2. Env var unset/empty → today's `<repo_root>/system-knowhow` fallback,
///      byte-identical to the dev/source-checkout behavior.
///   3. Neither resolves → unavailable. On a packaged build (`is_packaged`) this
///      is a real defect (there is no checkout), so warn loudly naming the env
///      var (INV-4); dev/e2e without the dir is expected and stays quiet.
///
/// Pure over its inputs (env value + repo root + packaged flag) so it is
/// unit-testable offline; the caller does the I/O of reading the env var and
/// logging the returned warnings.
pub fn resolve_system_knowhow_dir(
    env_value: Option<&str>,
    repo_root: &Path,
    is_packaged: bool,
) -> (Option<PathBuf>, Option<String>) {
    // 1. The env var is authoritative when set — the packaged launcher points it
    //    at <resources>/system-knowhow. Trim only to DETECT a blank value
    //    (= unset); the path is built from the original bytes — a legitimate
    //    dir path may carry edge whitespace.
    if let Some(value) = env_value.filter(|v| !v.trim().is_empty()) {
        let candidate = PathBuf::from(value);
        if candidate.is_dir() {
            return (Some(candidate), None);
        }
        return (None, Some(format!(
            "[Knowhow] LUCIDOS_SYSTEM_KNOWHOW_DIR is set to '{value}' but that directory does not \
             exist — the engine-shipped reference set is UNAVAILABLE (load_knowhow('system-knowhow/…'), \
             GET /api/v1/knowhow, and the data-API read path all degrade). This is a packaging bug: \
             the bundle must stage system-knowhow/ at that path."
        )));
    }

    // 2. No env var: the dev/source-checkout fallback, unchanged.
    let candidate = repo_root.join("system-knowhow");
    if candidate.is_dir() {
        return (Some(candidate), None);
    }

    // 3. Unresolvable. Loud only when packaged — a source checkout without the
    //    dir is expected (matches the prior silent `None`).
    if is_packaged {
        return (None, Some(
            "[Knowhow] system-knowhow directory is UNAVAILABLE: LUCIDOS_SYSTEM_KNOWHOW_DIR is unset \
             and no <repo>/system-knowhow exists. The engine-shipped reference set is missing \
             (load_knowhow('system-knowhow/…'), GET /api/v1/knowhow, and the data-API read path all \
             degrade). This is a packaging bug: the bundle must set LUCIDOS_SYSTEM_KNOWHOW_DIR."
                .to_string(),
        ));
    }
    (None, None)
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

    /// A workspace root lists docs only, and the shipped corpus does not. The
    /// depth cap is about a user's own files, so it must not reach this tree:
    /// `shipped_system_knowhow_files_all_parse` walks every file at any depth.
    #[test]
    fn load_summaries_lists_a_doc_at_any_depth() {
        let tmp = tempfile::tempdir().unwrap();
        write_doc(
            &tmp.path().join("scripts").join("deep").join("helper.md"),
            "Helper",
            "Body.",
        );

        let ids: Vec<String> = SystemKnowhowStore::load_summaries(tmp.path())
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec!["scripts/deep/helper".to_string()]);
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

    // ── resolve_system_knowhow_dir (INV-3, INV-4) ────────────────────────────

    #[test]
    fn resolve_prefers_the_env_var_when_the_dir_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let staged = tmp.path().join("resources/system-knowhow");
        std::fs::create_dir_all(&staged).unwrap();
        // A repo_root that ALSO has a system-knowhow — the env var must still win.
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
        // repo_root HAS a system-knowhow, proving we do NOT silently fall back to it.
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("system-knowhow")).unwrap();
        let missing = tmp.path().join("resources/system-knowhow"); // never created

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
        // The dev/source-checkout path: env unset, repo_root has the dir.
        // Byte-identical to the pre-change behavior, and no warning either way.
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

    /// The env path is used with its original bytes — trimming is only for the
    /// blank-detection above, so a dir whose real path carries edge whitespace
    /// still resolves.
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
        // No env var and no repo-root dir: dev stays silent (expected), packaged
        // warns loudly naming the env var (INV-4).
        let empty = tempfile::tempdir().unwrap(); // no system-knowhow subdir

        let (dev_dir, dev_warning) = resolve_system_knowhow_dir(None, empty.path(), false);
        assert_eq!(dev_dir, None);
        assert_eq!(dev_warning, None, "dev without the dir is expected");

        let (pkg_dir, pkg_warning) = resolve_system_knowhow_dir(None, empty.path(), true);
        assert_eq!(pkg_dir, None);
        let pkg_warning = pkg_warning.expect("packaged + unresolvable must warn");
        assert!(pkg_warning.contains("LUCIDOS_SYSTEM_KNOWHOW_DIR"));
    }

    /// A system-knowhow `description:` is a ROUTING signal, not a summary: the
    /// engine semantically matches the user's message against it to decide
    /// which doc to offer, and every one of them sits in the prompt of every
    /// turn whether or not it is ever loaded. So it carries two things and
    /// nothing else: what the doc covers, and the phrases a user might say that
    /// should reach it. The doc body one `load_knowhow` away carries the
    /// conclusions, the worked examples and the caveats.
    ///
    /// The ceiling is per-file rather than a total, because a total lets one
    /// runaway description hide behind twenty short ones. Same reasoning as
    /// `PER_TOOL_SCHEMA_CEILING_CHARS`.
    ///
    /// A RATCHET, set just above where the 2026-08-07 trim landed: 24 files,
    /// 6,584 characters of description, mean 274, largest 362
    /// (`thread-events`). It was 700 before that trim, which let a description
    /// carry a summary of the doc rather than a route to it (`oauth-providers`
    /// was 692). Raising it means a description has earned the room, in a
    /// change that says why.
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
