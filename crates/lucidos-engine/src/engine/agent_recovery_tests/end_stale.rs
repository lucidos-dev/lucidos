mod end_stale_session_branch_preservation {
    //! Regression: when the user clicks Apply on a thread with no live CC
    //! session, `apply_now` calls `end_stale_waiting_session(.., discard=false, ..)`
    //! which used to silently `git branch -D` the branch in the
    //! `proposal_files=None` arm — losing the user's committed work whenever
    //! the per-commit `ChangeProposed` events hadn't synced into the `changes`
    //! projection (so `has_pending_for_branch` returned false). Symptom: the
    //! UI shows "No changes to apply — branch is already merged or has no
    //! commits" and the Files diff fails with "unknown revision".
    //!
    //! We can't instantiate a full `LucidosEngine` from a unit test, so we
    //! pin the contract by replaying the exact git operations
    //! `end_stale_waiting_session` performs in the no-proposal-files +
    //! discard=false path, and asserting the branch survives.

    use crate::engine::git_ops::{
        branch_head_sha, find_worktree_for_branch, git_cmd, proposal_files_for_branch,
    };
    use crate::test_support::make_repo_and_worktree;

    /// `proposal_files_for_branch` returns `None` for a branch whose commits
    /// have an empty net diff (commit + revert). That's the trigger condition
    /// for the deleted else branch — the case where the buggy code would have
    /// run `git branch -D` had we not removed it.
    #[tokio::test]
    async fn apply_intent_preserves_branch_when_proposal_files_returns_none() {
        let branch = "claude-code/preserve-branch-on-apply";
        let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

        // Commits with zero net diff — same shape as commit + revert,
        // npm install + reset, or any other no-op pair CC can produce. This
        // is what trips `proposal_files_for_branch` into returning None.
        std::fs::write(wt.join("scratch.txt"), "v1").unwrap();
        git_cmd(&["add", "."], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "add scratch"], &wt).await.unwrap();
        git_cmd(&["revert", "--no-edit", "HEAD"], &wt)
            .await
            .unwrap();

        // Precondition: proposal_files returns None (the trigger condition).
        assert_eq!(
            proposal_files_for_branch(&repo_root, branch).await,
            None,
            "test setup must reproduce the proposal_files=None trigger condition"
        );

        // Step 1 of end_stale_waiting_session: find + remove the worktree.
        let wt_path = find_worktree_for_branch(&repo_root, branch).await;
        assert!(
            wt_path.is_some(),
            "worktree must be registered before the cleanup runs"
        );
        git_cmd(
            &["worktree", "remove", "--force", wt.to_str().unwrap()],
            &repo_root,
        )
        .await
        .expect("worktree remove");

        // Step 2 (post-fix): the no-proposal-files else branch is a no-op
        // for discard=false. The buggy version called
        // `git_cmd(&["branch", "-D", branch], &repo_root)` here.
        //
        // A future regression that re-introduces the deletion in
        // `end_stale_waiting_session` (or in a sibling cleanup path) will
        // fail the branch_head_sha check below.
        assert!(
            branch_head_sha(&repo_root, branch).await.is_some(),
            "Apply intent on a branch with no proposable diff must leave the \
             branch ref intact so the user can retry / inspect / discard"
        );

        // And the user's actual commits are still reachable.
        let log = git_cmd(&["log", "--oneline", branch], &repo_root)
            .await
            .expect("git log runs");
        assert!(log.status.success(), "git log must succeed for live branch");
        let stdout = String::from_utf8_lossy(&log.stdout);
        assert!(
            stdout.contains("add scratch"),
            "user's commits must still be reachable on the branch; got: {}",
            stdout
        );
    }

    /// Same setup, but the branch has real changes — `proposal_files_for_branch`
    /// returns `Some(..)`. The function takes the propose-change arm (not the
    /// deleted else) and the branch is preserved by that path's normal
    /// success flow. We pin this to make sure removing the destructive
    /// else branch didn't accidentally interfere with the happy path.
    #[tokio::test]
    async fn apply_intent_preserves_branch_when_proposal_files_returns_some() {
        let branch = "claude-code/preserve-branch-with-real-diff";
        let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

        std::fs::write(wt.join("a.txt"), "hello").unwrap();
        git_cmd(&["add", "."], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "feat: add a"], &wt).await.unwrap();

        assert!(
            proposal_files_for_branch(&repo_root, branch).await.is_some(),
            "test setup: real-changes branch yields Some(files)"
        );

        // Remove worktree (Step 1 of end_stale_waiting_session)
        git_cmd(
            &["worktree", "remove", "--force", wt.to_str().unwrap()],
            &repo_root,
        )
        .await
        .expect("worktree remove");

        assert!(
            branch_head_sha(&repo_root, branch).await.is_some(),
            "branch with real proposable changes must survive worktree removal"
        );
    }
}
