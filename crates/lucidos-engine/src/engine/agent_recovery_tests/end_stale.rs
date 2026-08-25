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

    use crate::engine::agent_recovery::rescue_stale_worktree;
    use crate::engine::git_ops::{
        branch_head_sha, find_worktree_for_branch, git_cmd, proposal_files_for_branch,
        WorktreeLookup,
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
        git_cmd(&["commit", "-m", "add scratch"], &wt)
            .await
            .unwrap();
        git_cmd(&["revert", "--no-edit", "HEAD"], &wt)
            .await
            .unwrap();

        // Precondition: proposal_files returns None (the trigger condition).
        assert_eq!(
            proposal_files_for_branch(&repo_root, branch).await,
            None,
            "test setup must reproduce the proposal_files=None trigger condition"
        );

        // Step 1 of end_stale_waiting_session: find the worktree and rescue it
        // with a commit. It is NOT removed on this path, so the assertion below
        // covers the tree as well as the branch.
        assert!(
            matches!(
                find_worktree_for_branch(&repo_root, branch).await,
                WorktreeLookup::Found(_)
            ),
            "worktree must be registered before the settle runs"
        );
        rescue_stale_worktree(&wt).await;
        assert!(
            wt.exists(),
            "the non-discard settle rescues the tree and leaves it to the \
             cleanup worker (ADR 0035)"
        );

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

    /// Same setup, but the branch has real changes, so
    /// `proposal_files_for_branch` returns `Some(..)`. The function takes the
    /// propose-change arm (not the deleted else) and the branch is preserved by
    /// that path's normal success flow. We pin this to make sure removing the
    /// destructive else branch did not interfere with the happy path.
    #[tokio::test]
    async fn apply_intent_preserves_branch_when_proposal_files_returns_some() {
        let branch = "claude-code/preserve-branch-with-real-diff";
        let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

        std::fs::write(wt.join("a.txt"), "hello").unwrap();
        git_cmd(&["add", "."], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "feat: add a"], &wt)
            .await
            .unwrap();

        assert!(
            proposal_files_for_branch(&repo_root, branch)
                .await
                .is_some(),
            "test setup: real-changes branch yields Some(files)"
        );

        // Step 1 of end_stale_waiting_session: rescue the worktree, keep it.
        rescue_stale_worktree(&wt).await;

        assert!(
            branch_head_sha(&repo_root, branch).await.is_some(),
            "a branch with real proposable changes must survive the settle"
        );
        assert!(wt.exists(), "and so must its worktree");
    }
}

mod unknown_worktree_lookup_is_not_a_no {
    //! Regression for the class in `.claude/rules/rust.md`: a probe that could
    //! not run must never authorize a destructive arm.
    //!
    //! `find_worktree_for_branch` used to answer `None` for a spawn failure, a
    //! non-zero exit and its 30s timeout, all routine on a saturated host. The
    //! stale settle read that as "nothing holds the branch" and skipped the
    //! rescue, or, on a Discard, force-removed whatever tree it did find.
    //!
    //! The lookup is a parameter now, so this drives the `Unknown` arm against a
    //! real repo. A bogus root would prove nothing, since the removal would have
    //! failed against it anyway.

    use crate::engine::agent_recovery::settle_stale_worktree;
    use crate::engine::git_ops::{branch_head_sha, proposal_files_for_branch, WorktreeLookup};
    use crate::test_support::make_repo_and_worktree;
    use uuid::Uuid;

    const BRANCH: &str = "lucidos-claude-code-repo-unknown-lookup-3e0ffee0";

    /// A Discard is the one arm that removes a tree. It must still not remove
    /// one git never located, even though the user asked for the discard.
    #[tokio::test]
    async fn an_unknown_lookup_never_removes_a_stale_worktree() {
        let (_tmp, repo_root, wt) = make_repo_and_worktree(BRANCH).await;
        std::fs::write(wt.join("seed.txt"), "uncommitted work").unwrap();

        settle_stale_worktree(
            &repo_root,
            BRANCH,
            Uuid::new_v4(),
            true,
            WorktreeLookup::Unknown,
        )
        .await;

        assert!(
            wt.exists(),
            "a Discard whose lookup could not run must leave the tree alone"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("seed.txt")).unwrap(),
            "uncommitted work",
            "and the uncommitted work in it must be untouched"
        );
        assert!(
            branch_head_sha(&repo_root, BRANCH).await.is_some(),
            "the branch survives too: nothing downstream may read the skip as a delete"
        );
    }

    /// The same skip on the non-discard arm. An auto-commit is not destructive,
    /// but committing into a tree we could not locate is still acting on a guess.
    #[tokio::test]
    async fn an_unknown_lookup_never_rescues_a_stale_worktree() {
        let (_tmp, repo_root, wt) = make_repo_and_worktree(BRANCH).await;
        std::fs::write(wt.join("seed.txt"), "uncommitted work").unwrap();

        settle_stale_worktree(
            &repo_root,
            BRANCH,
            Uuid::new_v4(),
            false,
            WorktreeLookup::Unknown,
        )
        .await;

        assert_eq!(
            proposal_files_for_branch(&repo_root, BRANCH).await,
            None,
            "the rescue commit must not have run: it is what puts the edit on the branch"
        );
    }

    /// The counterpart, so the two skips above are not vacuous: a located tree
    /// really is removed on a Discard.
    #[tokio::test]
    async fn a_found_lookup_still_removes_the_discarded_worktree() {
        let (_tmp, repo_root, wt) = make_repo_and_worktree(BRANCH).await;
        std::fs::write(wt.join("seed.txt"), "uncommitted work").unwrap();

        settle_stale_worktree(
            &repo_root,
            BRANCH,
            Uuid::new_v4(),
            true,
            WorktreeLookup::Found(wt.clone()),
        )
        .await;

        assert!(
            !wt.exists(),
            "the Discard arm removes the tree, which is what Unknown must never do"
        );
    }
}

mod app_thread_stale_settle {
    //! An **app** coding-agent thread's worktree and branch live in the
    //! WORKSPACE git, not the Lucidos source repo. `end_stale_waiting_session`
    //! used to ask the Lucidos repo about both, so Apply proposed nothing, Stop
    //! settled with no Apply card, and Discard left the branch behind.
    //!
    //! These pin the routing target with a real workspace-shaped repo, and the
    //! worktree property the settle now holds: a Discard removes the tree,
    //! everything else rescues it and leaves it alone.

    use crate::engine::agent_recovery::{
        remove_discarded_stale_worktree, rescue_stale_worktree, stale_discard_branch_delete_root,
        stale_session_repo, StaleSessionRepo,
    };
    use crate::engine::git_ops::{
        branch_head_sha, create_sparse_app_worktree, git_cmd, proposal_files_for_branch,
        worktrees_dir,
    };
    use std::path::{Path, PathBuf};

    const APP_ID: &str = "habit-tracker";
    const BRANCH: &str = "lucidos-claude-code-app-habit-tracker-add-streaks-ae6846f4";

    async fn init_repo(root: &Path, seed: &str) {
        git_cmd(&["init", "--initial-branch=main"], root)
            .await
            .unwrap();
        git_cmd(&["config", "user.email", "settle@test"], root)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Settle Test"], root)
            .await
            .unwrap();
        tokio::fs::write(root.join(seed), "seed").await.unwrap();
        git_cmd(&["add", "."], root).await.unwrap();
        git_cmd(&["commit", "-m", "seed"], root).await.unwrap();
    }

    /// A workspace git holding the thread's app, a sibling app and an artifact,
    /// plus a separate Lucidos-source repo. Returns
    /// `(tmpdir, workspace, lucidos, app worktree)`, with the worktree a sparse
    /// cone over the thread's own app, exactly as the spawn path builds it.
    async fn workspace_with_app_worktree() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        let lucidos = tmp.path().join("lucidos");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&lucidos).unwrap();

        init_repo(&lucidos, "Cargo.toml").await;
        init_repo(&workspace, "README.md").await;
        for dir in [
            "data/apps/habit-tracker",
            "data/apps/other",
            "data/artifacts",
        ] {
            tokio::fs::create_dir_all(workspace.join(dir))
                .await
                .unwrap();
        }
        tokio::fs::write(
            workspace.join("data/apps/habit-tracker/index.html"),
            "<h1>h</h1>",
        )
        .await
        .unwrap();
        tokio::fs::write(workspace.join("data/apps/other/index.html"), "<h1>o</h1>")
            .await
            .unwrap();
        tokio::fs::write(workspace.join("data/artifacts/report.md"), "notes")
            .await
            .unwrap();
        git_cmd(&["add", "."], &workspace).await.unwrap();
        git_cmd(&["commit", "-m", "scaffold"], &workspace)
            .await
            .unwrap();

        let wt = worktrees_dir(&workspace).join("thread-ae6846f4");
        create_sparse_app_worktree(&workspace, APP_ID, BRANCH, &wt)
            .await
            .expect("the spawn path's sparse app worktree");
        (tmp, workspace, lucidos, wt)
    }

    async fn edit_app(wt: &Path, body: &str) {
        tokio::fs::write(wt.join("data/apps/habit-tracker/index.html"), body)
            .await
            .unwrap();
    }

    /// The bug. The app branch is proposable from the workspace git and
    /// invisible to the Lucidos repo. A settle rooted at the Lucidos repo
    /// therefore reads committed work as "no proposable diff".
    #[tokio::test]
    async fn an_app_branch_is_proposable_only_from_the_workspace_git() {
        let (_tmp, workspace, lucidos, wt) = workspace_with_app_worktree().await;
        edit_app(&wt, "<h1>streaks</h1>").await;
        git_cmd(&["add", "."], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "feat: streaks"], &wt)
            .await
            .unwrap();

        assert_eq!(
            proposal_files_for_branch(&workspace, BRANCH).await,
            Some(vec!["data/apps/habit-tracker/index.html".to_string()]),
            "the workspace git owns the app branch, so it yields the proposal"
        );
        assert_eq!(
            proposal_files_for_branch(&lucidos, BRANCH).await,
            None,
            "the Lucidos repo has never heard of this branch, which is why the \
             old hardcoded root proposed nothing"
        );
        assert_eq!(
            stale_session_repo(Some("app"), false, &lucidos, &workspace),
            StaleSessionRepo::Owned(workspace),
            "the settle must route an app thread at the repo that can answer"
        );
    }

    /// The rescue commit is what carries uncommitted work into the Apply card:
    /// the proposal reads committed state only.
    #[tokio::test]
    async fn the_rescue_commit_puts_uncommitted_app_work_into_the_proposal() {
        let (_tmp, workspace, _lucidos, wt) = workspace_with_app_worktree().await;
        edit_app(&wt, "<h1>uncommitted</h1>").await;

        assert_eq!(
            proposal_files_for_branch(&workspace, BRANCH).await,
            None,
            "precondition: nothing is committed yet, so there is nothing to propose"
        );

        rescue_stale_worktree(&wt).await;

        assert_eq!(
            proposal_files_for_branch(&workspace, BRANCH).await,
            Some(vec!["data/apps/habit-tracker/index.html".to_string()]),
            "the rescue commit must land the edit on the branch"
        );
        assert!(
            wt.exists(),
            "the tree stays: reclaiming it is the cleanup worker's job (ADR 0035)"
        );
    }

    /// Discard is the one arm that removes the tree, and it has to: git refuses
    /// to delete a branch a worktree still has checked out.
    #[tokio::test]
    async fn discarding_an_app_thread_leaves_no_branch_and_no_worktree() {
        let (_tmp, workspace, lucidos, wt) = workspace_with_app_worktree().await;
        edit_app(&wt, "<h1>throwaway</h1>").await;
        git_cmd(&["add", "."], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "wip"], &wt).await.unwrap();

        let repo = stale_session_repo(Some("app"), false, &lucidos, &workspace);
        remove_discarded_stale_worktree(&workspace, &wt).await;
        let delete_root = stale_discard_branch_delete_root(true, &repo, BRANCH)
            .expect("a Discard on an owned repo authorizes the delete");
        git_cmd(&["branch", "-D", BRANCH], delete_root)
            .await
            .unwrap();

        assert!(!wt.exists(), "the discarded worktree must be gone");
        assert_eq!(
            branch_head_sha(&workspace, BRANCH).await,
            None,
            "the discarded branch must be gone from the workspace git"
        );
    }

    /// The structural half, in the style of ADR 0035's
    /// `the_completion_path_removes_only_the_two_worktrees_it_is_allowed_to`. A
    /// behavioural test shows only that today's paths keep the tree. It cannot
    /// stop the next reader putting a removal back on the Apply or Stop arm.
    /// There it would be a teardown reclaiming a worktree.
    #[test]
    fn the_stale_settle_removes_a_worktree_only_on_discard() {
        const RECOVERY_SRC: &str = include_str!("../agent_recovery/recovery.rs");
        const REMOVE_CALL: &str = r#"&["worktree", "remove", "--force","#;

        let calls: Vec<usize> = RECOVERY_SRC
            .match_indices(REMOVE_CALL)
            .map(|m| m.0)
            .collect();
        assert_eq!(
            calls.len(),
            1,
            "recovery.rs may remove exactly one worktree, found {}. The allowed one is the \
             session's own tree on an explicit user Discard, which git requires before the \
             branch delete. Anything else is a session teardown reclaiming a worktree, which \
             has one owner: the WorktreeCleanup worker (ADR 0035).",
            calls.len()
        );

        let helper = RECOVERY_SRC
            .find("async fn remove_discarded_stale_worktree(")
            .expect("the Discard removal must stay in remove_discarded_stale_worktree");
        assert!(
            calls[0] > helper,
            "the removal must sit inside remove_discarded_stale_worktree"
        );
    }
}
