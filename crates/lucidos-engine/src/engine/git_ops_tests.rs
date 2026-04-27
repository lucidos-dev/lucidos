use super::*;

/// Helper: create a temp git repo with an initial commit on main.
/// Returns (tempdir_guard, repo_path).
async fn make_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init"], &repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
    tokio::fs::write(repo.join("init.txt"), "initial")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "initial commit"], &repo).await;
    (tmp, repo)
}

#[test]
fn merge_direction_filter_rejects_main_into_branch() {
    let branch = "claude-code/20260325-115419-8fded7";

    // "Merge branch 'main' into feature" -- wrong direction, should be rejected
    assert!(!is_merge_of_branch_into_main(
        "07032b95 Merge branch 'main' into claude-code/20260325-115419-8fded7",
        branch,
    ));
}

#[test]
fn merge_direction_filter_accepts_branch_into_main() {
    let branch = "claude-code/20260325-115419-8fded7";

    // "Merge branch 'feature'" -- correct direction (into current/main)
    assert!(is_merge_of_branch_into_main(
        "a1b2c3d4 Merge branch 'claude-code/20260325-115419-8fded7'",
        branch,
    ));

    // "Merge feature: description" -- custom merge message
    assert!(is_merge_of_branch_into_main(
        "a1b2c3d4 Merge claude-code/20260325-115419-8fded7: fix slider",
        branch,
    ));
}

#[test]
fn merge_direction_filter_rejects_unrelated_branches() {
    let branch = "claude-code/20260325-115419-8fded7";

    // Completely unrelated branch
    assert!(!is_merge_of_branch_into_main(
        "a1b2c3d4 Merge branch 'claude-code/20260325-999999-aaaaaa'",
        branch,
    ));
}

/// Finds an out-of-band `--no-ff` merge commit and returns
/// `(pre = parent1 = old main, post = merge commit)`.
/// This is the apply_change idempotency fast-path.
#[tokio::test]
async fn find_branch_merge_in_main_detects_no_ff_merge() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "claude-code/feature-x"], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    let pre_main_sha =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();

    let _ = git_cmd(
        &[
            "merge",
            "--no-ff",
            "claude-code/feature-x",
            "-m",
            "Merge branch 'claude-code/feature-x'",
        ],
        &repo,
    )
    .await;
    let post_main_sha =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();

    let result = find_branch_merge_in_main(&repo, "claude-code/feature-x").await;
    let (pre, post) = result.expect("should find the merge commit");
    assert_eq!(pre, pre_main_sha, "pre should be old main (parent 1)");
    assert_eq!(post, post_main_sha, "post should be the merge commit");
}

/// Returns None when the branch has never been merged into main.
#[tokio::test]
async fn find_branch_merge_in_main_returns_none_when_not_merged() {
    let (_tmp, repo) = make_test_repo().await;

    let result = find_branch_merge_in_main(&repo, "claude-code/never-merged").await;
    assert!(
        result.is_none(),
        "should not find a merge for a branch that never existed"
    );
}

/// Reverse-direction merges (main into branch) must NOT be reported.
#[tokio::test]
async fn find_branch_merge_in_main_ignores_catchup_merges() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "claude-code/feature-y"], &repo).await;
    tokio::fs::write(repo.join("y.txt"), "y").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add y"], &repo).await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    tokio::fs::write(repo.join("main.txt"), "main")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "main work"], &repo).await;

    // Catchup: merge main INTO the branch (creates "Merge branch 'main' into claude-code/feature-y")
    let _ = git_cmd(&["checkout", "claude-code/feature-y"], &repo).await;
    let _ = git_cmd(&["merge", "main", "--no-edit"], &repo).await;

    // Branch was never merged INTO main, only the reverse direction
    let _ = git_cmd(&["checkout", "main"], &repo).await;
    let result = find_branch_merge_in_main(&repo, "claude-code/feature-y").await;
    assert!(
        result.is_none(),
        "should not match a catchup merge in the wrong direction"
    );
}

/// Works even when the branch ref has been deleted after the merge.
#[tokio::test]
async fn find_branch_merge_in_main_works_after_branch_deleted() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "claude-code/feature-z"], &repo).await;
    tokio::fs::write(repo.join("z.txt"), "z").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add z"], &repo).await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    let _ = git_cmd(
        &[
            "merge",
            "--no-ff",
            "claude-code/feature-z",
            "-m",
            "Merge branch 'claude-code/feature-z'",
        ],
        &repo,
    )
    .await;

    // Delete the branch ref — simulates the out-of-band cleanup that left the
    // change record stuck as "pending" with no live branch to verify against.
    let _ = git_cmd(&["branch", "-D", "claude-code/feature-z"], &repo).await;

    let result = find_branch_merge_in_main(&repo, "claude-code/feature-z").await;
    assert!(
        result.is_some(),
        "should still find the merge after branch deletion"
    );
}

/// Validates that MERGE_MUTEX serializes concurrent merge attempts.
/// Two tasks that both acquire the mutex should execute sequentially,
/// not concurrently -- the second task must wait for the first.
#[tokio::test]
async fn merge_mutex_serializes_concurrent_access() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    let counter = Arc::new(AtomicU32::new(0));
    let max_concurrent = Arc::new(AtomicU32::new(0));

    let mut handles = Vec::new();
    for _ in 0..5 {
        let counter = counter.clone();
        let max_concurrent = max_concurrent.clone();
        handles.push(tokio::spawn(async move {
            let _guard = MERGE_MUTEX.lock().await;
            let active = counter.fetch_add(1, Ordering::SeqCst) + 1;
            // Track maximum concurrent holders
            max_concurrent.fetch_max(active, Ordering::SeqCst);
            // Simulate merge work
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            counter.fetch_sub(1, Ordering::SeqCst);
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // At most 1 task should hold the mutex at any time
    assert_eq!(
        max_concurrent.load(Ordering::SeqCst),
        1,
        "MERGE_MUTEX must serialize access -- max concurrent holders should be 1"
    );
}

#[tokio::test]
async fn commits_in_range_returns_subjects_in_chronological_order() {
    let (_tmp, repo) = make_test_repo().await;
    let pre_sha =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("a.txt"), "a").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "feat: add a"], &repo).await;
    tokio::fs::write(repo.join("b.txt"), "b").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "feat: add b"], &repo).await;

    let post_sha = String::from_utf8_lossy(
        &git_cmd(&["rev-parse", "feature"], &repo)
            .await
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();

    let commits = commits_in_range(&repo, &pre_sha, &post_sha).await;
    assert_eq!(
        commits,
        vec!["feat: add a".to_string(), "feat: add b".to_string()]
    );
}

#[tokio::test]
async fn commits_in_range_filters_auto_commit_subjects() {
    let (_tmp, repo) = make_test_repo().await;
    let pre_sha =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("a.txt"), "a").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(
        &["commit", "-m", "Claude Code changes (auto-committed)"],
        &repo,
    )
    .await;
    tokio::fs::write(repo.join("b.txt"), "b").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "fix: real change"], &repo).await;

    let post_sha = String::from_utf8_lossy(
        &git_cmd(&["rev-parse", "feature"], &repo)
            .await
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();

    let commits = commits_in_range(&repo, &pre_sha, &post_sha).await;
    assert_eq!(commits, vec!["fix: real change".to_string()]);
}

#[tokio::test]
async fn commits_in_range_empty_for_identical_shas() {
    let (_tmp, repo) = make_test_repo().await;
    let sha =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();

    let commits = commits_in_range(&repo, &sha, &sha).await;
    assert!(commits.is_empty());
}

#[tokio::test]
async fn ff_main_to_leaves_clean_working_tree() {
    let (_tmp, repo) = make_test_repo().await;

    // Create a feature branch with a new file
    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "new feature")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

    let main_sha =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();
    let feature_sha = String::from_utf8_lossy(
        &git_cmd(&["rev-parse", "feature"], &repo)
            .await
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();

    // Switch back to main before ff (simulates normal repo state)
    let _ = git_cmd(&["checkout", "main"], &repo).await;

    let result = ff_main_to(&repo, &feature_sha, &main_sha).await;
    assert!(
        result.is_ok(),
        "ff_main_to should succeed: {:?}",
        result.err()
    );

    // Main ref should now point to feature
    let new_main =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();
    assert_eq!(new_main, feature_sha);

    // Working tree must be clean -- no staged or unstaged changes
    let status = git_cmd(&["status", "--porcelain"], &repo).await.unwrap();
    let status_output = String::from_utf8_lossy(&status.stdout).to_string();
    assert!(
        status_output.trim().is_empty(),
        "Working tree should be clean after ff_main_to, got: {}",
        status_output
    );

    // The feature file should exist in the working tree
    assert!(
        repo.join("feature.txt").exists(),
        "feature.txt should be in working tree after ff"
    );
}

#[tokio::test]
async fn ff_main_to_with_diverged_main_fails() {
    let (_tmp, repo) = make_test_repo().await;

    // Create feature branch
    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "feature commit"], &repo).await;

    // Go back to main and make a diverging commit
    let _ = git_cmd(&["checkout", "main"], &repo).await;
    tokio::fs::write(repo.join("main-only.txt"), "main work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "main diverged"], &repo).await;

    let main_sha =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();
    let feature_sha = String::from_utf8_lossy(
        &git_cmd(&["rev-parse", "feature"], &repo)
            .await
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();

    let result = ff_main_to(&repo, &feature_sha, &main_sha).await;
    assert!(result.is_err(), "ff_main_to should fail when main diverged");
}

#[tokio::test]
async fn catchup_and_ff_leaves_clean_working_tree() {
    let (_tmp, repo) = make_test_repo().await;

    // Create a worktree with a feature branch (dedicated tempdir avoids collisions)
    let wt_tmp = tempfile::tempdir().unwrap();
    let wt_dir = wt_tmp.path().join("wt");
    let _ = git_cmd(
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            wt_dir.to_str().unwrap(),
            "main",
        ],
        &repo,
    )
    .await;

    // Make a commit in the worktree
    tokio::fs::write(wt_dir.join("feature.txt"), "feature work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt_dir).await;
    let _ = git_cmd(&["commit", "-m", "feature work"], &wt_dir).await;

    // Switch main repo to main (it might be on the feature branch after worktree add)
    let _ = git_cmd(&["checkout", "main"], &repo).await;

    let result = catchup_and_ff_to_main(&repo, &wt_dir, "feature").await;
    assert!(
        result.is_ok(),
        "catchup_and_ff should succeed: {:?}",
        result.err()
    );

    // Main repo working tree must be clean
    let status = git_cmd(&["status", "--porcelain"], &repo).await.unwrap();
    let status_output = String::from_utf8_lossy(&status.stdout).to_string();
    assert!(
        status_output.trim().is_empty(),
        "Main repo should be clean after catchup_and_ff, got: {}",
        status_output
    );

    // Feature file should be in main repo working tree
    assert!(
        repo.join("feature.txt").exists(),
        "feature.txt should appear in main repo after ff"
    );

    // Clean up worktree
    let _ = git_cmd(
        &["worktree", "remove", "--force", wt_dir.to_str().unwrap()],
        &repo,
    )
    .await;
}

#[tokio::test]
async fn catchup_and_ff_with_concurrent_main_commit() {
    let (_tmp, repo) = make_test_repo().await;

    // Create worktree with feature branch (dedicated tempdir avoids collisions)
    let wt_tmp = tempfile::tempdir().unwrap();
    let wt_dir = wt_tmp.path().join("wt");
    let _ = git_cmd(
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            wt_dir.to_str().unwrap(),
            "main",
        ],
        &repo,
    )
    .await;

    // Commit in worktree (touches different file than main will)
    tokio::fs::write(wt_dir.join("feature.txt"), "feature")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt_dir).await;
    let _ = git_cmd(&["commit", "-m", "feature"], &wt_dir).await;

    // Meanwhile, commit on main (different file -- no conflict)
    let _ = git_cmd(&["checkout", "main"], &repo).await;
    tokio::fs::write(repo.join("concurrent.txt"), "concurrent main work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "concurrent main commit"], &repo).await;

    // catchup_and_ff should merge main into feature, then ff main
    let result = catchup_and_ff_to_main(&repo, &wt_dir, "feature").await;
    assert!(
        result.is_ok(),
        "should succeed with non-conflicting concurrent commit: {:?}",
        result.err()
    );

    // Both files should be present and working tree clean
    let status = git_cmd(&["status", "--porcelain"], &repo).await.unwrap();
    let status_output = String::from_utf8_lossy(&status.stdout).to_string();
    assert!(
        status_output.trim().is_empty(),
        "Main repo should be clean, got: {}",
        status_output
    );
    assert!(repo.join("feature.txt").exists());
    assert!(repo.join("concurrent.txt").exists());

    let _ = git_cmd(
        &["worktree", "remove", "--force", wt_dir.to_str().unwrap()],
        &repo,
    )
    .await;
}

/// Sequential apply of two changes must leave a clean working tree after each.
/// Regression test: when HEAD is detached (e.g. after a failed `git pull --rebase`),
/// `ff_main_to` used `git reset --hard HEAD` which reset to the detached HEAD
/// position (old commit) instead of the newly-moved main. This left the working
/// tree dirty, causing the second apply to fail with "uncommitted changes".
#[tokio::test]
async fn sequential_ff_main_to_leaves_clean_working_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();

    // Set up repo with initial commit on main
    let _ = git_cmd(&["init"], repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
    let _ = tokio::fs::write(repo.join("base.txt"), "initial").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "init"], repo).await;

    let main_sha =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], repo).await.unwrap().stdout)
            .trim()
            .to_string();

    // Create branch1 from main with a file change
    let _ = git_cmd(&["checkout", "-b", "branch1"], repo).await;
    let _ = tokio::fs::write(repo.join("file1.txt"), "change from branch1").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "branch1 commit"], repo).await;
    let branch1_sha = String::from_utf8_lossy(
        &git_cmd(&["rev-parse", "branch1"], repo)
            .await
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();

    // Create branch2 from branch1 with another file change (so it's ff-able after branch1)
    let _ = git_cmd(&["checkout", "-b", "branch2"], repo).await;
    let _ = tokio::fs::write(repo.join("file2.txt"), "change from branch2").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "branch2 commit"], repo).await;
    let branch2_sha = String::from_utf8_lossy(
        &git_cmd(&["rev-parse", "branch2"], repo)
            .await
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();

    // Go back to main and DETACH HEAD (simulates pull --rebase failure)
    let _ = git_cmd(&["checkout", "main"], repo).await;
    let _ = git_cmd(&["checkout", "--detach"], repo).await;

    // Verify HEAD is detached
    let head_ref = git_cmd(&["symbolic-ref", "HEAD"], repo).await;
    assert!(
        head_ref.is_err() || !head_ref.unwrap().status.success(),
        "HEAD should be detached"
    );

    // Apply branch1: ff main from initial -> branch1
    let result = ff_main_to(repo, &branch1_sha, &main_sha).await;
    assert!(
        result.is_ok(),
        "ff_main_to for branch1 should succeed: {:?}",
        result.err()
    );

    // Working tree MUST be clean after first apply
    let dirty = auto_commit_safe_files_if_dirty(repo).await;
    assert!(
        !dirty,
        "Working tree must be clean after first ff_main_to -- \
                         this was the bug: detached HEAD caused reset to wrong position"
    );

    // Apply branch2: ff main from branch1 -> branch2
    let result2 = ff_main_to(repo, &branch2_sha, &branch1_sha).await;
    assert!(
        result2.is_ok(),
        "ff_main_to for branch2 should succeed: {:?}",
        result2.err()
    );

    // Working tree MUST be clean after second apply
    let dirty2 = auto_commit_safe_files_if_dirty(repo).await;
    assert!(
        !dirty2,
        "Working tree must be clean after second ff_main_to"
    );

    // Both files should exist in the working tree
    assert!(
        repo.join("file1.txt").exists(),
        "file1.txt from branch1 should exist"
    );
    assert!(
        repo.join("file2.txt").exists(),
        "file2.txt from branch2 should exist"
    );
}

/// Push-main-in-background must not leave the working tree dirty or detach HEAD.
/// Regression test: the old `pull --rebase` approach could detach HEAD on
/// conflict, leaving dirty files that block subsequent change applies.
#[tokio::test]
async fn push_main_does_not_dirty_working_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();

    // Set up repo with initial commit (no origin remote)
    let _ = git_cmd(&["init"], repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
    let _ = tokio::fs::write(repo.join("base.txt"), "initial").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "init"], repo).await;

    // Push should be a no-op with no remote -- must not dirty anything
    push_main_in_background(repo);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let dirty = auto_commit_safe_files_if_dirty(repo).await;
    assert!(
        !dirty,
        "push_main_in_background must not dirty working tree"
    );

    // HEAD should still be on main (not detached)
    let head_ref = git_cmd(&["symbolic-ref", "HEAD"], repo).await;
    assert!(
        head_ref.is_ok() && head_ref.unwrap().status.success(),
        "HEAD should still be attached to main after push"
    );
}

/// ensure_head_on_main must clean up stale rebase state left by a killed
/// `git pull --rebase` or CC session. Uses fallback directory removal when
/// `git rebase --abort` fails (e.g. corrupt/incomplete state).
#[tokio::test]
async fn ensure_head_on_main_aborts_stale_rebase() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();

    git_cmd(&["init"], repo).await.unwrap();
    git_cmd(&["checkout", "-b", "main"], repo).await.unwrap();
    tokio::fs::write(repo.join("file.txt"), "v1").await.unwrap();
    git_cmd(&["add", "."], repo).await.unwrap();
    git_cmd(&["commit", "-m", "first"], repo).await.unwrap();
    tokio::fs::create_dir_all(repo.join(".git/rebase-merge"))
        .await
        .unwrap();

    ensure_head_on_main(repo).await;

    // Fallback should have removed the directory
    assert!(
        !repo.join(".git/rebase-merge").exists(),
        "rebase-merge dir should be removed after ensure_head_on_main"
    );
}

#[tokio::test]
async fn harden_marker_missing_when_no_db_row() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;
    assert_eq!(
        harden_marker_state(&pool, &repo_path, "any-branch").await,
        HardenMarkerState::Missing,
        "No DB row should report Missing"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn harden_marker_with_matching_head_sha_is_fresh() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "test-branch"], &repo_path).await;
    tokio::fs::write(repo_path.join("test.txt"), "hello")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "test.txt"], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "add test file"], &repo_path).await;

    let head_sha = current_head_sha(&repo_path).await.unwrap();
    record_hardened(&pool, &repo_path, "test-branch", &head_sha)
        .await
        .unwrap();

    assert!(
        is_harden_marker_fresh(&pool, &repo_path, "test-branch").await,
        "Marker with matching HEAD SHA should be fresh"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn harden_marker_becomes_stale_after_new_commit() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "test-branch"], &repo_path).await;
    tokio::fs::write(repo_path.join("file1.txt"), "first")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "file1.txt"], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "first commit"], &repo_path).await;

    let head_sha = current_head_sha(&repo_path).await.unwrap();
    record_hardened(&pool, &repo_path, "test-branch", &head_sha)
        .await
        .unwrap();
    assert!(
        is_harden_marker_fresh(&pool, &repo_path, "test-branch").await,
        "Should be fresh right after hardening"
    );

    tokio::fs::write(repo_path.join("file2.txt"), "second")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "file2.txt"], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "fix harden finding"], &repo_path).await;

    assert!(
        !is_harden_marker_fresh(&pool, &repo_path, "test-branch").await,
        "Should be STALE after new commit changes HEAD"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression for the badge-thread scenario: branch HEAD `X` is hardened,
/// then `X` is fast-forwarded into main. The marker stays fresh because HEAD
/// itself didn't change.
#[tokio::test]
async fn harden_marker_stays_fresh_when_branch_merged_to_main() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo_path).await;
    tokio::fs::write(repo_path.join("badge.tsx"), "<Badge/>")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "add badge"], &repo_path).await;

    let head_sha = current_head_sha(&repo_path).await.unwrap();
    record_hardened(&pool, &repo_path, "feature", &head_sha)
        .await
        .unwrap();
    assert!(is_harden_marker_fresh(&pool, &repo_path, "feature").await);

    let _ = git_cmd(&["checkout", "main"], &repo_path).await;
    let _ = git_cmd(&["merge", "--ff-only", "feature"], &repo_path).await;
    let _ = git_cmd(&["checkout", "feature"], &repo_path).await;

    assert!(
        is_harden_marker_fresh(&pool, &repo_path, "feature").await,
        "Marker must stay fresh when branch is merged to main without HEAD changing"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Main advancing with unrelated commits must not invalidate a fresh marker
/// when HEAD on the branch is unchanged.
#[tokio::test]
async fn harden_marker_stays_fresh_when_main_advances() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo_path).await;
    tokio::fs::write(repo_path.join("feature.txt"), "f")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "feature work"], &repo_path).await;

    let head_sha = current_head_sha(&repo_path).await.unwrap();
    record_hardened(&pool, &repo_path, "feature", &head_sha)
        .await
        .unwrap();

    let _ = git_cmd(&["checkout", "main"], &repo_path).await;
    tokio::fs::write(repo_path.join("other.txt"), "o")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "main work"], &repo_path).await;
    let _ = git_cmd(&["checkout", "feature"], &repo_path).await;

    assert!(
        is_harden_marker_fresh(&pool, &repo_path, "feature").await,
        "Marker must stay fresh when main advances without touching branch HEAD"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn harden_marker_state_distinguishes_missing_stale_fresh() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    assert_eq!(
        harden_marker_state(&pool, &repo_path, "feature").await,
        HardenMarkerState::Missing,
        "No DB row should report Missing"
    );

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo_path).await;
    tokio::fs::write(repo_path.join("a.txt"), "a").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "initial feature"], &repo_path).await;

    let head_sha = current_head_sha(&repo_path).await.unwrap();
    record_hardened(&pool, &repo_path, "feature", &head_sha)
        .await
        .unwrap();
    assert_eq!(
        harden_marker_state(&pool, &repo_path, "feature").await,
        HardenMarkerState::Fresh,
    );

    tokio::fs::write(repo_path.join("b.txt"), "b").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo_path).await;
    let _ = git_cmd(&["commit", "-m", "new commit after harden"], &repo_path).await;
    assert_eq!(
        harden_marker_state(&pool, &repo_path, "feature").await,
        HardenMarkerState::Stale,
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: marker must be findable via (repo_root, branch_name) even
/// after the worktree directory is gone. Stale-session recovery removes the
/// worktree before propose-change runs, and apply needs to trust the marker
/// without re-running `/harden` on already-hardened work.
#[tokio::test]
async fn harden_marker_survives_worktree_removal() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, repo_path) = make_test_repo().await;

    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    let branch_name = "claude-code/survives-removal";
    let o = git_cmd(
        &[
            "worktree",
            "add",
            "-b",
            branch_name,
            wt_path.to_str().unwrap(),
            "main",
        ],
        &repo_path,
    )
    .await
    .unwrap();
    assert!(
        o.status.success(),
        "worktree add failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    tokio::fs::write(wt_path.join("file.txt"), "work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt_path).await;
    let _ = git_cmd(&["commit", "-m", "work"], &wt_path).await;

    let head_sha = current_head_sha(&wt_path).await.unwrap();
    record_hardened(&pool, &repo_path, branch_name, &head_sha)
        .await
        .unwrap();
    assert_eq!(
        harden_marker_state(&pool, &repo_path, branch_name).await,
        HardenMarkerState::Fresh,
    );

    let o = git_cmd(
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        &repo_path,
    )
    .await
    .unwrap();
    assert!(o.status.success());

    assert_eq!(
        harden_marker_state(&pool, &repo_path, branch_name).await,
        HardenMarkerState::Fresh,
        "DB-backed marker must survive worktree removal",
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn floor_char_boundary_truncation_handles_multibyte() {
    // Regression: &s[..200] panics when byte 200 falls inside a multi-byte
    // character like em dash (U+2014, 3 bytes). floor_char_boundary avoids this.
    let mut s = String::new();
    // Fill with 198 ASCII chars, then an em dash (3 bytes = positions 198..201)
    for _ in 0..198 {
        s.push('a');
    }
    s.push('\u{2014}'); // U+2014, 3 bytes
    s.push_str("after");
    assert_eq!(s.len(), 206); // 198 + 3 + 5

    // Old code: &s[..200] would panic here (byte 200 is inside the em dash)
    // New code: floor_char_boundary(200) -> 198 (before the em dash)
    let safe_end = s.floor_char_boundary(200.min(s.len()));
    let truncated = &s[..safe_end];
    assert_eq!(truncated.len(), 198);
    assert!(truncated.is_char_boundary(truncated.len()));

    // Also verify min(200) with string shorter than 200
    let short = "h\u{00e9}llo w\u{00f6}rld";
    let safe_end = short.floor_char_boundary(200.min(short.len()));
    let truncated = &short[..safe_end];
    assert_eq!(truncated, short);
}

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

/// Recovery must NOT propose a Change when the branch has commits but zero net
/// diff. Without this gate, `safe_cleanup_worktree` creates a `changes` row with
/// `file_count=0`, which renders Apply/Discard buttons that do nothing useful.
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
        "crates/lucidos-engine/tests/api_e2e.rs".into()
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

#[test]
fn repo_at_different_path_is_external() {
    let dev = std::path::PathBuf::from("/Users/me/IdeaProjects/lucidos");
    let other = std::path::PathBuf::from("/Users/me/IdeaProjects/user-acquisition");
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

