use super::common::make_test_repo;
use super::*;

#[tokio::test]
async fn detect_origin_returns_none_for_repo_without_remote() {
    // Create a temporary git repo with no remotes
    let tmp = tempfile::tempdir().unwrap();
    let repo_path = tmp.path();

    // git init
    let init = std::process::Command::new("git")
        .args(["init"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    assert!(init.status.success(), "git init failed");

    // Create an initial commit so HEAD exists
    std::process::Command::new("git")
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    // No remote -> should return None (branch from HEAD)
    let result = detect_origin_default_branch(repo_path).await;
    assert_eq!(
        result, None,
        "Repo without origin should return None, not a fallback ref"
    );
}

/// Run git in `dir`, asserting success, and return its trimmed stdout.
fn git_ok(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A clone of a fresh bare origin, with one pushed commit on `main`.
fn clone_with_origin() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    git_ok(
        tmp.path(),
        &["init", "--bare", "--initial-branch=main", "origin.git"],
    );
    git_ok(tmp.path(), &["clone", "-q", "origin.git", "work"]);
    git_ok(&work, &["config", "user.email", "test@example.com"]);
    git_ok(&work, &["config", "user.name", "Test"]);
    git_ok(&work, &["commit", "-q", "--allow-empty", "-m", "base"]);
    git_ok(&work, &["branch", "-M", "main"]);
    git_ok(&work, &["push", "-q", "-u", "origin", "main"]);
    (tmp, work)
}

/// Standing on another branch, a spawn must never rewind a local default
/// branch that holds commits origin lacks.
#[tokio::test]
async fn detect_origin_keeps_unpushed_commits_on_the_local_default_branch() {
    let (_tmp, work) = clone_with_origin();
    git_ok(&work, &["commit", "-q", "--allow-empty", "-m", "unpushed"]);
    let local_main = git_ok(&work, &["rev-parse", "refs/heads/main"]);
    git_ok(&work, &["checkout", "-q", "-b", "feature"]);

    assert_eq!(
        detect_origin_default_branch(&work).await.as_deref(),
        Some("origin/main")
    );
    assert_eq!(
        git_ok(&work, &["rev-parse", "refs/heads/main"]),
        local_main,
        "the unpushed commit must stay on local main"
    );
}

/// The control: a local default branch that is merely behind origin still
/// fast-forwards.
#[tokio::test]
async fn detect_origin_fast_forwards_a_local_default_branch_that_is_behind() {
    let (tmp, work) = clone_with_origin();
    git_ok(tmp.path(), &["clone", "-q", "origin.git", "other"]);
    let other = tmp.path().join("other");
    git_ok(&other, &["config", "user.email", "test@example.com"]);
    git_ok(&other, &["config", "user.name", "Test"]);
    git_ok(&other, &["commit", "-q", "--allow-empty", "-m", "newer"]);
    git_ok(&other, &["push", "-q", "origin", "main"]);
    let origin_main = git_ok(&other, &["rev-parse", "HEAD"]);
    git_ok(&work, &["checkout", "-q", "-b", "feature"]);

    detect_origin_default_branch(&work).await;
    assert_eq!(
        git_ok(&work, &["rev-parse", "refs/heads/main"]),
        origin_main
    );
}

#[tokio::test]
async fn worktree_creation_succeeds_for_repo_without_remote() {
    // Create a temporary git repo with no remotes
    let tmp = tempfile::tempdir().unwrap();
    let repo_path = tmp.path();

    std::process::Command::new("git")
        .args(["init"])
        .current_dir(repo_path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    let origin_default = detect_origin_default_branch(repo_path).await;
    assert_eq!(origin_default, None);

    // Now create a worktree -- should succeed by branching from HEAD
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("test-worktree");
    let branch_name = "claude-code/test-branch";

    let mut wt_args = vec![
        "worktree",
        "add",
        wt_path.to_str().unwrap(),
        "-b",
        branch_name,
    ];
    if let Some(ref base_ref) = origin_default {
        wt_args.push(base_ref);
    }

    let result = git_cmd(&wt_args, repo_path).await;
    assert!(
        result.is_ok(),
        "git worktree add failed: {:?}",
        result.err()
    );
    let output = result.unwrap();
    assert!(
        output.status.success(),
        "git worktree add returned non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Cleanup worktree
    let _ = git_cmd(
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        repo_path,
    )
    .await;
}

#[tokio::test]
async fn worktree_add_works_without_git_crypt() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");

    let out = worktree_add(&repo, &wt_path, &["-b", "feature/test"])
        .await
        .expect("worktree_add returned Err");
    assert!(
        out.status.success(),
        "checkout step failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        wt_path.join("init.txt").exists(),
        "init.txt not checked out"
    );
    assert!(wt_path.join(".git").exists(), "worktree .git missing");
}

/// A worktree directory deleted without `git worktree remove` leaves a stale
/// entry in `$GIT_DIR/worktrees`. git then refuses to re-add at the same path
/// with "missing but already registered worktree". `worktree_add` must
/// self-heal by pruning the stale registration before adding — otherwise a
/// Claude Code spawn that reuses the deterministic `thread-<id>` path dies with
/// an Event stream error (the reported PR-1076 follow-up regression).
#[tokio::test]
async fn worktree_add_recovers_from_missing_but_registered_path() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("thread-stale");

    // First spawn: create the worktree.
    let out = worktree_add(&repo, &wt_path, &["-b", "claude-code/old"])
        .await
        .expect("first worktree_add returned Err");
    assert!(
        out.status.success(),
        "first add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Simulate the residue: the directory is wiped but `git worktree remove`
    // was never run, so git keeps the registration as "missing".
    tokio::fs::remove_dir_all(&wt_path).await.unwrap();
    assert!(
        !wt_path.exists(),
        "precondition: worktree dir should be gone"
    );

    // Second spawn reuses the same deterministic path with a fresh branch.
    let out = worktree_add(&repo, &wt_path, &["-b", "claude-code/new"])
        .await
        .expect("second worktree_add returned Err");
    assert!(
        out.status.success(),
        "re-add over stale registration failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        wt_path.join("init.txt").exists(),
        "init.txt not checked out on re-add"
    );
    assert!(
        wt_path.join(".git").exists(),
        "worktree .git missing on re-add"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn worktree_add_links_git_crypt_dir_when_present() {
    let (_tmp, repo) = make_test_repo().await;

    let parent_gc = repo.join(".git/git-crypt");
    tokio::fs::create_dir_all(&parent_gc).await.unwrap();
    tokio::fs::write(parent_gc.join("keys"), b"stub")
        .await
        .unwrap();

    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");

    let out = worktree_add(&repo, &wt_path, &["-b", "feature/test"])
        .await
        .expect("worktree_add returned Err");
    assert!(
        out.status.success(),
        "checkout step failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let per_wt_git = git_cmd(&["rev-parse", "--absolute-git-dir"], &wt_path)
        .await
        .unwrap();
    let per_wt_git = std::path::PathBuf::from(
        String::from_utf8_lossy(&per_wt_git.stdout)
            .trim()
            .to_string(),
    );
    let link = per_wt_git.join("git-crypt");
    let meta = tokio::fs::symlink_metadata(&link)
        .await
        .expect("git-crypt symlink missing in per-worktree git dir");
    assert!(
        meta.file_type().is_symlink(),
        "git-crypt entry is not a symlink"
    );

    // macOS resolves /var/folders/... → /private/var/folders/..., so
    // read_link's raw output won't compare equal to the source path.
    let resolved = std::fs::canonicalize(tokio::fs::read_link(&link).await.unwrap()).unwrap();
    let expected = std::fs::canonicalize(&parent_gc).unwrap();
    assert_eq!(
        resolved, expected,
        "symlink does not point at parent git-crypt"
    );
}

/// A branch with commit + revert has zero net diff but non-zero commits.
/// `branch_changed_files` must return empty (no actual changes),
/// even though `has_branch_commits` returns true (commits exist).
/// This mismatch caused the "Apply" button to appear for no-op changes.
#[tokio::test]
async fn commit_plus_revert_branch_has_no_changed_files() {
    let (_tmp, repo) = make_test_repo().await;

    // Create a feature branch, commit a file, then revert
    let o = git_cmd(&["checkout", "-b", "feature"], &repo)
        .await
        .unwrap();
    assert!(o.status.success(), "checkout -b feature failed");
    tokio::fs::write(repo.join("new.txt"), "content")
        .await
        .unwrap();
    let o = git_cmd(&["add", "."], &repo).await.unwrap();
    assert!(o.status.success(), "git add failed");
    let o = git_cmd(&["commit", "-m", "add file"], &repo).await.unwrap();
    assert!(o.status.success(), "git commit failed");
    let o = git_cmd(&["revert", "--no-edit", "HEAD"], &repo)
        .await
        .unwrap();
    assert!(o.status.success(), "git revert failed");

    // Branch has commits (commit + revert = 2 commits ahead of main)
    assert!(
        has_branch_commits(&repo, "feature").await,
        "Branch should have commits even after revert"
    );

    // But branch_changed_files must be empty (zero net diff)
    let files = branch_changed_files(&repo, "feature").await;
    assert!(
        files.is_empty(),
        "Branch with commit+revert should have no changed files, got: {:?}",
        files
    );
}

/// `branch_changed_files` must diff against the SAME base the Diff button uses
/// (`default_diff_base`), so the branch-work gate it feeds can never
/// disagree with the diff the button renders.
///
/// Regression for the `example-repo` migration report: a migration tool
/// rewrote the user's local default branch so `origin/<default>` (the PR
/// branch's true fork point) is no longer an ancestor of local `main`. Diffing
/// against local `main` reported 53 changed files; the Diff button (diffing
/// against `origin/main`) showed 0 — the button lit up on an empty diff. With
/// the base aligned, `branch_changed_files` returns empty in this scenario,
/// matching the button.
#[tokio::test]
async fn branch_changed_files_uses_origin_base_when_local_default_diverged() {
    let origin_tmp = tempfile::tempdir().unwrap();
    let origin = origin_tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "-q", "--bare", "-b", "main"], &origin).await;

    let (_tmp, repo) = make_test_repo().await;
    let c_root =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "HEAD"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();

    // A commit on main that the PR branch forks from, then publish so
    // `origin/main` holds that fork point.
    tokio::fs::write(repo.join("fork-point.txt"), "shipped before the branch\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(
        &["commit", "-m", "feature on main (branch forks here)"],
        &repo,
    )
    .await;
    let _ = git_cmd(
        &["remote", "add", "origin", origin.to_str().unwrap()],
        &repo,
    )
    .await;
    let _ = git_cmd(&["push", "-q", "origin", "main"], &repo).await;
    let _ = git_cmd(&["remote", "set-head", "origin", "main"], &repo).await;

    // The CC branch forks from the fork point and adds nothing of its own —
    // its commits are exactly what already lives on origin/main.
    let _ = git_cmd(&["branch", "feature", "main"], &repo).await;

    // A migration rewrites local `main`: reset to the root, commit a notice.
    // `origin/main` is no longer an ancestor of local `main` — they diverge.
    let _ = git_cmd(&["reset", "-q", "--hard", &c_root], &repo).await;
    tokio::fs::write(repo.join("MIGRATION.md"), "moved to new-org\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(
        &["commit", "-m", "migration: secrets transfer (automated)"],
        &repo,
    )
    .await;

    // Against local `main` the three-dot diff would surface `fork-point.txt`
    // (and miss `MIGRATION.md`); against `origin/main` (the branch's true fork
    // point) the branch has zero net diff.
    let files = branch_changed_files(&repo, "feature").await;
    assert!(
        files.is_empty(),
        "branch_changed_files must diff against origin/main when local main diverged, got: {:?}",
        files
    );
    assert_eq!(
        proposal_files_for_branch(&repo, "feature").await,
        None,
        "no net diff against the branch's true fork point must not warrant a Change proposal"
    );
}

/// Recovery must NOT propose a Change when the branch has commits but zero net
/// diff. Without this gate, `propose_branch_changes` creates a `changes` row
/// with `file_count=0`, which renders Apply/Discard buttons that do nothing
/// useful.
#[tokio::test]
async fn proposal_files_for_branch_rejects_commit_plus_revert() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo)
        .await
        .unwrap();
    tokio::fs::write(repo.join("new.txt"), "x").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "add file"], &repo).await.unwrap();
    let _ = git_cmd(&["revert", "--no-edit", "HEAD"], &repo)
        .await
        .unwrap();

    assert_eq!(
        proposal_files_for_branch(&repo, "feature").await,
        None,
        "branch with commit+revert (zero net diff) must not warrant a Change proposal"
    );
}

#[tokio::test]
async fn proposal_files_for_branch_returns_files_for_real_changes() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo)
        .await
        .unwrap();
    tokio::fs::write(repo.join("new.txt"), "x").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "add file"], &repo).await.unwrap();

    assert_eq!(
        proposal_files_for_branch(&repo, "feature").await,
        Some(vec!["new.txt".to_string()]),
        "branch with real changes must yield the changed file list"
    );
}

#[tokio::test]
async fn proposal_files_for_branch_rejects_no_commits() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["branch", "feature"], &repo).await.unwrap();

    assert_eq!(
        proposal_files_for_branch(&repo, "feature").await,
        None,
        "branch with no commits ahead of main must not warrant a Change proposal"
    );
}

/// Regression for real thread `bb9e68d6` ("Codex vs Claude Code for UI"): the
/// branch's work was applied (merged into main), then the engine back-merged
/// main *into* the branch at an earlier main tip (conflict recovery). That
/// criss-cross leaves the branch with only a *merge* commit ahead of main and
/// TWO merge bases, which regresses the three-dot `branch_changed_files` base to
/// the original fork point — re-surfacing the already-applied file as a phantom
/// diff. `has_branch_commits` (two-dot existence) is fooled by the merge commit,
/// so the startup recovery sweep re-proposed the already-applied change as a new
/// pending change (and the change state reconciled to unproposed work). A branch
/// whose only commits ahead of main are merge commits has no authored work left
/// to propose — `proposal_files_for_branch` must return `None`.
#[tokio::test]
async fn proposal_files_for_branch_rejects_already_applied_after_criss_cross_back_merge() {
    let (_tmp, repo) = make_test_repo().await;

    // The branch authors real work.
    let _ = git_cmd(&["checkout", "-b", "claude-code/feature"], &repo).await;
    tokio::fs::write(repo.join("work.rs"), "fn work() {}")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "branch authored work"], &repo).await;

    // main advances independently.
    let _ = git_cmd(&["checkout", "main"], &repo).await;
    tokio::fs::write(repo.join("other.rs"), "fn other() {}")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "main independent work"], &repo).await;
    let pre_apply_main =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "HEAD"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();

    // Apply (main side): main merges the branch, so `work.rs` is now on main.
    let _ = git_cmd(
        &[
            "merge",
            "--no-ff",
            "-m",
            "apply: merge feature into main",
            "claude-code/feature",
        ],
        &repo,
    )
    .await;

    // Back-merge main's PRE-apply tip into the branch — the criss-cross that
    // gives main and the branch two merge bases.
    let _ = git_cmd(&["checkout", "claude-code/feature"], &repo).await;
    let _ = git_cmd(
        &[
            "merge",
            "--no-ff",
            "-m",
            "Merge branch 'main' into claude-code/feature",
            &pre_apply_main,
        ],
        &repo,
    )
    .await;

    // Precondition: a merge commit ahead of main fools the loose existence check.
    assert!(
        has_branch_commits(&repo, "claude-code/feature").await,
        "precondition: the back-merge leaves a merge commit ahead of main"
    );
    // Precondition: the criss-cross regresses the three-dot base, so the
    // already-applied file re-surfaces in `branch_changed_files`.
    assert!(
        !branch_changed_files(&repo, "claude-code/feature")
            .await
            .is_empty(),
        "precondition: criss-cross regresses the 3-dot base, re-surfacing the applied file"
    );

    // But the branch has zero authored (non-merge) commits that aren't already
    // in main, so it must NOT warrant a fresh Change proposal.
    assert_eq!(
        proposal_files_for_branch(&repo, "claude-code/feature").await,
        None,
        "already-applied branch with only a back-merge commit must not be re-proposed"
    );
}

#[test]
fn files_have_client_update_detects_bundle_inputs() {
    for f in [
        "crates/lucidos-app/src/App.tsx",
        "crates/lucidos-app/src/styles/global/base.css",
        "crates/lucidos-app/public/sw.js",
        "crates/lucidos-app/vite/gatewaySession.ts",
        "crates/lucidos-app/index.html",
        "crates/lucidos-app/vite.config.ts",
        "crates/lucidos-app/package.json",
        "packages/lucidos-sdk/src/index.ts",
        "package.json",
        "package-lock.json",
    ] {
        assert!(
            files_have_client_update(&[f.into()]),
            "{f} feeds the bundle"
        );
    }
}

/// An Apply touching only an e2e spec must not wait for a rebuild. The
/// build-watch never runs one, so the wait ends in a false warning.
#[test]
fn files_have_client_update_ignores_files_outside_the_bundle() {
    for f in [
        "crates/lucidos-app/e2e/transcript-window-fills-the-pane.spec.ts",
        "crates/lucidos-app/e2e/fixture.html",
        "crates/lucidos-app/dev-build-watch.mjs",
        "crates/lucidos-engine/src/api/sdk_iframe_audio.js",
        "packages/lucidos-sdk/tests/preferences.test.ts",
        "scripts/lib/e2e.ts",
        "crates/lucidos-engine/src/main.rs",
        "Cargo.toml",
        "README.md",
    ] {
        assert!(
            !files_have_client_update(&[f.into()]),
            "{f} is not a bundle input"
        );
    }
    assert!(!files_have_client_update(&[]));
}

/// The classifier must agree with what the build-watch watches, or an Apply
/// either waits for a build that never runs or skips one that does.
#[test]
fn files_have_client_update_matches_the_build_watch_inputs() {
    let watcher = include_str!("../../../../lucidos-app/dev-build-watch.mjs");
    let mut watched: Vec<String> = watcher
        .lines()
        .filter_map(|l| l.trim().strip_prefix("resolve("))
        .filter_map(|l| Some((l.split(',').next()?, l.split('\'').nth(1)?)))
        .map(|(base, rel)| match (base, rel.strip_prefix("../../")) {
            ("PROJECT_DIR", _) => rel.to_string(),
            ("APP_DIR", Some(up)) => up.to_string(),
            ("APP_DIR", None) => format!("crates/lucidos-app/{rel}"),
            _ => panic!("unknown base {base} in dev-build-watch.mjs"),
        })
        .collect();
    let mut classified: Vec<String> = restart_detection::CLIENT_BUNDLE_DIRS
        .iter()
        .map(|dir| dir.trim_end_matches('/'))
        .chain(restart_detection::CLIENT_BUNDLE_FILES.iter().copied())
        .map(str::to_string)
        .collect();
    watched.sort();
    classified.sort();
    assert_eq!(
        watched, classified,
        "dev-build-watch.mjs and files_have_client_update disagree"
    );
}

/// External repos with non-`main`/`master` default branches must be detected
/// via `origin/HEAD`. Without this, Tier 0 cleanup would never fire on
/// external-repo worktrees (defensive `has_branch_commits` returns true on
/// `git rev-list main..branch` failure when `main` doesn't exist), and
/// applied worktrees would linger until Tier 2 (30d).
#[tokio::test]
async fn default_local_branch_reads_origin_head_for_non_main_repos() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();

    // Build a fake "remote" repo with `develop` as its default branch
    let remote_tmp = tempfile::tempdir().unwrap();
    let remote = remote_tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "--bare", "-b", "develop"], &remote)
        .await
        .unwrap();

    // Init local repo on `develop`, set up origin, commit, push so
    // `origin/develop` exists, then `set-head -a` to populate `origin/HEAD`
    let _ = git_cmd(&["init", "-b", "develop"], &repo).await.unwrap();
    let _ = git_cmd(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        &repo,
    )
    .await
    .unwrap();
    tokio::fs::write(repo.join("init.txt"), "x").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "initial"], &repo).await.unwrap();
    let _ = git_cmd(&["push", "-u", "origin", "develop"], &repo)
        .await
        .unwrap();
    let o = git_cmd(&["remote", "set-head", "origin", "-a"], &repo)
        .await
        .unwrap();
    assert!(
        o.status.success(),
        "remote set-head -a failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    assert_eq!(
        default_local_branch(&repo).await,
        "develop",
        "default_local_branch must follow origin/HEAD, not assume main/master"
    );
}

/// Repos without a configured `origin/HEAD` (test fixtures, fresh clones
/// before push, etc.) must still resolve to `main` via the heuristic
/// fallback. This locks in backwards compatibility with the previous
/// implementation.
#[tokio::test]
async fn default_local_branch_falls_back_to_main_without_origin() {
    let (_tmp, repo) = make_test_repo().await;
    assert_eq!(default_local_branch(&repo).await, "main");
}

/// The cache must actually return cached values within its TTL — proven by
/// renaming the underlying branch between calls and asserting the second
/// call returns the original (cached) name, not the live one. Without the
/// cache, the second call would re-resolve and return `"renamed-main"`.
#[tokio::test]
async fn default_local_branch_returns_cached_value_within_ttl() {
    let (_tmp, repo) = make_test_repo().await;
    assert_eq!(default_local_branch(&repo).await, "main");

    let o = git_cmd(&["branch", "-m", "main", "renamed-main"], &repo)
        .await
        .unwrap();
    assert!(
        o.status.success(),
        "branch rename failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    assert_eq!(
        default_local_branch(&repo).await,
        "main",
        "second call within TTL must return cached value, not re-resolve"
    );
}

/// The cache must key on `repo_root` so two different repos don't share
/// each other's cached values. Regression guard for a future
/// "simplification" that drops the path key.
#[tokio::test]
async fn default_local_branch_cache_separates_per_repo_root() {
    let (_tmp_main, repo_main) = make_test_repo().await;

    let tmp_master = tempfile::tempdir().unwrap();
    let repo_master = tmp_master.path().to_path_buf();
    let _ = git_cmd(&["init"], &repo_master).await.unwrap();
    let _ = git_cmd(&["checkout", "-b", "master"], &repo_master)
        .await
        .unwrap();
    tokio::fs::write(repo_master.join("init.txt"), "x")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_master).await.unwrap();
    let _ = git_cmd(&["commit", "-m", "initial"], &repo_master)
        .await
        .unwrap();

    assert_eq!(default_local_branch(&repo_main).await, "main");
    assert_eq!(default_local_branch(&repo_master).await, "master");
    assert_eq!(
        default_local_branch(&repo_main).await,
        "main",
        "main repo cache must not be polluted by master repo lookup"
    );
}

/// The reported external-repo bug: an external repo whose default branch is
/// neither `main` nor `master` (e.g. `develop`) and whose canonical branch was
/// never checked out locally — the coding-agent worktree branched straight off
/// `origin/develop`. `default_local_branch` can't find a *local* default and
/// falls through to the hardcoded `"main"` guess; since `origin/main` doesn't
/// exist either, `default_diff_base` returned bare `main` and the Diff button's
/// `main...<branch>` range died with `fatal: unknown revision 'main'`.
///
/// `default_diff_base` must fall back to `origin/<default>` (the ref the branch
/// was actually cut from — its true fork point), so the diff range resolves.
#[tokio::test]
async fn default_diff_base_falls_back_to_origin_when_local_default_branch_missing() {
    // A "remote" whose default branch is `develop`, seeded with one commit.
    let remote_tmp = tempfile::tempdir().unwrap();
    let remote = remote_tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "-q", "--bare", "-b", "develop"], &remote).await;

    let seed_tmp = tempfile::tempdir().unwrap();
    let seed = seed_tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "-q", "-b", "develop"], &seed).await;
    tokio::fs::write(seed.join("base.txt"), "base\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &seed).await;
    let _ = git_cmd(&["commit", "-q", "-m", "base"], &seed).await;
    let _ = git_cmd(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        &seed,
    )
    .await;
    let _ = git_cmd(&["push", "-q", "origin", "develop"], &seed).await;

    // The repo-under-test: has `origin` + `origin/HEAD` -> origin/develop, but
    // NO local `develop` branch (never checked out).
    let repo_tmp = tempfile::tempdir().unwrap();
    let repo = repo_tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "-q"], &repo).await;
    let _ = git_cmd(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        &repo,
    )
    .await;
    let _ = git_cmd(&["fetch", "-q", "origin"], &repo).await;
    let o = git_cmd(&["remote", "set-head", "origin", "-a"], &repo)
        .await
        .unwrap();
    assert!(
        o.status.success(),
        "remote set-head -a failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    // The CC worktree branch forks straight off origin/develop (mirrors
    // `resolve_worktree_base` for an external repo) and adds a commit — WITHOUT
    // ever creating a local `develop` branch.
    let branch = "claude-code/20260701-083109-27b2c5";
    let _ = git_cmd(&["checkout", "-q", "-b", branch, "origin/develop"], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-q", "-m", "cc work"], &repo).await;

    // Precondition: there is genuinely no local `develop`/`main`/`master` to
    // diff against — the hardcoded `main` fallback is a phantom ref here.
    for phantom in ["develop", "main", "master"] {
        assert!(
            !git_cmd(&["rev-parse", "--verify", "--quiet", phantom], &repo)
                .await
                .unwrap()
                .status
                .success(),
            "precondition: local `{phantom}` must not exist"
        );
    }

    let base = default_diff_base(&repo).await;
    assert_eq!(
        base, "origin/develop",
        "must fall back to origin/<default> when the local default branch is absent, got `{base}`"
    );

    // The user-visible symptom: the three-dot diff range must resolve (this is
    // the exact command the Diff button runs).
    let range = format!("{base}...{branch}");
    let diff = git_cmd(&["diff", &range, "--no-color"], &repo)
        .await
        .unwrap();
    assert!(
        diff.status.success(),
        "diff range `{range}` must resolve, got: {}",
        String::from_utf8_lossy(&diff.stderr)
    );
    assert!(
        String::from_utf8_lossy(&diff.stdout).contains("feature.txt"),
        "diff must show the branch's authored file"
    );
}

/// Belt-and-suspenders: a repo with NO `origin` remote whose default branch is
/// neither `main` nor `master` (e.g. `trunk`). `default_local_branch` still
/// hands back the `"main"` guess, and there's no `origin/<default>` to fall back
/// to — so `default_diff_base` must degrade to the primary worktree's tip commit
/// (a ref that always resolves) rather than erroring the diff with a phantom
/// `main`.
#[tokio::test]
async fn default_diff_base_falls_back_to_primary_worktree_head_when_no_default_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "-q", "-b", "trunk"], &repo).await;
    tokio::fs::write(repo.join("base.txt"), "base\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-q", "-m", "base"], &repo).await;

    // CC branch off HEAD (mirrors an external repo with no `origin`), add work,
    // then return the primary checkout to `trunk` so the diff base != branch tip.
    let branch = "claude-code/20260701-090000-abcdef";
    let _ = git_cmd(&["checkout", "-q", "-b", branch], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-q", "-m", "cc work"], &repo).await;
    let _ = git_cmd(&["checkout", "-q", "trunk"], &repo).await;

    let base = default_diff_base(&repo).await;
    assert!(
        git_cmd(&["rev-parse", "--verify", "--quiet", &base], &repo)
            .await
            .unwrap()
            .status
            .success(),
        "default_diff_base must return a ref that resolves, got `{base}`"
    );

    let range = format!("{base}...{branch}");
    let diff = git_cmd(&["diff", &range, "--no-color"], &repo)
        .await
        .unwrap();
    assert!(
        diff.status.success(),
        "diff range `{range}` must resolve, got: {}",
        String::from_utf8_lossy(&diff.stderr)
    );
    assert!(
        String::from_utf8_lossy(&diff.stdout).contains("feature.txt"),
        "diff must show the branch's authored file"
    );
}

/// Regression guard for the empty-diff trap: when `default_diff_base` runs
/// inside a LINKED coding-agent worktree (as `diff_via_worktree` calls it) for a
/// no-origin, non-main/master-default repo, the last-resort base must NOT be the
/// worktree's own `HEAD` — that is the thread's branch, so `HEAD...HEAD` renders
/// an empty diff even though the branch has real changes. It must resolve to the
/// PRIMARY worktree's tip so the branch's work shows up.
#[tokio::test]
async fn default_diff_base_in_linked_worktree_never_uses_own_head() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init", "-q", "-b", "trunk"], &repo).await;
    tokio::fs::write(repo.join("base.txt"), "base\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-q", "-m", "base"], &repo).await;

    // Linked worktree on a CC branch off trunk (mirrors an external-repo spawn
    // with no origin), with an authored commit.
    let branch = "claude-code/20260701-091500-fedcba";
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    let out = git_cmd(
        &[
            "worktree",
            "add",
            "-b",
            branch,
            wt_path.to_str().unwrap(),
            "trunk",
        ],
        &repo,
    )
    .await
    .unwrap();
    assert!(
        out.status.success(),
        "worktree add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    tokio::fs::write(wt_path.join("feature.txt"), "feature\n")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt_path).await;
    let _ = git_cmd(&["commit", "-q", "-m", "cc work"], &wt_path).await;

    // Resolve the base FROM the worktree, exactly as diff_via_worktree does.
    let base = default_diff_base(&wt_path).await;
    assert_ne!(
        base, "HEAD",
        "base must not be the worktree's own HEAD (would diff the branch against itself)"
    );

    let range = format!("{base}...HEAD");
    let diff = git_cmd(&["diff", &range, "--no-color"], &wt_path)
        .await
        .unwrap();
    assert!(
        diff.status.success(),
        "diff range `{range}` must resolve, got: {}",
        String::from_utf8_lossy(&diff.stderr)
    );
    assert!(
        String::from_utf8_lossy(&diff.stdout).contains("feature.txt"),
        "the linked worktree's authored file must appear in the diff — an empty \
         diff means the base collapsed onto the branch's own HEAD"
    );
}

#[test]
fn files_have_client_update_mixed_files() {
    assert!(files_have_client_update(&[
        "crates/lucidos-engine/src/main.rs".into(),
        "crates/lucidos-app/src/App.tsx".into()
    ]));
}

#[test]
fn files_require_restart_detects_rust_and_migrations() {
    assert!(files_require_restart(&[
        "crates/lucidos-engine/src/main.rs".into()
    ]));
    assert!(files_require_restart(&["Cargo.toml".into()]));
    assert!(files_require_restart(&["Cargo.lock".into()]));
    assert!(files_require_restart(&[
        "crates/lucidos-engine/migrations/001.sql".into()
    ]));
}

#[test]
fn files_require_restart_ignores_tests_and_docs() {
    assert!(!files_require_restart(&[
        "crates/lucidos-e2e/tests/api.rs".into()
    ]));
    assert!(!files_require_restart(&["README.md".into()]));
    assert!(!files_require_restart(&[
        "crates/lucidos-app/src/App.tsx".into()
    ]));
    assert!(!files_require_restart(&[
        "crates/lucidos-app/src/global.css".into()
    ]));
    // SDK docs and SDK tests don't affect the bundle
    assert!(!files_require_restart(&[
        "packages/lucidos-sdk/README.md".into()
    ]));
    assert!(!files_require_restart(&[
        "packages/lucidos-sdk/tests/preferences.test.ts".into()
    ]));
    assert!(!files_require_restart(&[]));
}

/// Bundle-served paths require restart: edits don't take effect until the
/// engine restarts because `web-dev.sh -b` rebuilds the SDK bundle and the
/// engine re-loads compiled-in static assets.
#[test]
fn files_require_restart_for_sdk_bundle_sources() {
    // packages/lucidos-sdk/src — TS bundled into /api/v1/sdk.js
    assert!(files_require_restart(&[
        "packages/lucidos-sdk/src/preferences.ts".into()
    ]));
    assert!(files_require_restart(&[
        "packages/lucidos-sdk/src/index.ts".into()
    ]));
    // packages/lucidos-sdk root — build script + tsconfig + package.json
    assert!(files_require_restart(&[
        "packages/lucidos-sdk/build.mjs".into()
    ]));
    assert!(files_require_restart(&[
        "packages/lucidos-sdk/tsconfig.json".into()
    ]));
}

/// Engine-bundled iframe assets are include_str!'d into the binary,
/// so a Cargo rebuild + restart is required for the served bytes to refresh.
#[test]
fn files_require_restart_for_engine_bundled_iframe_assets() {
    assert!(files_require_restart(&[
        "crates/lucidos-engine/src/api/sdk_iframe.css".into()
    ]));
    assert!(files_require_restart(&[
        "crates/lucidos-engine/src/api/sdk_iframe_audio.js".into()
    ]));
}

/// The vendored fonts are the sharpest case of "an engine-bundled asset is not
/// always an engine FILE". They live in the app crate, because the host's
/// `@font-face` resolves them through Vite, so by path alone they read as a
/// frontend-only change. But `core::fonts` `include_bytes!`s the same files to
/// serve app iframes, so a running engine serves the copy it was BUILT with.
#[test]
fn files_require_restart_for_the_engine_bundled_fonts() {
    assert!(files_require_restart(&[
        "crates/lucidos-app/src/assets/fonts/FiraCode-VF.woff2".into()
    ]));
    assert!(files_require_restart(&[
        "crates/lucidos-app/src/assets/fonts/Geist-latin.woff2".into()
    ]));
    // The license text beside the font is not served by anything.
    assert!(!files_require_restart(&[
        "crates/lucidos-app/src/assets/fonts/LICENSE-FiraCode.txt".into()
    ]));
}

/// Every file a binary embeds needs a restart. Otherwise an Apply that edits
/// one offers no restart, and the running process keeps the old copy.
#[test]
fn a_file_a_binary_embeds_requires_a_restart() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    // Built into none of the binaries an Apply rebuilds and restarts onto.
    const NOT_IN_A_RESTARTED_BINARY: &[&str] = &["lucidos-app", "lucidos-e2e", "lucidos-eval"];
    let include = regex::Regex::new(r#"include_(?:str|bytes)!\(\s*"([^"]+)"\s*\)"#).unwrap();
    let mut embedded = Vec::new();
    for crate_dir in std::fs::read_dir(root.join("crates")).unwrap() {
        let crate_dir = crate_dir.unwrap().path();
        let name = crate_dir.file_name().unwrap().to_string_lossy().to_string();
        if NOT_IN_A_RESTARTED_BINARY.contains(&name.as_str()) {
            continue;
        }
        let mut dirs = vec![crate_dir.join("src")];
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                let file_name = path.file_name().unwrap().to_string_lossy().to_string();
                // A whole file or directory of tests, mounted under `#[cfg(test)]`.
                if file_name.ends_with("tests") || file_name.ends_with("tests.rs") {
                    continue;
                }
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if !file_name.ends_with(".rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                for literal in include.captures_iter(&production_source(&source)) {
                    let target = path.parent().unwrap().join(&literal[1]);
                    let target = target
                        .canonicalize()
                        .unwrap_or_else(|e| panic!("{} embeds {target:?}: {e}", path.display()));
                    let relative = target.strip_prefix(&root).unwrap();
                    embedded.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    assert!(
        embedded.len() > 20,
        "the scan found only {embedded:?}, so it no longer reads the sources"
    );
    let missed: Vec<&String> = embedded
        .iter()
        .filter(|f| !files_require_restart(&[f.to_string()]))
        .collect();
    assert!(
        missed.is_empty(),
        "a binary embeds these, but an Apply that edits one offers no restart: {missed:?}. \
         Add each to EMBEDDED_FILES in restart_detection.rs."
    );
}

/// A Rust file's source without its inline test modules. Each one is a
/// column-0 `#[cfg(test)]` over `mod <name> {`, closed by the next column-0
/// `}`, which rustfmt guarantees. A bodiless `mod x;` is only a declaration.
fn production_source(source: &str) -> String {
    let mut production = String::new();
    let mut rest = source;
    while let Some(at) = rest.find("\n#[cfg(test)]\n") {
        let (before, from) = rest.split_at(at);
        let item = from
            .lines()
            .skip(2)
            .find(|line| !line.starts_with("#["))
            .unwrap_or("");
        let names_a_module = item.starts_with("mod ") || item.starts_with("pub mod ");
        let body_end = from.find("\n}\n").map(|end| end + "\n}\n".len());
        match body_end {
            Some(end) if names_a_module && item.trim_end().ends_with('{') => {
                production.push_str(before);
                rest = from.split_at(end).1;
            }
            _ => {
                let (kept, after) = from.split_at("\n#[cfg(test)]\n".len());
                production.push_str(before);
                production.push_str(kept);
                rest = after;
            }
        }
    }
    production.push_str(rest);
    production
}

#[test]
fn production_source_drops_test_bodies_and_keeps_what_follows() {
    let source = "#[cfg(test)]\nmod a;\n\n#[cfg(test)]\nmod b;\nconst X: &str = \"x\";\n\
                  \n#[cfg(test)]\n#[allow(dead_code)]\nmod tests {\n    fn t() {}\n}\n\
                  const Y: &str = \"y\";\n\n#[cfg(test)]\nmod more {\n    fn u() {}\n}\n";
    let production = production_source(source);
    assert!(production.contains("mod b;"));
    assert!(production.contains("const X"));
    assert!(production.contains("const Y"));
    assert!(!production.contains("fn t()"));
    assert!(!production.contains("fn u()"));
}

/// The two shapes the embed guard exists for: a built-in theme is an engine data
/// file, and the app-iframe stylesheet lives in the frontend tree.
#[test]
fn files_require_restart_for_embedded_data_and_the_iframe_stylesheet() {
    assert!(files_require_restart(&[
        "crates/lucidos-engine/src/core/themes/builtin/nord.json".into()
    ]));
    assert!(files_require_restart(&[
        "crates/lucidos-app/src/styles/global/shared-components.css".into()
    ]));
    assert!(files_require_restart(&[
        "crates/lucidos-gateway/Cargo.toml".into()
    ]));
    assert!(!files_require_restart(&[
        "crates/lucidos-engine/src/core/themes/README.md".into()
    ]));
}

/// The changelog is the exception to "docs never restart". It is
/// `include_str!`'d by `engine::changelog` and served to the What's New panel,
/// so a running engine serves the copy it was BUILT with. Without the restart an
/// Apply that adds a release would leave the panel on the previous text, with
/// the button having promised nothing was needed. Other `.md` files are
/// unaffected, which is exactly why this one has to be named.
#[test]
fn files_require_restart_for_the_engine_bundled_changelog() {
    assert!(files_require_restart(&["CHANGELOG.md".into()]));
    assert!(!files_require_restart(&["docs/glossary.md".into()]));
}

/// The release notices are the changelog's sibling, `include_str!`'d by
/// `engine::release_notices`. Missing the restart costs more here: the changelog
/// merely reads stale, while an authored notice that never reaches a modal is an
/// instruction the user is never given at all.
#[test]
fn files_require_restart_for_the_engine_bundled_release_notices() {
    assert!(files_require_restart(&["release-notices.toml".into()]));
    // Other TOML is not bundled into the binary.
    assert!(!files_require_restart(&["rust-toolchain.toml".into()]));
}

/// The app document is the exception to "frontend files never restart": the
/// gateway `include_str!`s it and lifts the boot-splash stylesheet + mark out at
/// compile time (crates/lucidos-gateway/src/proxy.rs), so a running gateway
/// serves the splash it was BUILT with. Without the rebuild its splash and the
/// app's would drift apart, which is the whole thing sharing the file prevents.
#[test]
fn files_require_restart_for_the_gateway_bundled_app_document() {
    assert!(files_require_restart(&[
        "crates/lucidos-app/index.html".into()
    ]));
    // Other frontend HTML is still frontend-only.
    assert!(!files_require_restart(&[
        "crates/lucidos-app/e2e/fixture.html".into()
    ]));
}

/// Creating an isolation branch (`-b`) must NOT write upstream-tracking config.
/// That write — triggered by `branch.autoSetupMerge` — is the ONLY thing
/// `git worktree add` puts in the SHARED `.git/config`, and under several
/// near-simultaneous coding-agent spawns it raced on `.git/config.lock`, failing
/// the spawn with "could not lock config file .git/config / unable to write
/// upstream branch configuration". `worktree_add` now passes `--no-track`, so no
/// tracking config is written and there is nothing to collide on.
#[tokio::test]
async fn worktree_add_creates_branch_without_upstream_tracking() {
    let (_tmp, repo) = make_test_repo().await;
    // Reproduce the workspace config that made branch creation write tracking.
    let o = git_cmd(&["config", "branch.autoSetupMerge", "always"], &repo)
        .await
        .unwrap();
    assert!(o.status.success(), "set autoSetupMerge failed");

    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    let out = worktree_add(&repo, &wt_path, &["-b", "claude-code/no-track", "main"])
        .await
        .expect("worktree_add returned Err");
    assert!(
        out.status.success(),
        "worktree_add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cfg = git_cmd(
        &["config", "--get-regexp", r"^branch\.claude-code/no-track\."],
        &repo,
    )
    .await
    .unwrap();
    // `--get-regexp` exits non-zero with empty stdout when nothing matches.
    assert!(
        !cfg.status.success() && String::from_utf8_lossy(&cfg.stdout).trim().is_empty(),
        "branch must have no upstream tracking config, got: {}",
        String::from_utf8_lossy(&cfg.stdout)
    );
}

/// The reported failure, reproduced directly: a concurrent git process holds the
/// shared `.git/config.lock` while a coding-agent spawn creates its worktree.
/// Because `worktree_add` no longer writes the shared config (see `--no-track`
/// above), the add must succeed even while the lock is held — the prior code
/// died here with "could not lock config file .git/config: File exists".
#[tokio::test]
async fn worktree_add_succeeds_while_config_lock_is_held() {
    let (_tmp, repo) = make_test_repo().await;
    let o = git_cmd(&["config", "branch.autoSetupMerge", "always"], &repo)
        .await
        .unwrap();
    assert!(o.status.success(), "set autoSetupMerge failed");

    // Stand in for another git process mid-config-write.
    let lock = repo.join(".git/config.lock");
    tokio::fs::write(&lock, b"").await.unwrap();

    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    let result = worktree_add(&repo, &wt_path, &["-b", "claude-code/locked", "main"]).await;

    // Release the lock regardless of outcome so nothing is left behind.
    let _ = tokio::fs::remove_file(&lock).await;

    let out = result.expect("worktree_add returned Err");
    assert!(
        out.status.success(),
        "worktree_add must not need the shared config lock, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        wt_path.join("init.txt").exists(),
        "worktree not checked out"
    );
}

/// The third concurrent-spawn hazard: `worktree_add` prunes before it adds, and
/// a prune deletes any `worktrees/<id>` holding no `gitdir` file yet. That is
/// what a sibling's add looks like for a few syscalls, so no add may run while
/// another spawn holds the admin lock. Held here the way `prune_worktrees`
/// holds it.
///
/// Structural, because the real window is a few syscalls wide and only opens
/// under load. It fails when the lock leaves `worktree_add_pruning_stale`, and
/// cannot tell the prune's half of that pair from the add's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worktree_add_waits_for_a_prune_holding_the_admin_lock() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");

    let guard = super::worktree::WORKTREE_ADMIN_MUTEX.lock().await;
    let add = tokio::spawn(async move {
        worktree_add(&repo, &wt_path, &["-b", "claude-code/serialised", "main"]).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let ran_anyway = add.is_finished();

    drop(guard);
    let out = add
        .await
        .expect("add task panicked")
        .expect("worktree_add returned Err");

    assert!(
        !ran_anyway,
        "worktree add ran while the admin lock was held, so a concurrent prune can still delete its half-built admin dir"
    );
    assert!(
        out.status.success(),
        "worktree_add failed once the lock was free: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `for-each-ref` output in the shape [`RefFacts::read`] asks for.
fn ref_facts(refs: &[(&str, &str, &str)]) -> RefFacts {
    let stdout: String = refs
        .iter()
        .map(|(name, sha, symref)| format!("{name}\0{sha}\0{symref}\n"))
        .collect();
    RefFacts::parse(&stdout)
}

#[test]
fn diff_base_skips_the_ancestry_check_when_local_and_origin_agree() {
    let facts = ref_facts(&[
        ("refs/heads/main", "aaa", ""),
        (
            "refs/remotes/origin/HEAD",
            "aaa",
            "refs/remotes/origin/main",
        ),
        ("refs/remotes/origin/main", "aaa", ""),
    ]);
    assert_eq!(diff_base_from(&facts), DiffBase::Settled("main".into()));
}

#[test]
fn diff_base_asks_about_ancestry_when_local_and_origin_differ() {
    let facts = ref_facts(&[
        ("refs/heads/main", "aaa", ""),
        ("refs/remotes/origin/main", "bbb", ""),
    ]);
    assert_eq!(
        diff_base_from(&facts),
        DiffBase::LocalUnlessDiverged {
            local: "main".into(),
            remote_tracking: "origin/main".into(),
        }
    );
}

#[test]
fn diff_base_follows_origin_head_to_a_local_default() {
    let facts = ref_facts(&[
        ("refs/heads/develop", "aaa", ""),
        ("refs/heads/main", "ccc", ""),
        (
            "refs/remotes/origin/HEAD",
            "aaa",
            "refs/remotes/origin/develop",
        ),
    ]);
    assert_eq!(diff_base_from(&facts), DiffBase::Settled("develop".into()));
}

#[test]
fn diff_base_uses_master_when_there_is_no_main() {
    let facts = ref_facts(&[("refs/heads/master", "aaa", "")]);
    assert_eq!(diff_base_from(&facts), DiffBase::Settled("master".into()));
}

#[test]
fn diff_base_falls_back_to_origin_head_when_no_local_default_exists() {
    let facts = ref_facts(&[
        ("refs/heads/lucidos-thread-branch", "ddd", ""),
        (
            "refs/remotes/origin/HEAD",
            "aaa",
            "refs/remotes/origin/develop",
        ),
        ("refs/remotes/origin/develop", "aaa", ""),
    ]);
    assert_eq!(
        diff_base_from(&facts),
        DiffBase::Settled("origin/develop".into())
    );
}

#[test]
fn diff_base_uses_origin_main_when_local_main_is_missing() {
    let facts = ref_facts(&[("refs/remotes/origin/main", "aaa", "")]);
    assert_eq!(
        diff_base_from(&facts),
        DiffBase::Settled("origin/main".into())
    );
}

/// A `for-each-ref` that could not run reads as no refs at all, which lands
/// on the primary worktree's tip rather than a phantom branch name.
#[test]
fn diff_base_with_no_ref_facts_asks_the_primary_worktree() {
    assert_eq!(
        diff_base_from(&RefFacts::default()),
        DiffBase::PrimaryWorktreeHead
    );
}

#[tokio::test]
async fn ref_facts_read_parses_real_for_each_ref_output() {
    let (_tmp, repo) = make_test_repo().await;
    let facts = RefFacts::read(&repo).await;
    assert_eq!(diff_base_from(&facts), DiffBase::Settled("main".into()));
}

/// An `origin/HEAD` naming a branch the remote deleted names no default. A
/// local branch left behind by that name must not become the merge target.
#[tokio::test]
async fn a_dangling_origin_head_names_no_default() {
    let (_tmp, repo) = make_test_repo().await;
    for args in [
        &["branch", "develop"][..],
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/develop",
        ],
    ] {
        let o = git_cmd(args, &repo).await.unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    assert_eq!(default_local_branch(&repo).await, "main");
    assert_eq!(default_diff_base(&repo).await, "main");
}
