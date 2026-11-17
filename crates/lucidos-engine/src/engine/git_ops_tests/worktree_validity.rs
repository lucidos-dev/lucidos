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
        if p.is_absolute() {
            p
        } else {
            wt.join(p)
        }
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

/// Commit a `.gitattributes` routing every file through a smudge filter that
/// always fails, so any checkout in a new worktree of `repo` fails.
async fn break_checkout(repo: &std::path::Path) {
    tokio::fs::write(repo.join(".gitattributes"), "* filter=boom\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", ".gitattributes"], repo).await;
    let _ = git_cmd(&["commit", "-m", "route files through boom"], repo).await;
    let _ = git_cmd(&["config", "filter.boom.smudge", "false"], repo).await;
    let _ = git_cmd(&["config", "filter.boom.required", "true"], repo).await;
}

#[tokio::test]
async fn worktree_add_removes_the_tree_and_branch_it_added_when_checkout_fails() {
    let (_tmp, repo) = make_test_repo().await;
    break_checkout(&repo).await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-unpopulated");

    let out = worktree_add(&repo, &wt, &["-b", "claude-code/unpopulated"])
        .await
        .unwrap();

    assert!(!out.status.success(), "the fixture must make checkout fail");
    assert!(
        !is_live_worktree_at(&wt).await,
        "a tree whose checkout failed must not stay registered for reuse"
    );
    assert!(
        !git_ref_exists(&repo, "refs/heads/claude-code/unpopulated").await,
        "the -b branch this call created must be deleted with the tree"
    );
}

#[tokio::test]
async fn worktree_add_keeps_a_reused_branch_when_checkout_fails() {
    let (_tmp, repo) = make_test_repo().await;
    break_checkout(&repo).await;
    let _ = git_cmd(&["branch", "claude-code/existing"], &repo).await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-reuse");

    let out = worktree_add(&repo, &wt, &["claude-code/existing"])
        .await
        .unwrap();

    assert!(!out.status.success(), "the fixture must make checkout fail");
    assert!(!is_live_worktree_at(&wt).await);
    assert!(
        git_ref_exists(&repo, "refs/heads/claude-code/existing").await,
        "a branch the call only reused must survive the cleanup"
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
    assert!(
        is_live_worktree_at(&wt).await,
        "precondition: live before stranding"
    );

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
    tokio::fs::write(inside.join("leftover.txt"), b"x")
        .await
        .unwrap();
    assert!(
        !is_live_worktree_at(&inside).await,
        "precondition: not live"
    );

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
    assert!(
        is_live_worktree_at(&wt).await,
        "precondition: live worktree"
    );

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
    tokio::fs::write(wt.join("work.txt"), b"committed")
        .await
        .unwrap();
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

    tokio::fs::write(wt.join("change.txt"), "committed")
        .await
        .unwrap();
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

fn write_test_hook(dir: &std::path::Path, name: &str, body: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    #[cfg(unix)]
    make_executable(&path);
}

async fn add_hook_test_worktree(
    repo: &std::path::Path,
    base: &std::path::Path,
    name: &str,
) -> std::path::PathBuf {
    let wt = base.join(name);
    let branch = format!("claude-code/{name}");
    let out = worktree_add(repo, &wt, &["-b", &branch]).await.unwrap();
    assert!(out.status.success(), "fixture worktree_add failed");
    wt
}

async fn commit_in_worktree(
    wt: &std::path::Path,
    file: &str,
    env: &[(&str, &std::ffi::OsStr)],
) -> std::process::Output {
    tokio::fs::write(wt.join(file), file).await.unwrap();
    let _ = git_cmd(&["add", file], wt).await.unwrap();
    git_cmd_env(&["commit", "-m", "agent commit"], wt, env)
        .await
        .unwrap()
}

/// UA-2327. The worktree's own `core.hooksPath` override reads back as the
/// effective value. A reinstall then kept chaining to `.git/hooks` after the
/// repo set its own hooks dir.
#[tokio::test]
async fn reinstall_follows_the_repos_hooks_path_not_the_worktree_override() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = add_hook_test_worktree(&repo, wt_base.path(), "stale-chain").await;

    install_coding_agent_diff_hook(&repo, &wt).await.unwrap();
    let out = git_cmd(&["config", "core.hooksPath", ".githooks"], &repo)
        .await
        .unwrap();
    assert!(out.status.success(), "fixture git config failed");
    install_coding_agent_diff_hook(&repo, &wt).await.unwrap();

    let hooks = wt.join(".lucidos/git-hooks");
    let post_commit = std::fs::read_to_string(hooks.join("post-commit")).unwrap();
    let chain = wt.join(".githooks/post-commit");
    assert!(
        post_commit.contains(&format!("CHAIN='{}'", chain.display())),
        "post-commit: {post_commit}"
    );
    let pre_commit = std::fs::read_to_string(hooks.join("pre-commit")).unwrap();
    let target = wt.join(".githooks/pre-commit");
    assert!(
        pre_commit.contains(&format!("HOOK='{}'", target.display())),
        "pre-commit: {pre_commit}"
    );
}

/// A relative `core.hooksPath` resolves against the worktree root, as git does.
/// Every hook keeps its arguments, stdin and exit status, and post-commit runs
/// both the Diff refresh and the repo's own hook.
#[tokio::test]
async fn a_relative_repo_hooks_path_runs_every_hook_with_args_and_stdin() {
    let (_tmp, repo) = make_test_repo().await;
    let out = git_cmd(&["config", "core.hooksPath", ".githooks"], &repo)
        .await
        .unwrap();
    assert!(out.status.success(), "fixture git config failed");
    let wt_base = tempfile::tempdir().unwrap();
    let wt = add_hook_test_worktree(&repo, wt_base.path(), "relative-hooks").await;
    let logs = wt_base.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let log = |name: &str| shell_single_quote_for_test(&logs.join(name).to_string_lossy());

    let repo_hooks = wt.join(".githooks");
    write_test_hook(
        &repo_hooks,
        "pre-commit",
        &format!(": > {}\n", log("pre-commit")),
    );
    write_test_hook(
        &repo_hooks,
        "commit-msg",
        &format!("cat \"$1\" > {}\n", log("commit-msg")),
    );
    write_test_hook(
        &repo_hooks,
        "post-commit",
        &format!(": > {}\n", log("post-commit")),
    );
    write_test_hook(
        &repo_hooks,
        "pre-push",
        &format!(
            "echo \"$1 $2\" > {}\ncat >> {}\n",
            log("pre-push"),
            log("pre-push")
        ),
    );
    // A fake `lucidos` on PATH records that the Diff refresh ran.
    let bin = wt_base.path().join("bin");
    write_test_hook(
        &bin,
        "lucidos",
        &format!("echo \"$1\" > {}\n", log("diff-hook")),
    );
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());

    install_coding_agent_diff_hook(&repo, &wt).await.unwrap();

    let env = [
        ("LUCIDOS_THREAD_ID", std::ffi::OsStr::new("agent-thread")),
        ("PATH", std::ffi::OsStr::new(&path)),
    ];
    let commit = commit_in_worktree(&wt, "change.txt", &env).await;
    assert!(
        commit.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );
    let read_log = |name: &str| std::fs::read_to_string(logs.join(name));
    assert!(read_log("pre-commit").is_ok(), "pre-commit never ran");
    assert_eq!(read_log("commit-msg").unwrap().trim(), "agent commit");
    assert!(
        read_log("post-commit").is_ok(),
        "repo post-commit never ran"
    );
    assert_eq!(
        read_log("diff-hook").unwrap().trim(),
        "coding-agent-diff-hook"
    );

    let remote = wt_base.path().join("remote.git");
    let out = git_cmd(&["init", "--bare", &remote.to_string_lossy()], &repo)
        .await
        .unwrap();
    assert!(out.status.success(), "fixture bare init failed");
    let remote_url = remote.to_string_lossy().to_string();
    let out = git_cmd(&["remote", "add", "origin", &remote_url], &repo)
        .await
        .unwrap();
    assert!(out.status.success(), "fixture remote add failed");
    let push = git_cmd_env(&["push", "origin", "HEAD:refs/heads/pushed"], &wt, &env)
        .await
        .unwrap();
    assert!(
        push.status.success(),
        "push failed: {}",
        String::from_utf8_lossy(&push.stderr)
    );
    let pre_push = read_log("pre-push").unwrap();
    assert!(
        pre_push.starts_with(&format!("origin {remote_url}")),
        "pre-push: {pre_push}"
    );
    assert!(
        pre_push.contains("refs/heads/pushed"),
        "pre-push stdin: {pre_push}"
    );
}

/// git runs no hooks under an empty `core.hooksPath`, so the worktree forwards
/// to none. The Diff refresh still installs.
#[tokio::test]
async fn an_empty_repo_hooks_path_forwards_no_hooks() {
    let (_tmp, repo) = make_test_repo().await;
    write_test_hook(&repo.join(".git/hooks"), "pre-commit", "exit 1\n");
    let out = git_cmd(&["config", "core.hooksPath", ""], &repo)
        .await
        .unwrap();
    assert!(out.status.success(), "fixture git config failed");
    let wt_base = tempfile::tempdir().unwrap();
    let wt = add_hook_test_worktree(&repo, wt_base.path(), "empty-hooks").await;

    install_coding_agent_diff_hook(&repo, &wt).await.unwrap();

    let hooks = wt.join(".lucidos/git-hooks");
    assert!(hooks.join("post-commit").exists());
    assert!(!hooks.join("pre-commit").exists());
    let env = [("LUCIDOS_THREAD_ID", std::ffi::OsStr::new("agent-thread"))];
    let commit = commit_in_worktree(&wt, "free.txt", &env).await;
    assert!(
        commit.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );
}

#[tokio::test]
async fn an_absolute_repo_hooks_path_failing_pre_commit_blocks_the_commit() {
    let (_tmp, repo) = make_test_repo().await;
    let hooks_dir = tempfile::tempdir().unwrap();
    write_test_hook(
        hooks_dir.path(),
        "pre-commit",
        "echo blocked by absolute hook >&2\nexit 1\n",
    );
    let hooks_value = hooks_dir.path().to_string_lossy().to_string();
    let out = git_cmd(&["config", "core.hooksPath", &hooks_value], &repo)
        .await
        .unwrap();
    assert!(out.status.success(), "fixture git config failed");
    let wt_base = tempfile::tempdir().unwrap();
    let wt = add_hook_test_worktree(&repo, wt_base.path(), "absolute-hooks").await;

    install_coding_agent_diff_hook(&repo, &wt).await.unwrap();

    let env = [("LUCIDOS_THREAD_ID", std::ffi::OsStr::new("agent-thread"))];
    let commit = commit_in_worktree(&wt, "blocked.txt", &env).await;
    assert!(!commit.status.success(), "a failing pre-commit must block");
    assert!(
        String::from_utf8_lossy(&commit.stderr).contains("blocked by absolute hook"),
        "stderr: {}",
        String::from_utf8_lossy(&commit.stderr)
    );
}

/// The worktree-local hooks dir replaces the repo's. It once held only
/// `post-commit`, so a repo's failing `pre-commit` (a secret scanner) no longer
/// blocked an agent's commit.
#[tokio::test]
async fn install_coding_agent_diff_hook_keeps_the_repos_pre_commit_gate() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-pre-commit");
    let out = worktree_add(&repo, &wt, &["-b", "claude-code/pre-commit"])
        .await
        .unwrap();
    assert!(out.status.success(), "fixture worktree_add failed");

    let pre_commit = repo.join(".git/hooks/pre-commit");
    tokio::fs::write(
        &pre_commit,
        "#!/bin/sh\necho blocked by scanner >&2\nexit 1\n",
    )
    .await
    .unwrap();
    #[cfg(unix)]
    make_executable(&pre_commit);

    install_coding_agent_diff_hook(&repo, &wt).await.unwrap();

    tokio::fs::write(wt.join("secret.txt"), "token")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "secret.txt"], &wt).await.unwrap();
    let agent_env = std::ffi::OsStr::new("agent-thread");
    let commit = git_cmd_env(
        &["commit", "-m", "commit from worktree"],
        &wt,
        &[("LUCIDOS_THREAD_ID", agent_env)],
    )
    .await
    .unwrap();
    assert!(
        !commit.status.success(),
        "the repo's failing pre-commit must still block an agent's commit"
    );
    assert!(
        String::from_utf8_lossy(&commit.stderr).contains("blocked by scanner"),
        "stderr: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    // The engine's own auto-commit runs outside a session and never ran the
    // repo's hooks, so a hook cannot refuse an Apply.
    let empty_env = std::ffi::OsStr::new("");
    let commit = git_cmd_env(
        &["commit", "-m", "engine auto-commit"],
        &wt,
        &[("LUCIDOS_THREAD_ID", empty_env)],
    )
    .await
    .unwrap();
    assert!(
        commit.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );
}

/// git expands a leading `~/` in `core.hooksPath` before it runs a hook. Read
/// raw, the engine forwarded to `<worktree>/~/...`, so the repo's hooks were
/// skipped with no sign.
#[tokio::test]
async fn install_coding_agent_diff_hook_forwards_to_a_home_relative_hooks_path() {
    let (_tmp, repo) = make_test_repo().await;
    let out = git_cmd(&["config", "core.hooksPath", "~/.githooks"], &repo)
        .await
        .unwrap();
    assert!(out.status.success(), "fixture git config failed");
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-home-hooks");
    let out = worktree_add(&repo, &wt, &["-b", "claude-code/home-hooks"])
        .await
        .unwrap();
    assert!(out.status.success(), "fixture worktree_add failed");

    install_coding_agent_diff_hook(&repo, &wt).await.unwrap();

    let forwarder = std::fs::read_to_string(wt.join(".lucidos/git-hooks/pre-commit")).unwrap();
    let home = std::env::var("HOME").expect("HOME is set");
    let target = std::path::Path::new(&home).join(".githooks/pre-commit");
    assert!(
        forwarder.contains(&format!("HOOK='{}'", target.display())),
        "forwarder: {forwarder}"
    );
}
