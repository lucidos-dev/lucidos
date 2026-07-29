//! Tests for [`super::cleanup_failed_spawn`] — the failure-path cleanup that
//! runs when `spawn_or_resume` errors after the worktree context was
//! resolved.
//!
//! Regression background: the old inline cleanup ran
//! `git worktree remove --force` + `git branch -D` unconditionally under
//! `let _ =`. On a RESUME failure that destroyed the pre-existing worktree
//! and force-deleted the branch holding the thread's committed work — silent
//! data loss. The helper must only delete what the failed spawn attempt
//! itself created, and must never delete a branch whose worktree survived.

use super::common::make_test_repo;
use super::*;

async fn branch_exists(repo: &std::path::Path, branch: &str) -> bool {
    git_cmd(&["rev-parse", "--verify", branch], repo)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// THE data-loss regression: a spawn failure on a resumed thread (worktree +
/// branch existed before this attempt) must leave both fully intact.
#[tokio::test]
async fn failed_resume_spawn_keeps_preexisting_worktree_and_branch() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-resume");

    // Simulate a resumed thread: branch with committed work, checked out in
    // its persistent worktree.
    let out = worktree_add(&repo, &wt, &["-b", "claude-code/resumed"])
        .await
        .unwrap();
    assert!(out.status.success(), "fixture worktree_add failed");
    tokio::fs::write(wt.join("work.txt"), "committed work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt).await;
    let _ = git_cmd(&["commit", "-m", "thread work"], &wt).await;

    cleanup_failed_spawn(&repo, Some(&wt), "claude-code/resumed", false, false).await;

    assert!(
        wt.exists(),
        "pre-existing worktree must survive a failed resume spawn"
    );
    assert!(
        branch_exists(&repo, "claude-code/resumed").await,
        "branch with committed work must survive a failed resume spawn"
    );
}

/// Fresh-spawn failure: the worktree and branch this attempt created are
/// both removed (nothing of value exists on them yet).
#[tokio::test]
async fn failed_fresh_spawn_removes_created_worktree_and_branch() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-fresh");

    let out = worktree_add(&repo, &wt, &["-b", "claude-code/fresh"])
        .await
        .unwrap();
    assert!(out.status.success(), "fixture worktree_add failed");

    cleanup_failed_spawn(&repo, Some(&wt), "claude-code/fresh", true, true).await;

    assert!(!wt.exists(), "freshly created worktree must be removed");
    assert!(
        !branch_exists(&repo, "claude-code/fresh").await,
        "freshly created branch must be deleted"
    );
}

/// Ordering: when the worktree removal FAILS, the branch delete must be
/// skipped — the branch is still checked out in the surviving worktree, and
/// deleting it out from under the stranded-worktree sweeper would lose the
/// recovery anchor.
#[tokio::test]
async fn failed_worktree_removal_skips_branch_delete() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-real");

    let out = worktree_add(&repo, &wt, &["-b", "claude-code/held"])
        .await
        .unwrap();
    assert!(out.status.success(), "fixture worktree_add failed");

    // Point the cleanup at a directory git does NOT know as a worktree, so
    // `git worktree remove` fails — simulating a locked/unremovable worktree.
    let bogus = wt_base.path().join("not-a-worktree");
    tokio::fs::create_dir(&bogus).await.unwrap();

    cleanup_failed_spawn(&repo, Some(&bogus), "claude-code/held", true, true).await;

    assert!(
        branch_exists(&repo, "claude-code/held").await,
        "branch delete must be skipped when the worktree removal failed"
    );
}

/// Contract pin for the (worktree_created=false, branch_created=true)
/// combination no caller produces today: the kept pre-existing worktree
/// still holds the branch checked out, so the branch must survive — and the
/// helper must not early-return in a way that would change this if the
/// branch arm gains behavior.
#[tokio::test]
async fn kept_worktree_suppresses_branch_delete_even_when_branch_created() {
    let (_tmp, repo) = make_test_repo().await;
    let wt_base = tempfile::tempdir().unwrap();
    let wt = wt_base.path().join("thread-kept");

    let out = worktree_add(&repo, &wt, &["-b", "claude-code/kept"])
        .await
        .unwrap();
    assert!(out.status.success(), "fixture worktree_add failed");

    cleanup_failed_spawn(&repo, Some(&wt), "claude-code/kept", false, true).await;

    assert!(wt.exists(), "pre-existing worktree must be kept");
    assert!(
        branch_exists(&repo, "claude-code/kept").await,
        "branch checked out in a kept worktree must survive"
    );
}

/// Branch-only cleanup: spawn failed before any worktree existed (path None)
/// but after the branch was created — the branch alone is deleted.
#[tokio::test]
async fn failed_spawn_without_worktree_deletes_created_branch() {
    let (_tmp, repo) = make_test_repo().await;
    let out = git_cmd(&["branch", "claude-code/dangling"], &repo)
        .await
        .unwrap();
    assert!(out.status.success(), "fixture branch create failed");

    cleanup_failed_spawn(&repo, None, "claude-code/dangling", true, true).await;

    assert!(
        !branch_exists(&repo, "claude-code/dangling").await,
        "created branch must be deleted even when no worktree exists"
    );
}
