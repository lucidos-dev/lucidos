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
/// (`default_diff_base`), so the `coding_agent_has_diff` gate it feeds can never
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
/// pending change (and `coding_agent_has_diff` reconciled to TRUE). A branch
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
fn files_have_client_update_detects_frontend_files() {
    assert!(files_have_client_update(&["src/App.tsx".into()]));
    assert!(files_have_client_update(&["store/store.ts".into()]));
    assert!(files_have_client_update(&["styles/global.css".into()]));
    assert!(files_have_client_update(&["index.html".into()]));
    assert!(files_have_client_update(&["utils/helper.js".into()]));
    assert!(files_have_client_update(&["component.jsx".into()]));
}

#[test]
fn files_have_client_update_ignores_non_frontend_files() {
    assert!(!files_have_client_update(&["src/engine.rs".into()]));
    assert!(!files_have_client_update(&["Cargo.toml".into()]));
    assert!(!files_have_client_update(&["README.md".into()]));
    assert!(!files_have_client_update(&["migrations/001.sql".into()]));
    assert!(!files_have_client_update(&[]));
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
    // If any frontend file is present, returns true
    assert!(files_have_client_update(&[
        "src/engine.rs".into(),
        "src/App.tsx".into()
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

/// The vendored default font is the sharpest case of "an engine-bundled asset
/// is not always an engine FILE". `FiraCode-VF.woff2` lives in the app crate,
/// because the host's `@font-face` resolves it through Vite, so by path alone it
/// reads as a frontend-only change. But `api/sdk_fonts.rs` `include_bytes!`s that
/// same file to serve app iframes, so a running engine serves the copy it was
/// BUILT with. One copy in the tree, two consumers, and only one of them picks up
/// an edit without a rebuild.
#[test]
fn files_require_restart_for_the_engine_bundled_font() {
    assert!(files_require_restart(&[
        "crates/lucidos-app/src/assets/fonts/FiraCode-VF.woff2".into()
    ]));
    assert!(files_require_restart(&[
        "crates/lucidos-engine/src/api/sdk_fonts_fira_code.css".into()
    ]));
    // The license text beside the font is not served by anything.
    assert!(!files_require_restart(&[
        "crates/lucidos-app/src/assets/fonts/LICENSE-FiraCode.txt".into()
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
