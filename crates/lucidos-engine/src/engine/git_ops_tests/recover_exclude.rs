use super::*;
use super::common::make_test_repo;

/// Helper: create a branch ref pointing at main with no extra commits
/// (simulates the corrupted state where CC never committed its work).
async fn make_empty_branch(repo: &Path, branch: &str) {
    let _ = git_cmd(&["branch", branch, "main"], repo).await;
}

/// Branch with no commits + no worktree on disk + non-empty `change.files`
/// is the exact corrupted state from the incident — must NOT be silently
/// marked applied.
#[tokio::test]
async fn recover_no_commits_branch_errors_when_files_declared_but_branch_empty() {
    let (_tmp, repo) = make_test_repo().await;
    make_empty_branch(&repo, "claude-code/incident").await;

    let files = vec![
        "src/a.tsx".to_string(),
        "src/b.tsx".to_string(),
        "src/c.tsx".to_string(),
    ];
    let result = recover_no_commits_branch(&repo, "claude-code/incident", &files).await;
    let err = result
        .expect_err("must refuse to silently apply when files declared")
        .to_string();
    assert!(
        err.contains("no commits"),
        "error must explain the state: {}",
        err
    );
    assert!(
        err.contains("3 file"),
        "error must mention the file count: {}",
        err
    );
}

/// Branch with no commits + empty `change.files` is a legitimate no-op
/// (e.g. CC proposed nothing). Must still apply silently.
#[tokio::test]
async fn recover_no_commits_branch_legitimate_noop_when_files_empty() {
    let (_tmp, repo) = make_test_repo().await;
    make_empty_branch(&repo, "claude-code/empty").await;

    let result = recover_no_commits_branch(&repo, "claude-code/empty", &[]).await;
    assert_eq!(result.unwrap(), NoCommitsRecovery::LegitimateNoOp);
}

/// Branch with no commits + worktree on disk holding uncommitted work:
/// the helper must auto-commit the work and report `AutoCommitted` so
/// the merge path takes over instead of discarding the work.
#[tokio::test]
async fn recover_no_commits_branch_auto_commits_dirty_worktree() {
    let (_tmp, repo) = make_test_repo().await;
    make_empty_branch(&repo, "claude-code/dirty").await;

    // Add a worktree for the empty branch and write an uncommitted file
    let wt = repo.join("wt-dirty");
    let wt_str = wt.to_str().unwrap();
    let add_result = git_cmd(&["worktree", "add", wt_str, "claude-code/dirty"], &repo)
        .await
        .unwrap();
    assert!(
        add_result.status.success(),
        "worktree add failed: {}",
        String::from_utf8_lossy(&add_result.stderr)
    );

    tokio::fs::write(wt.join("draft.tsx"), "const x = 1;")
        .await
        .unwrap();
    // Branch ref still points at main — not yet committed
    assert!(
        !has_branch_commits(&repo, "claude-code/dirty").await,
        "precondition: branch must have no commits"
    );

    let files = vec!["draft.tsx".to_string()];
    let result = recover_no_commits_branch(&repo, "claude-code/dirty", &files).await;
    assert_eq!(result.unwrap(), NoCommitsRecovery::AutoCommitted);

    // Now the branch must have a commit, rescuing the work
    assert!(
        has_branch_commits(&repo, "claude-code/dirty").await,
        "after recovery, branch must have commits"
    );
}

/// Branch with no commits + clean worktree on disk + non-empty `change.files`
/// is still corrupted (the worktree had nothing to commit) — must error.
#[tokio::test]
async fn recover_no_commits_branch_errors_when_worktree_clean_but_files_declared() {
    let (_tmp, repo) = make_test_repo().await;
    make_empty_branch(&repo, "claude-code/clean-empty").await;

    let wt = repo.join("wt-clean");
    let add_result = git_cmd(
        &[
            "worktree",
            "add",
            wt.to_str().unwrap(),
            "claude-code/clean-empty",
        ],
        &repo,
    )
    .await
    .unwrap();
    assert!(add_result.status.success());

    let files = vec!["src/a.tsx".to_string()];
    let result = recover_no_commits_branch(&repo, "claude-code/clean-empty", &files).await;
    assert!(
        result.is_err(),
        "clean worktree + declared files must error"
    );
}

/// Branch had real commits that were already merged into main (e.g. via
/// a sibling apply). The branch ref still exists but has no unique commits
/// over main — `has_branch_commits` returns false. Since main already
/// contains the work, treat as a no-op success rather than refusing with a
/// "discard manually" error.
#[tokio::test]
async fn recover_no_commits_branch_already_applied_when_branch_merged_to_main() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "claude-code/feature"], &repo).await;
    tokio::fs::write(repo.join("feature.rs"), "fn feature() {}")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    let _ = git_cmd(
        &[
            "merge",
            "--no-ff",
            "claude-code/feature",
            "-m",
            "Merge branch 'claude-code/feature'",
        ],
        &repo,
    )
    .await;

    assert!(
        !has_branch_commits(&repo, "claude-code/feature").await,
        "precondition: branch must have no unique commits over main"
    );

    let files = vec!["feature.rs".to_string()];
    let result = recover_no_commits_branch(&repo, "claude-code/feature", &files).await;
    assert_eq!(
        result.unwrap(),
        NoCommitsRecovery::AlreadyApplied,
        "branch fully merged into main with referenced files in history must be a no-op"
    );
}

/// Same as above but with a fast-forward merge — branch's tip becomes main's
/// tip exactly (no merge commit). The work is still on main, so apply must
/// no-op rather than error.
#[tokio::test]
async fn recover_no_commits_branch_already_applied_after_fast_forward() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "claude-code/ff-feature"], &repo).await;
    tokio::fs::write(repo.join("ff.rs"), "fn ff() {}")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add ff feature"], &repo).await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    let _ = git_cmd(&["merge", "--ff-only", "claude-code/ff-feature"], &repo).await;

    assert!(
        !has_branch_commits(&repo, "claude-code/ff-feature").await,
        "precondition: branch must have no unique commits over main"
    );

    let files = vec!["ff.rs".to_string()];
    let result = recover_no_commits_branch(&repo, "claude-code/ff-feature", &files).await;
    assert_eq!(
        result.unwrap(),
        NoCommitsRecovery::AlreadyApplied,
        "fast-forward-merged branch with referenced files must be a no-op"
    );
}

#[test]
fn repo_at_different_path_is_external() {
    let dev = std::path::PathBuf::from("/Users/me/IdeaProjects/lucidos");
    let other = std::path::PathBuf::from("/Users/me/IdeaProjects/example-repo");
    assert!(is_external_repo_path(&other, &dev));
}

#[test]
fn trailing_slash_does_not_make_repo_external() {
    let dev = std::path::PathBuf::from("/Users/me/IdeaProjects/lucidos");
    let with_slash = std::path::PathBuf::from("/Users/me/IdeaProjects/lucidos/");
    assert!(!is_external_repo_path(&with_slash, &dev));
}

/// `find_worktree_for_branch` returns the path of the worktree currently
/// holding `branch_name`, regardless of where on disk it lives. This is the
/// lookup the apply path needs in order to reuse an existing CC worktree
/// instead of failing with "branch already used by worktree" when calling
/// `git worktree add` on a still-checked-out branch.
#[tokio::test]
async fn find_worktree_for_branch_returns_path_when_branch_is_checked_out() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["branch", "claude-code/feat"], &repo).await;
    let wt = _tmp.path().join("some/nested/worktree-path");
    let add = git_cmd(
        &[
            "worktree",
            "add",
            wt.to_str().unwrap(),
            "claude-code/feat",
        ],
        &repo,
    )
    .await
    .unwrap();
    assert!(add.status.success(), "setup: worktree add must succeed");

    let found = find_worktree_for_branch(&repo, "claude-code/feat").await;
    assert!(
        found.is_some(),
        "should detect the worktree holding the branch"
    );
    // git canonicalizes the worktree path; compare via canonicalize to avoid
    // /private/var vs /var symlink mismatches on macOS.
    let expected = std::fs::canonicalize(&wt).unwrap();
    let actual = std::fs::canonicalize(found.unwrap()).unwrap();
    assert_eq!(actual, expected);
}

/// No worktree holds the branch → None. The branch may exist as a ref or not;
/// either way the function only reports active worktrees.
#[tokio::test]
async fn find_worktree_for_branch_returns_none_when_branch_not_checked_out() {
    let (_tmp, repo) = make_test_repo().await;
    let _ = git_cmd(&["branch", "claude-code/unattached"], &repo).await;

    let found = find_worktree_for_branch(&repo, "claude-code/unattached").await;
    assert!(found.is_none(), "branch ref alone must not match");

    let found = find_worktree_for_branch(&repo, "claude-code/missing").await;
    assert!(found.is_none(), "missing branch must not match");
}

/// Read git's effective info/exclude file for a repo or worktree, as resolved
/// by git itself (`git rev-parse --git-path info/exclude`). Worktrees share
/// the common .git/info/exclude — git silently ignores per-worktree copies.
async fn read_exclude_file(wt_path: &std::path::Path) -> String {
    let out = git_cmd(&["rev-parse", "--git-path", "info/exclude"], wt_path)
        .await
        .unwrap();
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let path = if std::path::Path::new(&raw).is_absolute() {
        std::path::PathBuf::from(raw)
    } else {
        wt_path.join(raw)
    };
    tokio::fs::read_to_string(path).await.unwrap_or_default()
}

#[tokio::test]
async fn add_paths_to_worktree_exclude_writes_each_path_to_empty_file() {
    let (_tmp, repo) = make_test_repo().await;

    add_paths_to_worktree_exclude(&repo, &[".lucidos-workspace", ".claude/skills/lucidos-cli/"])
        .await;

    let body = read_exclude_file(&repo).await;
    let lines: Vec<&str> = body.lines().filter(|l| !l.is_empty()).collect();
    assert!(lines.contains(&".lucidos-workspace"), "missing marker: {body}");
    assert!(
        lines.contains(&".claude/skills/lucidos-cli/"),
        "missing skill dir: {body}"
    );
}

#[tokio::test]
async fn add_paths_to_worktree_exclude_preserves_existing_entries() {
    let (_tmp, repo) = make_test_repo().await;

    let exclude = repo.join(".git/info/exclude");
    tokio::fs::create_dir_all(exclude.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&exclude, "# pre-existing\nuser-custom-glob\n")
        .await
        .unwrap();

    add_paths_to_worktree_exclude(&repo, &[".lucidos-workspace"]).await;

    let body = tokio::fs::read_to_string(&exclude).await.unwrap();
    assert!(
        body.contains("# pre-existing"),
        "pre-existing comment lost: {body}"
    );
    assert!(
        body.contains("user-custom-glob"),
        "pre-existing glob lost: {body}"
    );
    assert!(
        body.lines().any(|l| l.trim() == ".lucidos-workspace"),
        "marker not appended: {body}"
    );
}

#[tokio::test]
async fn add_paths_to_worktree_exclude_is_idempotent() {
    let (_tmp, repo) = make_test_repo().await;

    let paths = &[".lucidos-workspace", ".claude/skills/lucidos-cli/"];
    add_paths_to_worktree_exclude(&repo, paths).await;
    add_paths_to_worktree_exclude(&repo, paths).await;
    add_paths_to_worktree_exclude(&repo, paths).await;

    let body = read_exclude_file(&repo).await;
    let marker_count = body
        .lines()
        .filter(|l| l.trim() == ".lucidos-workspace")
        .count();
    let skill_count = body
        .lines()
        .filter(|l| l.trim() == ".claude/skills/lucidos-cli/")
        .count();
    assert_eq!(marker_count, 1, "marker duplicated: {body}");
    assert_eq!(skill_count, 1, "skill dir duplicated: {body}");
}

/// Regression test for the fact that git silently ignores per-worktree
/// `info/exclude` files — exclude entries must land in the COMMON
/// .git/info/exclude or `git status` won't honor them. This test exercises
/// git's actual behavior, not just the file the helper writes to.
#[tokio::test]
async fn add_paths_to_worktree_exclude_makes_git_status_honor_paths_in_worktree() {
    let (_tmp, repo) = make_test_repo().await;

    let wt = _tmp.path().join("wt");
    let _ = git_cmd(
        &[
            "worktree",
            "add",
            wt.to_str().unwrap(),
            "-b",
            "claude-code/test-branch",
        ],
        &repo,
    )
    .await;

    let git_marker = wt.join(".git");
    assert!(
        tokio::fs::metadata(&git_marker)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false),
        "expected worktree .git to be a gitlink file"
    );

    tokio::fs::write(wt.join(".lucidos-workspace"), "marker")
        .await
        .unwrap();
    tokio::fs::create_dir_all(wt.join(".claude/skills/lucidos-cli"))
        .await
        .unwrap();
    tokio::fs::write(
        wt.join(".claude/skills/lucidos-cli/SKILL.md"),
        "skill content",
    )
    .await
    .unwrap();

    let status_before =
        String::from_utf8_lossy(&git_cmd(&["status", "--porcelain"], &wt).await.unwrap().stdout)
            .into_owned();
    assert!(
        status_before.contains(".lucidos-workspace"),
        "test setup: marker should be untracked before exclude write: {status_before}"
    );

    add_paths_to_worktree_exclude(&wt, &[".lucidos-workspace", ".claude/skills/lucidos-cli/"])
        .await;

    let body = read_exclude_file(&wt).await;
    assert!(
        body.lines().any(|l| l.trim() == ".lucidos-workspace"),
        "marker missing in resolved exclude file: {body}"
    );
    assert!(
        body.lines()
            .any(|l| l.trim() == ".claude/skills/lucidos-cli/"),
        "skill dir missing in resolved exclude file: {body}"
    );

    let status_after =
        String::from_utf8_lossy(&git_cmd(&["status", "--porcelain"], &wt).await.unwrap().stdout)
            .into_owned();
    assert!(
        !status_after.contains(".lucidos-workspace"),
        "marker still untracked after exclude write — git is not honoring our exclude entries: {status_after}"
    );
    assert!(
        !status_after.contains(".claude/skills"),
        "skill dir still untracked after exclude write — git is not honoring our exclude entries: {status_after}"
    );
}

/// Regression test for the app-coding-agent thread bug: CC runs inside
/// `<wt>/data/apps/<id>/` so `install_lucidos_cli_skill` writes the SKILL.md
/// at a DEEP path (`data/apps/<id>/.claude/skills/lucidos-cli/SKILL.md`).
/// The `.git/info/exclude` entry must therefore match at any depth, not just
/// at the worktree root. Exercises `git status` against `WORKTREE_EXCLUDE_PATHS`
/// itself to keep the constant and the matching semantics in lockstep.
#[tokio::test]
async fn worktree_exclude_paths_hide_deep_app_lucidos_cli_skill_from_git_status() {
    use super::super::claude_code::WORKTREE_EXCLUDE_PATHS;

    let (_tmp, repo) = make_test_repo().await;

    let wt = _tmp.path().join("wt-app");
    let _ = git_cmd(
        &[
            "worktree",
            "add",
            wt.to_str().unwrap(),
            "-b",
            "claude-code/app/habit-tracker/test-branch",
        ],
        &repo,
    )
    .await;

    let deep_skill_dir = wt.join("data/apps/habit-tracker/.claude/skills/lucidos-cli");
    tokio::fs::create_dir_all(&deep_skill_dir).await.unwrap();
    tokio::fs::write(deep_skill_dir.join("SKILL.md"), "skill content")
        .await
        .unwrap();

    // `-uall` expands untracked directories so we can match the deep path
    // explicitly instead of the collapsed `?? data/` summary.
    let status_before = String::from_utf8_lossy(
        &git_cmd(&["status", "--porcelain", "-uall"], &wt)
            .await
            .unwrap()
            .stdout,
    )
    .into_owned();
    assert!(
        status_before.contains(
            "data/apps/habit-tracker/.claude/skills/lucidos-cli/SKILL.md"
        ),
        "test setup: deep skill file should be untracked before exclude write: {status_before}"
    );

    add_paths_to_worktree_exclude(&wt, WORKTREE_EXCLUDE_PATHS).await;

    let status_after = String::from_utf8_lossy(
        &git_cmd(&["status", "--porcelain", "-uall"], &wt)
            .await
            .unwrap()
            .stdout,
    )
    .into_owned();
    assert!(
        !status_after.contains("data/apps/habit-tracker/.claude"),
        "deep skill file still untracked — gitignore pattern in WORKTREE_EXCLUDE_PATHS \
         does not match at depth; app coding-agent threads will keep auto-committing \
         the engine-injected SKILL.md as a phantom user change: {status_after}"
    );
}

#[tokio::test]
async fn add_paths_to_worktree_exclude_appends_only_missing_paths() {
    let (_tmp, repo) = make_test_repo().await;

    add_paths_to_worktree_exclude(&repo, &[".lucidos-workspace"]).await;
    add_paths_to_worktree_exclude(
        &repo,
        &[".lucidos-workspace", ".claude/skills/lucidos-cli/"],
    )
    .await;

    let body = read_exclude_file(&repo).await;
    let marker_count = body
        .lines()
        .filter(|l| l.trim() == ".lucidos-workspace")
        .count();
    let skill_count = body
        .lines()
        .filter(|l| l.trim() == ".claude/skills/lucidos-cli/")
        .count();
    assert_eq!(marker_count, 1, "marker duplicated: {body}");
    assert_eq!(skill_count, 1, "skill dir missing or duplicated: {body}");
}

/// `.lucidos/bin/lucidos` is a symlink the engine drops into every worktree
/// so spawned scripts can invoke the CLI. External repos don't have `.lucidos/`
/// in their `.gitignore`, so without filtering the symlink ends up in
/// auto-commits and renders as a fake "diff" in change proposals. Filter at
/// the `branch_changed_files` boundary so already-committed instances also
/// disappear from the change list.
#[tokio::test]
async fn branch_changed_files_filters_lucidos_runtime_paths() {
    let (_tmp, repo) = make_test_repo().await;

    // Branch with a real change AND every engine-injected path committed:
    // `.lucidos-workspace` (marker), `.lucidos/bin/lucidos` (CLI symlink),
    // and `.claude/skills/lucidos-cli/SKILL.md` (skill).
    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await.unwrap();
    tokio::fs::write(repo.join("real.txt"), "real change")
        .await
        .unwrap();
    tokio::fs::write(repo.join(".lucidos-workspace"), "ws-marker")
        .await
        .unwrap();
    tokio::fs::create_dir_all(repo.join(".lucidos/bin")).await.unwrap();
    tokio::fs::write(repo.join(".lucidos/bin/lucidos"), "")
        .await
        .unwrap();
    tokio::fs::create_dir_all(repo.join(".claude/skills/lucidos-cli"))
        .await
        .unwrap();
    tokio::fs::write(
        repo.join(".claude/skills/lucidos-cli/SKILL.md"),
        "skill body",
    )
    .await
    .unwrap();
    let _ = git_cmd(&["add", "-A"], &repo).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "Coding agent changes"], &repo)
        .await
        .unwrap();

    let files = branch_changed_files(&repo, "feature").await;
    assert_eq!(
        files,
        vec!["real.txt".to_string()],
        "all engine-injected paths must be filtered from branch_changed_files, got: {:?}",
        files
    );
}

/// Install an executable `pre-commit` hook that always exits non-zero so a
/// `git commit` against the repo fails — used to simulate hook-driven commit
/// rejection (e.g. lint, secrets scan) in unit tests.
#[cfg(unix)]
async fn install_failing_pre_commit_hook(repo: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let hooks_dir = repo.join(".git/hooks");
    tokio::fs::create_dir_all(&hooks_dir).await.unwrap();
    let hook_path = hooks_dir.join("pre-commit");
    tokio::fs::write(
        &hook_path,
        "#!/bin/sh\necho 'pre-commit hook blocked commit' >&2\nexit 1\n",
    )
    .await
    .unwrap();
    let mut perms = tokio::fs::metadata(&hook_path)
        .await
        .unwrap()
        .permissions();
    perms.set_mode(0o755);
    tokio::fs::set_permissions(&hook_path, perms).await.unwrap();
}

/// Regression for `harden-engine-apply-now-wait-and-commit-swallows-git-failures`:
/// a `git commit` failure (e.g. a pre-commit hook rejecting the commit) must
/// surface as `Err` instead of being silently dropped. Before the fix,
/// `wait_and_commit` invoked `let _ = git_cmd(["commit", ...])` and returned
/// `Ok(())`, so a real CC change would disappear without trace.
#[cfg(unix)]
#[tokio::test]
async fn commit_worktree_or_err_propagates_pre_commit_hook_failure() {
    let (_tmp, repo) = make_test_repo().await;
    tokio::fs::write(repo.join("change.txt"), "new content")
        .await
        .unwrap();
    install_failing_pre_commit_hook(&repo).await;

    let result = commit_worktree_or_err(&repo, "should fail").await;
    let err = result.expect_err("commit must surface pre-commit-hook failure as Err");
    assert!(
        err.contains("git commit"),
        "error should mention git commit failure: {}",
        err
    );
    assert!(
        err.contains("pre-commit hook blocked commit"),
        "error should carry the hook's stderr so callers can log it: {}",
        err
    );
}

/// `commit_worktree_or_err` returns `Ok(false)` when the worktree is clean —
/// no commit is attempted, no error is raised.
#[tokio::test]
async fn commit_worktree_or_err_returns_false_when_worktree_clean() {
    let (_tmp, repo) = make_test_repo().await;
    let result = commit_worktree_or_err(&repo, "no-op").await;
    assert_eq!(
        result.ok(),
        Some(false),
        "clean worktree must report no commit was made"
    );
}

/// `commit_worktree_or_err` commits staged + unstaged work and returns
/// `Ok(true)`.
#[tokio::test]
async fn commit_worktree_or_err_commits_dirty_worktree() {
    let (_tmp, repo) = make_test_repo().await;
    tokio::fs::write(repo.join("file.txt"), "content")
        .await
        .unwrap();

    let result = commit_worktree_or_err(&repo, "test commit").await;
    assert_eq!(
        result.ok(),
        Some(true),
        "dirty worktree must report a commit was made"
    );

    let log = git_cmd(&["log", "--oneline", "-1"], &repo).await.unwrap();
    assert!(
        String::from_utf8_lossy(&log.stdout).contains("test commit"),
        "commit message must land in git log: {}",
        String::from_utf8_lossy(&log.stdout)
    );
}

/// Set up a repo where `feature` has been merged into `main` with
/// `--no-commit`, leaving `MERGE_HEAD` and `.git/MERGE_MSG` populated — the
/// exact state the conflict-resolution path reaches before calling
/// `git_commit_no_edit`. Returns the (tempdir, repo path) pair.
async fn make_repo_with_pending_merge() -> (tempfile::TempDir, std::path::PathBuf) {
    let (tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await.unwrap();
    tokio::fs::write(repo.join("feature.txt"), "feature work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "-A"], &repo).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "feature commit"], &repo)
        .await
        .unwrap();

    let _ = git_cmd(&["checkout", "main"], &repo).await.unwrap();
    tokio::fs::write(repo.join("main-only.txt"), "main divergence")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "-A"], &repo).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "main commit"], &repo)
        .await
        .unwrap();

    let _ = git_cmd(
        &["merge", "--no-ff", "--no-commit", "feature"],
        &repo,
    )
    .await
    .unwrap();

    (tmp, repo)
}

/// Regression for `harden-engine-run-session-merge-commit-no-edit-swallowed`:
/// `git_commit_no_edit` must return `Err` when the commit fails so the caller
/// can log the original stderr. Before the fix, the merge-resolution path used
/// `let _ = git_cmd(["commit", "--no-edit"], ...)`, hiding the root cause
/// behind a follow-up `ff_merge_to_main` failure.
#[cfg(unix)]
#[tokio::test]
async fn git_commit_no_edit_surfaces_failure_for_logging() {
    let (_tmp, repo) = make_repo_with_pending_merge().await;
    install_failing_pre_commit_hook(&repo).await;

    let result = git_commit_no_edit(&repo).await;
    let err = result.expect_err("git_commit_no_edit must surface hook failure");
    assert!(
        err.contains("git commit --no-edit failed"),
        "error must identify which git invocation failed: {}",
        err
    );
    assert!(
        err.contains("pre-commit hook blocked commit"),
        "error must carry hook stderr for log triage: {}",
        err
    );
}

/// `git_commit_no_edit` returns `Ok(())` after finalising a pending merge so
/// the conflict-resolution path can proceed to `ff_merge_to_main`.
#[tokio::test]
async fn git_commit_no_edit_finalises_pending_merge() {
    let (_tmp, repo) = make_repo_with_pending_merge().await;

    let result = git_commit_no_edit(&repo).await;
    assert!(
        result.is_ok(),
        "git_commit_no_edit must finalise the pending merge: {:?}",
        result
    );

    let head_parents = git_cmd(&["log", "-1", "--pretty=%P"], &repo)
        .await
        .unwrap();
    let parents = String::from_utf8_lossy(&head_parents.stdout);
    assert_eq!(
        parents.split_whitespace().count(),
        2,
        "finalised merge commit must have two parents: {}",
        parents
    );
}

const PHANTOM_SKILL_REL: &str = ".claude/skills/lucidos-cli/SKILL.md";

/// Add a worktree on a fresh branch and return its path. Nested under the
/// repo's own tempdir so each test gets a unique path (the test repo IS the
/// tempdir, so siblings would collide in the shared tmp base).
async fn add_worktree(repo: &std::path::Path, branch: &str, name: &str) -> std::path::PathBuf {
    let wt = repo.join(name);
    let add = git_cmd(
        &["worktree", "add", wt.to_str().unwrap(), "-b", branch],
        repo,
    )
    .await
    .unwrap();
    assert!(
        add.status.success(),
        "worktree add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    wt
}

async fn porcelain_for(wt: &std::path::Path, rel: &str) -> String {
    String::from_utf8_lossy(
        &git_cmd(&["status", "--porcelain", "--", rel], wt)
            .await
            .unwrap()
            .stdout,
    )
    .into_owned()
}

/// The habit-tracker-app bug: an engine-injected `SKILL.md` that was auto-committed
/// in an earlier session is now tracked-but-stale. The engine overwrites it on
/// disk with its newer embedded copy → phantom `M`. `.git/info/exclude` can't
/// hide a tracked path, so `hide_phantom_tracked_skill` must skip-worktree it so
/// the CC session never sees the change.
#[tokio::test]
async fn hide_phantom_tracked_skill_hides_modified_tracked_skill() {
    let (_tmp, repo) = make_test_repo().await;
    let wt = add_worktree(&repo, "claude-code/phantom", "wt-phantom").await;

    let skill = wt.join(PHANTOM_SKILL_REL);
    tokio::fs::create_dir_all(skill.parent().unwrap())
        .await
        .unwrap();
    // Stale copy committed in a prior session (simulates the auto-commit).
    tokio::fs::write(&skill, "stale skill body\n").await.unwrap();
    let _ = git_cmd(&["add", "--", PHANTOM_SKILL_REL], &wt).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "auto-committed skill"], &wt)
        .await
        .unwrap();

    // Engine overwrites with its newer embedded copy at session start.
    tokio::fs::write(&skill, "newer skill body — adds `lucidos changes list`\n")
        .await
        .unwrap();
    let before = porcelain_for(&wt, PHANTOM_SKILL_REL).await;
    assert!(
        before.contains("SKILL.md"),
        "precondition: phantom modification must be visible before the guard: {before}"
    );

    hide_phantom_tracked_skill(&wt, PHANTOM_SKILL_REL).await;

    let after = porcelain_for(&wt, PHANTOM_SKILL_REL).await;
    assert!(
        after.trim().is_empty(),
        "phantom skill modification must be hidden from git status after the guard: {after}"
    );
}

/// The Lucidos-repo case: `SKILL.md` is intentionally tracked and (because the
/// embedded copy is byte-identical to the committed one) shows NO divergence at
/// session start. The guard must leave it alone so a later legitimate edit to
/// the skill source is still seen by git — skip-worktree here would silently
/// swallow real work.
#[tokio::test]
async fn hide_phantom_tracked_skill_leaves_clean_tracked_skill_editable() {
    let (_tmp, repo) = make_test_repo().await;
    let wt = add_worktree(&repo, "claude-code/clean", "wt-clean").await;

    let skill = wt.join(PHANTOM_SKILL_REL);
    tokio::fs::create_dir_all(skill.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&skill, "skill body\n").await.unwrap();
    let _ = git_cmd(&["add", "--", PHANTOM_SKILL_REL], &wt).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "tracked skill"], &wt)
        .await
        .unwrap();

    // No divergence at guard time (embedded == committed): must be a no-op.
    hide_phantom_tracked_skill(&wt, PHANTOM_SKILL_REL).await;

    // A subsequent real edit must remain visible to git.
    tokio::fs::write(&skill, "edited skill body\n").await.unwrap();
    let status = porcelain_for(&wt, PHANTOM_SKILL_REL).await;
    assert!(
        status.contains("SKILL.md"),
        "a real edit to a clean tracked skill must stay visible — guard must not skip-worktree it: {status}"
    );
}

/// The real habit-tracker scenario: an *app coding-agent thread* runs CC inside the
/// deep app folder (`data/apps/<id>/`), so the guard's `cwd` is a subdirectory
/// of the worktree and the skill path is resolved cwd-relative. The phantom must
/// still be hidden — pins that `git update-index --skip-worktree` works from a
/// subdir, not just the worktree root the other tests use.
#[tokio::test]
async fn hide_phantom_tracked_skill_hides_deep_app_skill_from_subdir_cwd() {
    let (_tmp, repo) = make_test_repo().await;
    let wt = add_worktree(&repo, "claude-code/app/habit-tracker/x", "wt-deep").await;

    let app_cwd = wt.join("data/apps/habit-tracker");
    let skill = app_cwd.join(PHANTOM_SKILL_REL);
    tokio::fs::create_dir_all(skill.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&skill, "stale skill body\n").await.unwrap();
    // Commit from the worktree root with the full deep path.
    let deep_rel = "data/apps/habit-tracker/.claude/skills/lucidos-cli/SKILL.md";
    let _ = git_cmd(&["add", "--", deep_rel], &wt).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "auto-committed deep skill"], &wt)
        .await
        .unwrap();

    tokio::fs::write(&skill, "newer embedded skill body\n")
        .await
        .unwrap();

    // Guard runs with the app folder as cwd and the cwd-relative skill path,
    // exactly as the spawn path invokes it for an app thread.
    hide_phantom_tracked_skill(&app_cwd, PHANTOM_SKILL_REL).await;

    let after = porcelain_for(&wt, deep_rel).await;
    assert!(
        after.trim().is_empty(),
        "phantom deep app skill must be hidden when the guard runs from the app-folder cwd: {after}"
    );
}

/// An untracked injected skill (the normal external-repo / post-cleanup case)
/// is already handled by `.git/info/exclude`. The guard must no-op: never error,
/// never start tracking the file.
#[tokio::test]
async fn hide_phantom_tracked_skill_noop_when_untracked() {
    let (_tmp, repo) = make_test_repo().await;
    let wt = add_worktree(&repo, "claude-code/untracked", "wt-untracked").await;

    let skill = wt.join(PHANTOM_SKILL_REL);
    tokio::fs::create_dir_all(skill.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&skill, "injected skill\n").await.unwrap();

    hide_phantom_tracked_skill(&wt, PHANTOM_SKILL_REL).await;

    let tracked = git_cmd(&["ls-files", "--", PHANTOM_SKILL_REL], &wt)
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&tracked.stdout).trim().is_empty(),
        "guard must not start tracking a previously-untracked injected skill"
    );
}
