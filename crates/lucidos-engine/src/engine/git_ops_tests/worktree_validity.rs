//! Tests for the resume-path worktree-validity guards
//! ([`super::is_live_worktree_at`] / [`super::clear_stranded_worktree_dir`]).
//!
//! Regression background: a resumed Claude Code session must never run in a
//! *stranded* directory — present on disk but not a live linked worktree — that
//! git resolves to the enclosing workspace data repo (`.lucidos/worktrees/*`
//! sits inside it). That was the "worktree torn down" failure: a cleanup
//! reclaimed the tree, a resume landed in the leftover dir, and git operated on
//! the data repo. `is_live_worktree_at` is the reuse gate; on a miss the spawn
//! path clears the residue and recreates.

use super::common::make_test_repo;
use super::*;

/// Read a linked worktree's admin dir (`<repo>/.git/worktrees/<name>`) from its
/// `.git` gitlink file and delete it — the way the cleanup bug strands a tree.
async fn strand(wt: &std::path::Path) {
    let dotgit = tokio::fs::read_to_string(wt.join(".git"))
        .await
        .expect("read worktree .git gitlink");
    let target = dotgit
        .trim()
        .strip_prefix("gitdir:")
        .expect("gitdir: line")
        .trim();
    let admin = {
        let p = std::path::PathBuf::from(target);
        if p.is_absolute() { p } else { wt.join(p) }
    };
    tokio::fs::remove_dir_all(&admin)
        .await
        .expect("remove admin dir to strand worktree");
}

#[tokio::test]
async fn is_live_worktree_at_true_for_real_worktree() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-live");
    let out = worktree_add(&repo, &wt, &["-b", "claude-code/live"])
        .await
        .unwrap();
    assert!(out.status.success(), "fixture worktree_add failed");
    assert!(
        is_live_worktree_at(&wt).await,
        "a real linked worktree must read as live"
    );
}

#[tokio::test]
async fn is_live_worktree_at_false_when_absent() {
    let base = tempfile::tempdir().unwrap();
    let missing = base.path().join("thread-gone");
    assert!(
        !is_live_worktree_at(&missing).await,
        "a missing path is not a live worktree"
    );
}

#[tokio::test]
async fn is_live_worktree_at_false_for_plain_dir_outside_any_repo() {
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("thread-residue");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(dir.join(".keep"), b"x").await.unwrap();
    assert!(
        !is_live_worktree_at(&dir).await,
        "a plain dir outside any repo is not a live worktree"
    );
}

#[tokio::test]
async fn is_live_worktree_at_false_for_dir_inside_enclosing_repo() {
    // The exact incident shape: a directory that physically sits *inside* a git
    // repo (as `.lucidos/worktrees/*` sits inside the workspace data repo) but
    // is not itself a worktree. git resolves `--show-toplevel` UP to the
    // enclosing repo, so the guard must reject it — otherwise a resumed session
    // would run against that repo.
    let (_tmp, repo) = make_test_repo().await;
    let inside = repo.join("nested-dir");
    tokio::fs::create_dir_all(&inside).await.unwrap();
    assert!(
        !is_live_worktree_at(&inside).await,
        "a plain dir inside an enclosing repo must NOT read as a live worktree"
    );
}

#[tokio::test]
async fn is_live_worktree_at_false_for_stranded_admin_dir() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-stranded");
    let out = worktree_add(&repo, &wt, &["-b", "claude-code/stranded"])
        .await
        .unwrap();
    assert!(out.status.success(), "fixture worktree_add failed");
    assert!(is_live_worktree_at(&wt).await, "precondition: live before stranding");

    strand(&wt).await;

    assert!(
        wt.exists(),
        "stranding leaves the working dir on disk (only the admin dir is gone)"
    );
    assert!(
        !is_live_worktree_at(&wt).await,
        "a stranded worktree must read as not-live"
    );
}

#[tokio::test]
async fn clear_stranded_worktree_dir_removes_residue_and_allows_readd() {
    // A stranded dir occupies the target path; clearing it must let a fresh
    // `worktree_add` succeed at the same path — the spawn self-heal.
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-heal");
    let out = worktree_add(&repo, &wt, &["-b", "claude-code/heal"])
        .await
        .unwrap();
    assert!(out.status.success());

    strand(&wt).await;
    assert!(wt.exists(), "stranded residue present before clearing");
    assert!(!is_live_worktree_at(&wt).await);

    clear_stranded_worktree_dir(&repo, &wt).await;
    assert!(!wt.exists(), "stranded residue must be removed");

    let out2 = worktree_add(&repo, &wt, &["-b", "claude-code/heal2"])
        .await
        .unwrap();
    assert!(
        out2.status.success(),
        "worktree_add must succeed after clearing residue: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    assert!(
        is_live_worktree_at(&wt).await,
        "the recreated worktree must read as live"
    );
}

#[tokio::test]
async fn clear_stranded_worktree_dir_removes_leftover_nested_in_enclosing_repo() {
    // The exact incident shape: a leftover directory inside the workspace data
    // repo (`<repo>/.lucidos/worktrees/thread-*`) that is not a worktree.
    // `git rev-parse --show-toplevel` resolves UP to the enclosing repo, which
    // is positive stranding evidence — clear must remove the residue.
    let (_tmp, repo) = make_test_repo().await;
    let inside = repo.join(".lucidos").join("worktrees").join("thread-x");
    tokio::fs::create_dir_all(&inside).await.unwrap();
    tokio::fs::write(inside.join("leftover.txt"), b"x").await.unwrap();
    assert!(!is_live_worktree_at(&inside).await, "precondition: not live");

    clear_stranded_worktree_dir(&repo, &inside).await;
    assert!(
        !inside.exists(),
        "a leftover dir resolving to the enclosing repo must be cleared"
    );
}

#[tokio::test]
async fn clear_stranded_worktree_dir_refuses_a_live_worktree() {
    // Safety: even if clear is ever called on a path that is actually a live
    // worktree (e.g. the upstream liveness probe false-negatived on a transient
    // git hiccup), it must touch NOTHING — not `git worktree remove --force`
    // (which would discard a dirty tree), not `remove_dir_all`. The positive-
    // stranding-evidence gate must precede both.
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-live-keep");
    worktree_add(&repo, &wt, &["-b", "claude-code/live-keep"])
        .await
        .unwrap();
    tokio::fs::write(wt.join("uncommitted.txt"), b"work in progress")
        .await
        .unwrap();
    assert!(is_live_worktree_at(&wt).await, "precondition: live worktree");

    clear_stranded_worktree_dir(&repo, &wt).await;

    assert!(wt.exists(), "a live worktree must never be cleared");
    assert!(
        wt.join("uncommitted.txt").exists(),
        "uncommitted work in a live worktree must survive"
    );
    assert!(
        is_live_worktree_at(&wt).await,
        "the worktree must still read as live after a no-op clear"
    );
}

#[tokio::test]
async fn clear_stranded_worktree_dir_never_deletes_the_branch() {
    // Clearing residue must never touch branch refs — committed work lives on
    // the branch and survives in the main repo even after the tree is gone.
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-keepbranch");
    worktree_add(&repo, &wt, &["-b", "claude-code/keepbranch"])
        .await
        .unwrap();
    // Put a commit on the branch so it carries real work.
    tokio::fs::write(wt.join("work.txt"), b"committed").await.unwrap();
    let _ = git_cmd(&["add", "."], &wt).await;
    let _ = git_cmd(&["commit", "-m", "work"], &wt).await;
    strand(&wt).await;

    clear_stranded_worktree_dir(&repo, &wt).await;

    let exists = git_cmd(&["rev-parse", "--verify", "claude-code/keepbranch"], &repo)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    assert!(exists, "clear must never delete the branch ref");
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

fn shell_single_quote_for_test(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[tokio::test]
async fn install_coding_agent_diff_hook_sets_worktree_local_hooks_and_chains_common_hook() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-hook");
    let out = worktree_add(&repo, &wt, &["-b", "claude-code/hook"])
        .await
        .unwrap();
    assert!(out.status.success(), "fixture worktree_add failed");

    let chained_log = wt_base.path().join("chained.log");
    let common_hook = repo.join(".git/hooks/post-commit");
    tokio::fs::write(
        &common_hook,
        format!(
            "#!/bin/sh\nprintf chained > {}\n",
            shell_single_quote_for_test(&chained_log.to_string_lossy())
        ),
    )
    .await
    .unwrap();
    #[cfg(unix)]
    make_executable(&common_hook);

    install_coding_agent_diff_hook(&repo, &wt).await.unwrap();

    let hooks_path = git_cmd(&["config", "--worktree", "--get", "core.hooksPath"], &wt)
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&hooks_path.stdout).trim(),
        ".lucidos/git-hooks",
        "Lucidos hook path must be scoped to this worktree"
    );

    let hook_script = tokio::fs::read_to_string(wt.join(".lucidos/git-hooks/post-commit"))
        .await
        .unwrap();
    assert!(hook_script.contains("lucidos coding-agent-diff-hook"));
    assert!(
        hook_script.contains(&common_hook.to_string_lossy().to_string()),
        "Lucidos hook must chain the repo's existing common post-commit hook"
    );

    tokio::fs::write(wt.join("change.txt"), "committed").await.unwrap();
    let _ = git_cmd(&["add", "change.txt"], &wt).await.unwrap();
    let empty_env = std::ffi::OsStr::new("");
    let commit = git_cmd_env(
        &["commit", "-m", "commit from worktree"],
        &wt,
        &[("LUCIDOS_THREAD_ID", empty_env)],
    )
    .await
    .unwrap();
    assert!(
        commit.status.success(),
        "fixture commit failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );
    assert_eq!(
        tokio::fs::read_to_string(&chained_log).await.unwrap(),
        "chained",
        "existing post-commit hook must still run after Lucidos installs its hook"
    );
}

#[tokio::test]
async fn install_coding_agent_diff_hook_preserves_prior_custom_hooks_path() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-custom-hook");
    let out = worktree_add(&repo, &wt, &["-b", "claude-code/custom-hook"])
        .await
        .unwrap();
    assert!(out.status.success(), "fixture worktree_add failed");

    let custom_dir = wt.join(".custom-hooks");
    let custom_hook = custom_dir.join("post-commit");
    tokio::fs::create_dir_all(&custom_dir).await.unwrap();
    tokio::fs::write(&custom_hook, "#!/bin/sh\nexit 0\n").await.unwrap();
    #[cfg(unix)]
    make_executable(&custom_hook);

    let enable = git_cmd(&["config", "extensions.worktreeConfig", "true"], &repo)
        .await
        .unwrap();
    assert!(enable.status.success());
    let set_custom = git_cmd(&["config", "--worktree", "core.hooksPath", ".custom-hooks"], &wt)
        .await
        .unwrap();
    assert!(set_custom.status.success());

    install_coding_agent_diff_hook(&repo, &wt).await.unwrap();
    install_coding_agent_diff_hook(&repo, &wt).await.unwrap();

    let hook_script = tokio::fs::read_to_string(wt.join(".lucidos/git-hooks/post-commit"))
        .await
        .unwrap();
    assert!(
        hook_script.contains(&custom_hook.to_string_lossy().to_string()),
        "reinstall must preserve the originally chained custom hooksPath"
    );
    assert!(
        !hook_script.contains(".lucidos/git-hooks/post-commit\nCHAIN='.lucidos/git-hooks/post-commit'"),
        "reinstall must not chain the Lucidos hook to itself"
    );
}
