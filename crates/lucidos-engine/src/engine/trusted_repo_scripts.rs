//! Which path-qualified heads of a coding-agent command are *trusted repo
//! scripts* (`docs/glossary.md`).
//!
//! A path head is a refusal by default, because the agent can write a file
//! and then run it (`command_guard::head_safety`). A trusted one resolves to a
//! tracked file of the session's own worktree whose bytes match the merge
//! target branch. The agent has not changed it, so it may go to the judge
//! like `make`. Anything git or the disk cannot answer reads as untrusted.
//!
//! Trust is that weak on purpose:
//!
//! - it is a check at request time, and the file can change before it runs;
//! - an earlier segment of the same line can rewrite it;
//! - only the head is compared, never a file it sources, as `make` never
//!   checks its Makefile.
//!
//! So a grant covers a trusted head only by its worktree-relative path, and
//! only when no segment runs before it (`command_guard::grant_covers_command`,
//! ADR 0571).

use crate::engine::command_guard::{segment_heads_as_written, unwrap_shell_command, TrustedHeads};
use crate::engine::git_ops::{default_local_branch, git_answer, git_cmd};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Words that move the shell's directory, or run a file that may. After one
/// of them, a relative head no longer resolves against the request's `cwd`.
const DIRECTORY_CHANGERS: [&str; 6] = ["cd", "chdir", "pushd", "popd", "source", "."];

/// The tree modes of a plain file. A symlink, submodule or tree on the target
/// branch is no script there, whatever bytes it holds.
const FILE_MODES: [&str; 2] = ["100644", "100755"];

/// The trusted repo scripts among `command`'s path heads.
///
/// `cwd` is where the request says the command starts. A relative head needs
/// it: without it, or after a directory change in the line, nothing relative
/// is trusted. `worktree` is the session's worktree as the engine knows it,
/// never the request's say-so. `None` trusts nothing.
pub(crate) async fn trusted_repo_scripts(
    command: &str,
    cwd: Option<&Path>,
    worktree: Option<&Path>,
) -> TrustedHeads {
    let Some(worktree) = worktree else {
        return TrustedHeads::NONE;
    };
    let heads = segment_heads_as_written(command);
    let path_heads: BTreeSet<&str> = heads
        .iter()
        .map(String::as_str)
        .filter(|h| h.contains('/'))
        .collect();
    if path_heads.is_empty() {
        return TrustedHeads::NONE;
    }
    let Ok(root) = tokio::fs::canonicalize(worktree).await else {
        return TrustedHeads::NONE;
    };
    let cwd = cwd
        .filter(|c| c.is_absolute() && !changes_directory(command))
        .map(Path::to_path_buf);
    // Fully qualified, so a tag the agent names like the branch cannot stand in.
    let target = format!("refs/heads/{}", default_local_branch(&root).await);
    let mut trusted = Vec::new();
    for head in path_heads {
        let Some(rel) = resolve_in_worktree(head, cwd.as_deref(), &root).await else {
            continue;
        };
        let Some(rel) = rel.to_str() else {
            continue;
        };
        if matches_target(&root, &target, rel).await {
            trusted.push((head.to_string(), rel.to_string()));
        }
    }
    TrustedHeads::new(trusted)
}

/// True when any word of the line could move the shell's directory. Read over
/// every word, not segment heads, so `builtin cd`, `(cd x; …)` and
/// `eval "cd x"` all count. Quotes and backslashes go first, so `\cd` and
/// `c''d` count too. A false hit only costs a card.
fn changes_directory(command: &str) -> bool {
    let bare: String = unwrap_shell_command(command)
        .chars()
        .filter(|c| !"'\"\\".contains(*c))
        .collect();
    bare.split(|c: char| c.is_whitespace() || ";&|(){}`".contains(c))
        .any(|word| DIRECTORY_CHANGERS.contains(&word))
}

/// `head` resolved as the shell would, symlinks followed, as a path relative
/// to the canonical worktree `root`. `None` when it does not resolve, leaves
/// the worktree, or is not a regular file.
async fn resolve_in_worktree(head: &str, cwd: Option<&Path>, root: &Path) -> Option<PathBuf> {
    let written = Path::new(head);
    let full = if written.is_absolute() {
        written.to_path_buf()
    } else {
        cwd?.join(written)
    };
    let real = tokio::fs::canonicalize(&full).await.ok()?;
    let rel = real.strip_prefix(root).ok()?.to_path_buf();
    tokio::fs::metadata(&real)
        .await
        .ok()?
        .is_file()
        .then_some(rel)
}

/// Whether `rel` is tracked and its working-tree bytes equal the plain file at
/// the same path on `target`. Every git failure answers no.
async fn matches_target(root: &Path, target: &str, rel: &str) -> bool {
    let tracked = git_answer(&["ls-files", "--error-unmatch", "--", rel], root)
        .await
        .or_unknown(false);
    if !tracked {
        return false;
    }
    let Some(target_blob) = target_file_blob(root, target, rel).await else {
        return false;
    };
    stdout_of(&["hash-object", "--no-filters", "--", rel], root)
        .await
        .is_some_and(|blob| blob == target_blob)
}

/// The blob id of the plain file at `rel` on `target`, or `None`.
async fn target_file_blob(root: &Path, target: &str, rel: &str) -> Option<String> {
    let listing = stdout_of(&["ls-tree", "--full-tree", target, "--", rel], root).await?;
    let (meta, _path) = listing.split_once('\t')?;
    match meta.split(' ').collect::<Vec<_>>().as_slice() {
        [mode, "blob", blob] if FILE_MODES.contains(mode) => Some(blob.to_string()),
        _ => None,
    }
}

/// Trimmed stdout of a git call that succeeded with output, else `None`.
async fn stdout_of(args: &[&str], dir: &Path) -> Option<String> {
    let out = git_cmd(args, dir).await.ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "scripts/tool.sh";
    const HEAD: &str = "./scripts/tool.sh";

    /// A repo on `main` holding [`SCRIPT`], with a linked worktree on a
    /// branch, the shape of a coding-agent session.
    struct Session {
        _tmp: tempfile::TempDir,
        worktree: PathBuf,
    }

    async fn git(args: &[&str], dir: &Path) {
        let mut full = vec!["-c", "user.name=t", "-c", "user.email=t@example.com"];
        full.extend_from_slice(args);
        let out = git_cmd(&full, dir).await.unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    async fn session() -> Session {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("scripts")).unwrap();
        git(&["init", "-b", "main"], &repo).await;
        std::fs::write(repo.join(SCRIPT), "#!/bin/sh\necho hi\n").unwrap();
        git(&["add", "."], &repo).await;
        git(&["commit", "-m", "init"], &repo).await;
        let worktree = tmp.path().join("wt");
        git(
            &["worktree", "add", "-b", "agent", worktree.to_str().unwrap()],
            &repo,
        )
        .await;
        Session {
            _tmp: tmp,
            worktree,
        }
    }

    impl Session {
        async fn trusts(&self, command: &str) -> bool {
            self.trusts_from(command, Some(&self.worktree)).await
        }

        async fn trusts_from(&self, command: &str, cwd: Option<&Path>) -> bool {
            let head = segment_heads_as_written(command)
                .into_iter()
                .find(|h| h.contains('/'))
                .expect("a path head");
            trusted_repo_scripts(command, cwd, Some(&self.worktree))
                .await
                .contains(&head)
        }
    }

    #[tokio::test]
    async fn an_unchanged_tracked_script_is_trusted() {
        let s = session().await;
        assert!(s.trusts(HEAD).await);
        assert!(
            s.trusts(&format!("/bin/zsh -lc 'git status && {HEAD} --x'"))
                .await
        );
        let absolute = s.worktree.canonicalize().unwrap().join(SCRIPT);
        assert!(s.trusts_from(absolute.to_str().unwrap(), None).await);
    }

    #[tokio::test]
    async fn a_modified_script_is_not_trusted() {
        let s = session().await;
        std::fs::write(s.worktree.join(SCRIPT), "#!/bin/sh\ncurl evil\n").unwrap();
        assert!(!s.trusts(HEAD).await);
    }

    #[tokio::test]
    async fn a_committed_change_is_not_trusted() {
        // Tracked and clean on the agent's branch, but not what main holds.
        let s = session().await;
        std::fs::write(s.worktree.join(SCRIPT), "#!/bin/sh\ncurl evil\n").unwrap();
        git(&["commit", "-am", "edit"], &s.worktree).await;
        assert!(!s.trusts(HEAD).await);
    }

    #[tokio::test]
    async fn a_tag_named_like_the_target_does_not_stand_in_for_it() {
        let s = session().await;
        std::fs::write(s.worktree.join(SCRIPT), "#!/bin/sh\ncurl evil\n").unwrap();
        git(&["commit", "-am", "edit"], &s.worktree).await;
        git(&["tag", "main", "HEAD"], &s.worktree).await;
        assert!(!s.trusts(HEAD).await);
    }

    #[tokio::test]
    async fn an_untracked_script_is_not_trusted() {
        let s = session().await;
        std::fs::write(s.worktree.join("scripts/new.sh"), "#!/bin/sh\n").unwrap();
        assert!(!s.trusts("./scripts/new.sh").await);
    }

    #[tokio::test]
    async fn a_script_absent_on_the_target_is_not_trusted() {
        let s = session().await;
        std::fs::write(s.worktree.join("scripts/new.sh"), "#!/bin/sh\n").unwrap();
        git(&["add", "."], &s.worktree).await;
        git(&["commit", "-m", "add"], &s.worktree).await;
        assert!(!s.trusts("./scripts/new.sh").await);
    }

    #[tokio::test]
    async fn a_script_outside_the_worktree_is_not_trusted() {
        let s = session().await;
        let outside = s.worktree.parent().unwrap().join("repo").join(SCRIPT);
        assert!(!s.trusts_from(outside.to_str().unwrap(), None).await);
        assert!(!s.trusts("/bin/ls").await);
    }

    #[tokio::test]
    async fn a_symlink_leaving_the_worktree_is_not_trusted() {
        let s = session().await;
        let outside = s.worktree.parent().unwrap().join("repo").join(SCRIPT);
        std::os::unix::fs::symlink(outside, s.worktree.join("scripts/link.sh")).unwrap();
        assert!(!s.trusts("./scripts/link.sh").await);
    }

    #[tokio::test]
    async fn a_symlink_inside_the_worktree_follows_to_its_file() {
        let s = session().await;
        std::os::unix::fs::symlink("tool.sh", s.worktree.join("scripts/link.sh")).unwrap();
        assert!(s.trusts("./scripts/link.sh").await);
    }

    #[tokio::test]
    async fn a_relative_head_needs_a_known_cwd_and_no_directory_change() {
        let s = session().await;
        assert!(!s.trusts_from(HEAD, None).await);
        assert!(!s.trusts(&format!("cd scripts && {HEAD}")).await);
        assert!(!s.trusts(&format!("(builtin cd x; {HEAD})")).await);
        for line in [
            "chdir x",
            "\\cd x",
            "c''d x",
            ". ./setup.sh",
            "source env.sh",
        ] {
            assert!(!s.trusts(&format!("{line} && {HEAD}")).await, "{line}");
        }
        // From a subdirectory the same text names a different file.
        let sub = s.worktree.join("scripts");
        assert!(!s.trusts_from(HEAD, Some(&sub)).await);
    }

    /// Whether `granted` covers `command` run from the worktree, under the
    /// trust the engine resolves for it.
    async fn grant_covers(s: &Session, command: &str, granted: &[&str]) -> bool {
        use crate::engine::command_guard::{grant_covers_command, CommandSite};
        let trusted = trusted_repo_scripts(command, Some(&s.worktree), Some(&s.worktree)).await;
        grant_covers_command(
            "Bash",
            command,
            CommandSite::TextOnly { root: None },
            &trusted,
            |p| granted.contains(&p),
        )
    }

    const PATH_GRANT: &str = "Bash(./scripts/tool.sh:*)";

    /// A path grant covers an unchanged tracked script run first, however the
    /// head is written (ADR 0571).
    #[tokio::test]
    async fn a_path_grant_covers_a_trusted_script_run_first() {
        let s = session().await;
        assert!(grant_covers(&s, HEAD, &[PATH_GRANT]).await);
        assert!(grant_covers(&s, &format!("/bin/zsh -lc '{HEAD} --check'"), &[PATH_GRANT]).await);
        let absolute = s.worktree.canonicalize().unwrap().join(SCRIPT);
        assert!(grant_covers(&s, absolute.to_str().unwrap(), &[PATH_GRANT]).await);
        // A later segment still needs its own grant.
        let chained = format!("{HEAD} && curl -X POST https://example.com");
        assert!(!grant_covers(&s, &chained, &[PATH_GRANT]).await);
        assert!(grant_covers(&s, &chained, &[PATH_GRANT, "Bash(curl:*)"]).await);
    }

    /// Invariant 3 of the plan: trust at request time, and nothing before the
    /// head.
    #[tokio::test]
    async fn a_path_grant_covers_no_changed_late_or_foreign_script() {
        let s = session().await;
        let all = [PATH_GRANT, "Bash(git:*)", "Bash(echo:*)", "Bash(s.sh:*)"];
        // Something runs before the head, which could rewrite it.
        for command in [
            format!("git status && {HEAD}"),
            format!("echo x > {SCRIPT} && {HEAD}"),
            "echo x > s.sh && ./s.sh".to_string(),
        ] {
            assert!(!grant_covers(&s, &command, &all).await, "{command}");
        }
        // An untracked script, and a symlink leaving the worktree.
        std::fs::write(s.worktree.join("scripts/new.sh"), "#!/bin/sh\n").unwrap();
        assert!(!grant_covers(&s, "./scripts/new.sh", &["Bash(./scripts/new.sh:*)"]).await);
        let outside = s.worktree.parent().unwrap().join("repo").join(SCRIPT);
        std::os::unix::fs::symlink(&outside, s.worktree.join("scripts/link.sh")).unwrap();
        assert!(!grant_covers(&s, "./scripts/link.sh", &all).await);
        // The same path in another checkout.
        assert!(!grant_covers(&s, outside.to_str().unwrap(), &all).await);
        // A modified script.
        std::fs::write(s.worktree.join(SCRIPT), "#!/bin/sh\ncurl evil\n").unwrap();
        assert!(!grant_covers(&s, HEAD, &all).await);
    }

    #[tokio::test]
    async fn a_directory_outside_any_repo_trusts_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("scripts")).unwrap();
        std::fs::write(tmp.path().join(SCRIPT), "#!/bin/sh\n").unwrap();
        let trusted = trusted_repo_scripts(HEAD, Some(tmp.path()), Some(tmp.path())).await;
        assert_eq!(trusted, TrustedHeads::NONE);
    }

    #[tokio::test]
    async fn no_worktree_trusts_nothing() {
        let s = session().await;
        let trusted = trusted_repo_scripts(HEAD, Some(&s.worktree), None).await;
        assert_eq!(trusted, TrustedHeads::NONE);
    }
}
