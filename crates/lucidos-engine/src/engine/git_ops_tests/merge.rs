use super::common::make_test_repo;
use super::*;

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
        &["commit", "-m", "Coding agent changes (auto-committed)"],
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

#[test]
fn index_lock_collision_is_told_apart_from_a_real_git_failure() {
    assert!(is_index_lock_collision(
        "fatal: Unable to create '/ws/.git/index.lock': File exists.\n\n\
         Another git process seems to be running in this repository"
    ));
    // Not a lock collision -- these must fail on the first attempt, never retry.
    assert!(!is_index_lock_collision(
        "error: Your local changes to the following files would be overwritten by checkout"
    ));
    assert!(!is_index_lock_collision("fatal: invalid reference: main"));
    assert!(!is_index_lock_collision(""));
}

/// Regression (2026-08-03): `ff_main_to` advanced `main` and then lost the race
/// for `.git/index.lock` on the `checkout -f main` that syncs the working tree.
/// The failure was logged and swallowed, so the repo root stayed at the OLD
/// commit while `main` pointed at the new one -- every file the merge added
/// looked deleted to the next `git add -A` of `data/`, and the following
/// `commit_all_dirty` committed that deletion on top of main. In the nightly
/// this reverted a just-applied app change (`style-b.css` merged, then removed
/// by a "Script task output" commit). The sync now waits the lock out.
#[tokio::test]
async fn ff_main_to_syncs_the_working_tree_when_the_index_lock_is_briefly_held() {
    let (_tmp, repo) = make_test_repo().await;

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
    let _ = git_cmd(&["checkout", "main"], &repo).await;

    // Stand in for the other writer on this repo (libgit2 via ArtifactManager),
    // holding the index lock across the moment main advances and releasing it
    // shortly after. `update-ref` takes `refs/heads/main.lock`, not the index
    // lock, so the ref still moves -- only the working-tree sync collides.
    let lock = repo.join(".git/index.lock");
    tokio::fs::write(&lock, b"").await.unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let _ = tokio::fs::remove_file(&lock).await;
    });

    let result = ff_main_to(&repo, &feature_sha, &main_sha).await;
    assert!(
        result.is_ok(),
        "ff_main_to should succeed: {:?}",
        result.err()
    );

    assert!(
        repo.join("feature.txt").exists(),
        "the merged file must be in the working tree after ff_main_to, even when \
         the index lock was held while main advanced -- otherwise the next \
         auto-commit of data/ records it as deleted"
    );
    let status = git_cmd(&["status", "--porcelain"], &repo).await.unwrap();
    let status_output = String::from_utf8_lossy(&status.stdout).to_string();
    assert!(
        status_output.trim().is_empty(),
        "working tree should be clean and level with main, got: {}",
        status_output
    );
}

/// A sync that never wins the lock must FAIL the merge, not report a clean
/// apply over a working tree it knows is behind main. The callers' retry loops
/// re-enter on the Err (the ff is a no-op by then), and a caller that gives up
/// surfaces a recoverable failed apply rather than letting the next auto-commit
/// of `data/` revert the merged files.
#[tokio::test]
async fn ff_main_to_fails_when_the_working_tree_cannot_be_synced() {
    let (_tmp, repo) = make_test_repo().await;

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
    let _ = git_cmd(&["checkout", "main"], &repo).await;

    // Held for the whole retry budget and never released.
    tokio::fs::write(repo.join(".git/index.lock"), b"")
        .await
        .unwrap();

    let err = ff_main_to(&repo, &feature_sha, &main_sha)
        .await
        .expect_err("a working tree left behind main must fail the merge");
    assert!(
        err.to_string().contains("working tree could not be synced"),
        "the error must name the unsynced working tree, got: {}",
        err
    );
}

/// Regression (2026-08-03, second occurrence): `update-ref` publishes the merge
/// instantly while the `checkout -f main` that syncs the repo-root working tree
/// is a separate process. A `commit_all_dirty` landing in that window resets its
/// index to the NEW head, stages the OLD working tree over it, and commits every
/// just-merged file as deleted, straight onto main. Retrying cannot close it:
/// neither side fails. `ff_main_to` must therefore hold REPO_WORKTREE_MUTEX
/// across publish AND sync, which this pins by taking the lock first and proving
/// main cannot move until it is released.
#[tokio::test]
async fn ff_main_to_cannot_publish_main_while_a_worktree_snapshot_holds_the_lock() {
    let (_tmp, repo) = make_test_repo().await;

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
    let _ = git_cmd(&["checkout", "main"], &repo).await;

    // Stand in for `commit_dirty_logged`, which holds this lock while it
    // snapshots the working tree.
    let snapshot_guard = REPO_WORKTREE_MUTEX.lock().await;

    let merge_repo = repo.clone();
    let merge_branch = feature_sha.clone();
    let merge_main = main_sha.clone();
    let merging =
        tokio::spawn(async move { ff_main_to(&merge_repo, &merge_branch, &merge_main).await });

    // While the snapshot holds the lock the merge must not have published main,
    // so a snapshot of the working tree can never disagree with the ref.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let during =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();
    assert_eq!(
        during, main_sha,
        "main must not advance while a worktree snapshot holds REPO_WORKTREE_MUTEX"
    );

    drop(snapshot_guard);
    merging
        .await
        .expect("merge task panicked")
        .expect("ff_main_to should succeed once the snapshot releases the lock");

    let after =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();
    assert_eq!(
        after, feature_sha,
        "main must advance once the lock is free"
    );
    assert!(
        repo.join("feature.txt").exists(),
        "the merged file must be in the working tree by the time the lock is released"
    );
}

/// Regression (2026-08-03, same class as the two above): the exclusion above is
/// only worth what its LIFETIME is worth, and it used to be held by the
/// snapshot's caller around a `tokio::time::timeout`. `commit_all_dirty` does
/// its libgit2 work inside `spawn_blocking`, and a blocking task cannot be
/// cancelled: when the 30s ceiling fired, `commit_dirty_logged` logged the
/// timeout, returned, and dropped its guard while the closure was still inside
/// `reset_index_to_head` / `add_all` / `commit_index`. A concurrent `ff_main_to`
/// then took the free lock and published main mid-snapshot, which is exactly the
/// interleaving that records every just-merged file as deleted. The guard now
/// travels INTO the blocking closure, so it is released when the snapshot really
/// finishes rather than when its caller walks away.
#[tokio::test]
async fn a_timed_out_auto_commit_snapshot_still_excludes_a_concurrent_publish() {
    let (_tmp, repo) = make_test_repo().await;
    // Opens the repo `make_test_repo` just created, and commits a .gitignore on
    // main, so build it BEFORE the shas below are captured.
    let artifacts = crate::core::ArtifactManager::new(repo.clone()).unwrap();

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
    let _ = git_cmd(&["checkout", "main"], &repo).await;

    // Stall the snapshot at a real contention point: the ArtifactManager's repo
    // handle, which every artifact commit takes. `commit_all_dirty`'s closure
    // parks there AFTER it owns the worktree exclusion, which is the state this
    // test is about. The oneshot makes the stall observably in place before the
    // snapshot starts, so nothing here depends on a sleep.
    let repo_handle = artifacts.repo_handle_for_test();
    let (held_tx, held_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let stall = tokio::task::spawn_blocking(move || {
        let _repo = repo_handle.lock().unwrap();
        held_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    held_rx.await.expect("the stall task must take the handle");

    // REPO_WORKTREE_MUTEX is process-global and every other `ff_main_to` test in
    // this binary takes it too. Own it first and register the snapshot as the
    // next waiter (tokio hands the lock out in request order), so the guard the
    // assertions below are about is provably the snapshot's own.
    let queue_head = REPO_WORKTREE_MUTEX.lock().await;
    let mut snapshot = Box::pin(artifacts.commit_all_dirty("Script task output"));
    tokio::select! {
        biased;
        _ = &mut snapshot => panic!("the snapshot cannot finish while the repo handle is stalled"),
        _ = std::future::ready(()) => {}
    }
    drop(queue_head);

    // The caller gives up and walks away: `commit_dirty_logged` on its ceiling.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(500), &mut snapshot)
            .await
            .is_err(),
        "the stalled snapshot must still be in flight when its caller's timeout fires"
    );
    drop(snapshot);

    let merge_repo = repo.clone();
    let merge_branch = feature_sha.clone();
    let merge_main = main_sha.clone();
    let merging =
        tokio::spawn(async move { ff_main_to(&merge_repo, &merge_branch, &merge_main).await });

    // The caller's future is gone but the snapshot is not, so the merge must
    // still be excluded -- publishing main here is what commits the merged files
    // as deleted.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let during =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();
    assert_eq!(
        during, main_sha,
        "main must not advance while an abandoned snapshot is still writing the index"
    );

    // Let the snapshot finish. Only its real completion may free the exclusion.
    release_tx.send(()).unwrap();
    stall.await.expect("stall task panicked");
    merging
        .await
        .expect("merge task panicked")
        .expect("ff_main_to should succeed once the snapshot has really finished");

    let after =
        String::from_utf8_lossy(&git_cmd(&["rev-parse", "main"], &repo).await.unwrap().stdout)
            .trim()
            .to_string();
    assert_eq!(
        after, feature_sha,
        "main must advance once the lock is free"
    );
    assert!(
        repo.join("feature.txt").exists(),
        "the merged file must be in the working tree once the merge completes"
    );
}

/// The retry is scoped to lock collisions: a command that fails for its own
/// reasons must come back on the first attempt, not after the full budget.
#[tokio::test]
async fn git_cmd_await_index_lock_does_not_retry_a_real_failure() {
    let (_tmp, repo) = make_test_repo().await;

    let started = std::time::Instant::now();
    let out = git_cmd_await_index_lock(&["checkout", "-f", "no-such-branch"], &repo)
        .await
        .unwrap();
    assert!(!out.status.success());
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "a non-lock failure must not burn the retry budget, took {:?}",
        started.elapsed()
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
/// `git pull --rebase` or Claude Code session. Uses fallback directory removal when
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

/// Regression: a Lucidos-source coding-agent spawn must branch its worktree off
/// the local default branch (`main`), NEVER off whatever the shared repo
/// checkout happens to have at `HEAD`. The real bug: the dev repo's one primary
/// checkout sat parked on an unrelated in-flight `claude-code/*` branch, so a
/// new thread that did nothing surfaced that branch's commits as phantom
/// "pending changes" because the worktree was `Created from HEAD`.
#[tokio::test]
async fn lucidos_source_worktree_bases_off_main_not_parked_head() {
    let (_tmp, repo) = make_test_repo().await; // `main` + init.txt

    // A *different* session's in-flight branch with a commit of its own.
    git_cmd(
        &["checkout", "-b", "claude-code/20260601-204403-parked"],
        &repo,
    )
    .await
    .unwrap();
    tokio::fs::write(repo.join("phantom.txt"), "backup work from another session")
        .await
        .unwrap();
    git_cmd(&["add", "."], &repo).await.unwrap();
    git_cmd(&["commit", "-m", "fix(backup): unrelated work"], &repo)
        .await
        .unwrap();

    // Park the shared primary checkout on that branch — HEAD is now off `main`,
    // exactly the state the dev repo was in when the Rectangle thread spawned.
    let parked_head = git_cmd(&["rev-parse", "HEAD"], &repo).await.unwrap();
    let main_head = git_cmd(&["rev-parse", "main"], &repo).await.unwrap();
    assert_ne!(
        String::from_utf8_lossy(&parked_head.stdout).trim(),
        String::from_utf8_lossy(&main_head.stdout).trim(),
        "precondition: parked HEAD must differ from main"
    );

    // A Lucidos-source spawn (is_external_repo = false) must resolve `main`.
    let base = resolve_worktree_base(&repo, false).await;
    assert_eq!(
        base.as_deref(),
        Some("main"),
        "Lucidos-source spawn must base off the local default branch, not HEAD"
    );

    // Drive the real worktree-creation path with that base, mirroring
    // spawn_context.rs: `["-b", branch]` + the optional trailing base ref.
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("thread-new");
    let new_branch = "claude-code/20260602-053944-new";
    let mut args = vec!["-b", new_branch];
    if let Some(ref b) = base {
        args.push(b);
    }
    let out = worktree_add(&repo, &wt_path, &args)
        .await
        .expect("worktree_add returned Err");
    assert!(
        out.status.success(),
        "worktree add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The new thread inherits NONE of the parked branch's work.
    assert!(
        !wt_path.join("phantom.txt").exists(),
        "new worktree leaked the parked branch's file — it was cut from HEAD, not main"
    );
    let extra = git_cmd(&["rev-list", &format!("main..{new_branch}")], &repo)
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&extra.stdout).trim().is_empty(),
        "new branch carries commits beyond main — phantom 'pending changes' will appear"
    );

    let _ = git_cmd(
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        &repo,
    )
    .await;
}

/// External-repo spawns keep their own contract: branch off the `origin` default
/// when a remote exists, and fall back to `HEAD` (→ `None`) only when there is
/// no `origin` — the user's checked-out branch is the right base there.
#[tokio::test]
async fn external_repo_worktree_base_is_none_without_origin() {
    let (_tmp, repo) = make_test_repo().await;
    let base = resolve_worktree_base(&repo, true).await;
    assert_eq!(
        base, None,
        "external repo with no origin remote should branch from HEAD (None)"
    );
}
