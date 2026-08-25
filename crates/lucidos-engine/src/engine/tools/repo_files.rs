//! Reaching files inside a *registered repository* from the chat file tools.
//!
//! `read_file`, `glob_files`, `grep_files` and `edit_file` take an optional
//! `repo`. With it set the path is repo-root-relative rather than `data/`-
//! relative, and resolution runs through here instead of through
//! `normalize_data_path`. See `docs/adr/0093-file-tools-reach-registered-repos.md`.
//!
//! Three things this module owns, and nothing else does:
//!
//! - **Which repos exist.** Only a row in `repositories` resolves. No file tool
//!   takes a filesystem path, so an unregistered tree is unreachable by name
//!   and unreachable by accident.
//! - **Where a path may land.** [`resolve_in_repo`] refuses `..` and absolute
//!   inputs, then canonicalizes and proves containment, so a symlink inside the
//!   repo pointing out is refused. Same guard, same reasoning as
//!   `resolve_tmp_path` (ADR 0051 § "Why a symlink guard").
//! - **Which files a walk sees.** [`repo_entries`] enumerates through
//!   `git ls-files`, so `.gitignore` decides. A grep over a Rust repo must not
//!   descend `target/`, and no hand-maintained skip list stays correct.

use std::path::{Path, PathBuf};

use crate::core::repositories::{Repository, RepositoryStore};

/// Resolve the `repo` tool argument (a registered repository's name or id) to
/// its row. The error is agent-facing and names the recovery route: a wrong
/// name is by far the likeliest cause, and the file tools cannot list
/// repositories themselves.
pub(crate) async fn resolve_repo(pool: &sqlx::PgPool, repo: &str) -> Result<Repository, String> {
    let repo = repo.trim();
    if repo.is_empty() {
        return Err("'repo' is empty. Pass a registered repository's name or id.".to_string());
    }
    match RepositoryStore::resolve(pool, repo).await {
        Ok(Some(r)) => Ok(r),
        Ok(None) => Err(format!(
            "No registered repository '{repo}'. Call manage_repositories with action 'list' to \
             see the registered ones, or 'add' to register this path first."
        )),
        Err(e) => Err(format!("Failed to look up repository '{repo}': {e}")),
    }
}

/// Join a repo-root-relative path onto `repo_root`, proving the result stays
/// inside the repo.
///
/// The string check catches `..` and absolute inputs before any filesystem
/// call. Canonicalization catches what a string check cannot: a symlink
/// committed inside the repo that points elsewhere on the host. Both are
/// needed, and neither substitutes for the other.
///
/// A path that does not exist fails to canonicalize and is returned as-is, so
/// the caller reports an ordinary "file not found". A missing file must never
/// masquerade as a security error, or every typo reads like an attack.
pub(crate) fn resolve_in_repo(repo_root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = relative.trim().trim_start_matches("./");
    if relative.is_empty() {
        return Err("'path' is empty".to_string());
    }
    if crate::core::is_path_traversal(relative) {
        return Err(format!(
            "'{relative}' must be relative to the repository root, with no '..' segments"
        ));
    }

    let full = repo_root.join(relative);
    if let (Ok(root), Ok(target)) = (repo_root.canonicalize(), full.canonicalize()) {
        if !target.starts_with(&root) {
            return Err(format!(
                "'{relative}' resolves outside the repository through a symlink"
            ));
        }
    }
    Ok(full)
}

/// Every file in the repo a walk may see, sorted lexicographically. Each entry
/// is a repo-relative path and its absolute path. That is the shape
/// `glob_files` and `grep_files` already consume, so the search helpers take
/// entries and stay ignorant of where they came from.
///
/// `--cached --others --exclude-standard` is tracked files plus untracked ones
/// git would not ignore. That is the working tree as the user thinks of it: a
/// file written five minutes ago is in, and `target/` is out.
///
/// The disk filter does two things. `--cached` also lists a tracked file
/// deleted from the working tree, and a glob must not answer with paths whose
/// reads then fail. It also uses `symlink_metadata`, so a symlink is dropped
/// rather than followed.
///
/// Dropping symlinks is load-bearing, not tidiness. A search reads its entries
/// directly, never through [`resolve_in_repo`]. So a followed link to
/// `/etc/passwd` would hand back its contents, through a tool whose whole
/// contract is staying inside the repo. `list_searchable_data_files` skips
/// symlinks for the same reason, one tree over.
pub(crate) async fn repo_entries(repo_root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let output = crate::engine::git_ops::git_cmd(
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
        repo_root,
    )
    .await?;

    if !output.status.success() {
        return Err(format!(
            "git ls-files failed in the repository: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let mut entries: Vec<(String, PathBuf)> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|rel| (rel.to_string(), repo_root.join(rel)))
        .filter(|(_, abs)| std::fs::symlink_metadata(abs).is_ok_and(|m| m.file_type().is_file()))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn git(root: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    }

    fn init_repo(root: &Path) {
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "t@example.com"]);
        git(root, &["config", "user.name", "T"]);
    }

    #[test]
    fn resolves_an_ordinary_relative_path() {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), "src/main.rs", "fn main() {}");

        let got = resolve_in_repo(repo.path(), "src/main.rs").unwrap();
        assert_eq!(std::fs::read_to_string(got).unwrap(), "fn main() {}");
    }

    #[test]
    fn strips_a_leading_dot_slash() {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), "README.md", "hi");

        let got = resolve_in_repo(repo.path(), "./README.md").unwrap();
        assert_eq!(std::fs::read_to_string(got).unwrap(), "hi");
    }

    #[test]
    fn refuses_a_parent_escape_and_an_absolute_path() {
        let repo = tempfile::tempdir().unwrap();

        for bad in ["../secret", "a/../../secret", "/etc/passwd"] {
            let err = resolve_in_repo(repo.path(), bad).unwrap_err();
            assert!(
                err.contains("relative to the repository root"),
                "{bad} gave: {err}"
            );
        }
    }

    #[test]
    fn refuses_a_symlink_that_escapes_the_repository() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "private").unwrap();

        let repo = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            repo.path().join("escape.txt"),
        )
        .unwrap();

        let err = resolve_in_repo(repo.path(), "escape.txt").unwrap_err();
        assert!(err.contains("symlink"), "got: {err}");
    }

    #[test]
    fn allows_a_symlink_that_stays_inside_the_repository() {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), "src/real.rs", "inside");
        std::os::unix::fs::symlink(repo.path().join("src/real.rs"), repo.path().join("link.rs"))
            .unwrap();

        let got = resolve_in_repo(repo.path(), "link.rs").unwrap();
        assert_eq!(std::fs::read_to_string(got).unwrap(), "inside");
    }

    /// A typo must read as a typo. Canonicalization fails on a missing path, so
    /// the containment check is skipped rather than reported as an escape.
    #[test]
    fn a_missing_path_is_not_a_security_error() {
        let repo = tempfile::tempdir().unwrap();
        let got = resolve_in_repo(repo.path(), "src/nope.rs").unwrap();
        assert!(!got.exists());
    }

    #[tokio::test]
    async fn entries_skip_gitignored_trees_and_include_untracked_files() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(repo.path(), ".gitignore", "target/\n");
        write(repo.path(), "src/main.rs", "fn main() {}");
        write(repo.path(), "target/debug/huge.bin", "build output");
        git(repo.path(), &["add", "."]);
        // Written after `git add`, so it is untracked but not ignored.
        write(repo.path(), "src/fresh.rs", "brand new");

        let rels: Vec<String> = repo_entries(repo.path())
            .await
            .unwrap()
            .into_iter()
            .map(|(rel, _)| rel)
            .collect();

        assert!(rels.contains(&"src/main.rs".to_string()), "{rels:?}");
        assert!(rels.contains(&"src/fresh.rs".to_string()), "{rels:?}");
        assert!(
            !rels.iter().any(|r| r.starts_with("target/")),
            "gitignored build output leaked into the walk: {rels:?}"
        );
    }

    /// A tracked symlink pointing out of the repo must not enter the walk.
    /// `grep_entries` reads its entries directly, so an entry that escapes is
    /// an entry whose contents leave the repository.
    #[tokio::test]
    async fn entries_drop_a_symlink_that_escapes_the_repository() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "private").unwrap();

        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(repo.path(), "real.rs", "inside");
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            repo.path().join("escape.txt"),
        )
        .unwrap();
        git(repo.path(), &["add", "-A"]);

        let entries = repo_entries(repo.path()).await.unwrap();
        let rels: Vec<&str> = entries.iter().map(|(rel, _)| rel.as_str()).collect();

        assert!(rels.contains(&"real.rs"), "{rels:?}");
        assert!(
            !rels.contains(&"escape.txt"),
            "a symlink out of the repo reached the walk: {rels:?}"
        );
        for (_, abs) in &entries {
            assert!(
                !std::fs::read_to_string(abs)
                    .unwrap_or_default()
                    .contains("private"),
                "outside content is reachable through an entry"
            );
        }
    }

    /// A symlink INSIDE the repo is dropped too. The walk cannot prove
    /// containment cheaply per entry, so it follows no link at all. The target
    /// is listed under its own real path anyway.
    #[tokio::test]
    async fn entries_drop_an_internal_symlink_but_keep_its_target() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(repo.path(), "src/real.rs", "inside");
        std::os::unix::fs::symlink(repo.path().join("src/real.rs"), repo.path().join("link.rs"))
            .unwrap();
        git(repo.path(), &["add", "-A"]);

        let rels: Vec<String> = repo_entries(repo.path())
            .await
            .unwrap()
            .into_iter()
            .map(|(rel, _)| rel)
            .collect();

        assert_eq!(rels, vec!["src/real.rs".to_string()]);
    }

    /// `--cached` lists a tracked file that no longer exists on disk. A glob
    /// answering with it would hand back paths whose reads then fail.
    #[tokio::test]
    async fn entries_drop_a_tracked_file_deleted_from_the_working_tree() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        write(repo.path(), "keep.rs", "keep");
        write(repo.path(), "gone.rs", "gone");
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "init"]);
        std::fs::remove_file(repo.path().join("gone.rs")).unwrap();

        let rels: Vec<String> = repo_entries(repo.path())
            .await
            .unwrap()
            .into_iter()
            .map(|(rel, _)| rel)
            .collect();

        assert_eq!(rels, vec!["keep.rs".to_string()]);
    }
}
