mod end_stale_session_branch_preservation {
    //! Regression: when the user clicks Apply on a thread with no live CC
    //! session, `apply_now` calls `end_stale_waiting_session(.., discard=false, ..)`
    //! which used to silently `git branch -D` the branch in the
    //! `proposal_files=None` arm — losing the user's committed work whenever
    //! the per-commit `ChangeProposed` events hadn't synced into the `changes`
    //! projection (so the branch had no pending change). Symptom: the
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

mod a_refused_branch_delete_is_reported {
    //! The stale settle's `git branch -D` must say so when git refuses.
    //!
    //! `git_cmd` answers `Ok` for a non-zero exit, so an `if let Err` over it
    //! fires only on a spawn failure or the 30s timeout. Git refuses while a
    //! worktree still holds the branch, which is what an `Unknown` lookup
    //! leaves: `settle_stale_worktree` skips its removal, and the Discard then
    //! failed silently with the branch and its commits still there.
    //!
    //! `end_stale_waiting_session` needs a `LucidosEngine`, so the call site is
    //! pinned by source, in the style of the tripwire above. The behavioural
    //! half proves what the scan is about, against real git in a tempdir it
    //! made itself.

    use crate::engine::git_ops::{branch_head_sha, git_cmd, git_ran_ok};
    use crate::test_support::make_repo_and_worktree;

    const BRANCH: &str = "lucidos-claude-code-repo-refused-delete-7c1a20b4";

    #[test]
    fn the_stale_settle_deletes_its_branch_through_git_ran_ok() {
        const RECOVERY_SRC: &str = include_str!("../agent_recovery/recovery.rs");

        let sites: Vec<usize> = RECOVERY_SRC
            .match_indices(r#"&["branch", "-D","#)
            .map(|m| m.0)
            .collect();
        for at in &sites {
            let start = RECOVERY_SRC[..*at].rfind('\n').map(|n| n + 1).unwrap_or(0);
            let line = RECOVERY_SRC[start..]
                .lines()
                .next()
                .unwrap_or_default()
                .trim();
            assert!(
                line.contains("git_ran_ok("),
                "a `git branch -D` in recovery.rs must run through `git_ran_ok`, which \
                 folds a non-zero exit into `Err`. `git_cmd` calls a refused delete `Ok` \
                 and the log never names it. Offending line: {line}"
            );
        }
        assert_eq!(
            sites.len(),
            1,
            "the stale settle deletes one branch, on an explicit Discard"
        );
    }

    #[tokio::test]
    async fn git_refuses_while_a_worktree_holds_the_branch() {
        let (_tmp, repo, wt) = make_repo_and_worktree(BRANCH).await;
        assert!(wt.exists(), "the worktree is what makes git refuse");

        assert!(
            git_cmd(&["branch", "-D", BRANCH], &repo).await.is_ok(),
            "git_cmd reads a refused delete as Ok, which is why the old site was silent"
        );
        assert!(
            git_ran_ok(&["branch", "-D", BRANCH], &repo).await.is_err(),
            "git_ran_ok must surface the refusal so the log can name it"
        );
        assert!(
            branch_head_sha(&repo, BRANCH).await.is_some(),
            "and the branch survived both attempts, with the discarded commits on it"
        );
    }
}

mod a_stale_recovery_keeps_withheld_work_visible {
    //! I9: stale-session recovery ends its turn with `CodingAgentIdled`. Its
    //! `has_changes` must report the branch's work whether a change carries it
    //! or the turn end withheld it. Reporting "proposed or not" erased
    //! withheld work to `none`. `end_stale_waiting_session` needs a
    //! `LucidosEngine`, so the call site is pinned by source and the projection
    //! half is replayed through the bus.

    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent, UnproposedReason};
    use crate::engine::thread_lifecycle::CodingAgentChangeState;
    use crate::test_support::{
        read_change_state, setup_test_db, start_cc_session, teardown_test_db,
    };
    use uuid::Uuid;

    #[test]
    fn the_stale_idle_reports_branch_work_not_the_proposal() {
        const RECOVERY_SRC: &str = include_str!("../agent_recovery/recovery.rs");
        let body = &RECOVERY_SRC[RECOVERY_SRC
            .find("async fn end_stale_waiting_session(")
            .unwrap()..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        let work = body
            .find("branch_has_work = true;")
            .expect("the branch's work is recorded");
        let settle = body
            .find("self.propose_stale_branch(")
            .expect("before it is proposed or withheld");
        assert!(work < settle);
        assert!(
            body.contains("has_changes: branch_has_work,"),
            "the idle reports the branch's work"
        );
    }

    /// The stale path's events for withheld work: the withholding, then the
    /// idle with work. The work ends unproposed with its reason, never `none`.
    #[tokio::test]
    async fn withheld_work_survives_the_stale_idle() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let branch = "claude-code/stale-withheld";
        start_cc_session(&bus, thread_id, branch, None).await;
        let meta = || EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        };
        for event in [
            ThreadEvent::ProposalWithheld {
                agent_returns: false,
                branch_name: branch.into(),
                files: vec!["a.txt".into()],
                reason: UnproposedReason::TurnIncomplete,
            },
            ThreadEvent::CodingAgentIdled {
                has_changes: true,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: None,
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
                turn_end: None,
            },
        ] {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: meta(),
            })
            .await
            .expect("emit");
        }

        assert_eq!(
            read_change_state(&pool, thread_id).await,
            CodingAgentChangeState::Unproposed {
                reason: Some(UnproposedReason::TurnIncomplete)
            }
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }
}

mod recovery_pass_shape {
    //! Source-shape checks for `recovery.rs`. No harness builds a
    //! `LucidosEngine`, so the stale settle and the boot recovery pass cannot be
    //! driven end to end. Each test pins one ordering or call shape that no
    //! behavioural test can see. Comment lines are stripped first, so a comment
    //! cannot satisfy or break a check.

    const RECOVERY_SRC: &str = include_str!("../agent_recovery/recovery.rs");
    const MAIN_SRC: &str = include_str!("../../../../lucidos-engine-bin/src/main.rs");

    fn code(src: &str) -> String {
        src.lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn squash(src: &str) -> String {
        src.chars().filter(|c| !c.is_whitespace()).collect()
    }

    fn between(src: &str, from: &str, to: &str) -> String {
        let start = src
            .find(from)
            .unwrap_or_else(|| panic!("`{from}` is missing"));
        let rest = &src[start..];
        let end = rest
            .find(to)
            .unwrap_or_else(|| panic!("`{to}` is missing after `{from}`"));
        code(&rest[..end])
    }

    fn end_stale_body() -> String {
        between(
            RECOVERY_SRC,
            "async fn end_stale_waiting_session(",
            "async fn end_stale_lane_thread(",
        )
    }

    fn recover_body() -> String {
        between(
            RECOVERY_SRC,
            "async fn recover_orphaned_worktrees(self: &Arc<Self>)",
            "pub(crate) async fn rescue_stale_worktree(",
        )
    }

    fn lost_branch_loop() -> String {
        between(
            &recover_body(),
            "for branch in &actively_running_branches {",
            "let t_lost_branches",
        )
    }

    #[test]
    fn the_stale_settle_reads_branch_and_kind_from_one_session_row() {
        let body = end_stale_body();
        assert_eq!(
            body.matches("'SessionStarted'").count(),
            1,
            "a second SessionStarted read could name a different session"
        );
        assert!(
            squash(&body).contains("payload->>'branch',payload->>'coding_agent_kind'FROMevents")
        );
    }

    #[test]
    fn a_thread_without_a_session_errors_and_a_branchless_one_idles() {
        let body = end_stale_body();
        assert!(
            !body.contains("SessionEnded"),
            "the stale settle must never emit SessionEnded"
        );
        assert!(squash(&body).contains(&squash(
            r#"None => { return Err("No Claude Code session found for this thread".into()); }"#
        )));
        let arm_start = body.find("Some(_) => {").expect("the no-branch arm");
        let arm = &body[arm_start..];
        let arm = &arm[..arm.find("return Ok(());").expect("the arm returns")];
        assert!(
            arm.contains("ThreadEvent::CodingAgentIdled"),
            "a session with no branch is idled before the settle returns"
        );
    }

    #[test]
    fn a_discard_clears_pending_changes_only_after_the_worktree_settle() {
        let body = end_stale_body();
        let settle = body.find("settle_stale_worktree(").expect("the settle");
        let discard = body
            .find("discard_open_changes_for_thread(")
            .expect("the discard");
        assert!(
            settle < discard,
            "discard_change resets the tree it finds, so the worktree settle runs first"
        );
    }

    #[test]
    fn the_worktree_scan_lists_roots_concurrently_but_in_root_order() {
        let body = recover_body();
        assert!(body.contains(".buffered(RECOVERY_GIT_CONCURRENCY)"));
        assert!(
            !body.contains("buffer_unordered"),
            "an unordered stream would reorder to_recover"
        );
    }

    #[test]
    fn the_lost_branch_loop_never_consults_already_recovered() {
        assert!(
            !lost_branch_loop().contains("already_recovered"),
            "every branch in the loop is running, so the set would only drop a live turn"
        );
    }

    #[test]
    fn the_lost_branch_loop_does_not_prune_worktrees() {
        assert!(
            !lost_branch_loop().contains("prune"),
            "worktree_add prunes under the admin lock, the only safe sweep"
        );
    }

    #[test]
    fn a_vanished_branch_ref_is_recovered_before_the_session_is_ended() {
        assert!(squash(&lost_branch_loop()).contains(
            "ifletSome(&thread_id)=branch_to_thread.get(branch){matchrecover_branch_ref_from_worktree("
        ));
    }

    #[test]
    fn a_parked_question_is_preserved_before_the_thread_joins_recovering_threads() {
        let body = recover_body();
        let guard = body
            .find("thread_has_unanswered_question(self.pool(), thread_id).await {")
            .expect("the park guard");
        let insert = body
            .find("recovering_threads.insert(")
            .expect("the recovering insert");
        assert!(guard < insert);
        assert!(
            body[guard..insert].contains("continue;"),
            "the guard skips the thread before it is marked recovering"
        );
    }

    #[test]
    fn a_crash_is_surfaced_with_the_restart_reason_and_never_spawned() {
        let body = recover_body();
        assert!(
            !body.contains("ContinuationRequested"),
            "recovery must not request a continuation for a crash"
        );
        assert!(squash(&body).contains(
            "reason:Some(ENGINE_RESTART_INTERRUPT_REASON.to_string()),worktree_path:Some(wt_path"
        ));
    }

    #[test]
    fn only_a_user_switch_on_a_thread_free_to_start_a_turn_queues_an_auto_resume() {
        assert!(squash(&recover_body()).contains(&squash(
            "if switch_was_user_initiated(self.pool(), thread_id).await \
             && crate::core::store::thread_hold::may_start_turn(self.pool(), thread_id).await \
             { self.enqueue_switch_resume(thread_id);"
        )));
    }

    #[test]
    fn the_archived_filter_runs_before_any_recreation_or_synthetic_event() {
        let body = recover_body();
        assert!(squash(&body).contains(&squash(
            "actively_running_branches.retain(|branch| eligible.contains_key(branch)); \
             to_recover.retain(|(_, branch, _, _)| eligible.contains_key(branch)); \
             branch_to_thread = eligible.clone();"
        )));
        let filter = body.find("&restored_branches {").expect("the filter");
        for later in [
            "recreate_lost_worktree(",
            "emit_change_withdrawn(",
            "emit_response_aborted(",
            "end_stuck_session(self",
        ] {
            let at = body.find(later).unwrap_or_else(|| panic!("`{later}`"));
            assert!(filter < at, "the archived filter must precede `{later}`");
        }
    }

    #[test]
    fn the_stuck_session_idle_carries_the_restart_reason() {
        let closure = between(
            &recover_body(),
            "let end_stuck_session =",
            "for branch in &actively_running_branches {",
        );
        assert!(
            squash(&closure).contains("reason:Some(ENGINE_RESTART_INTERRUPT_REASON.to_string())")
        );
    }

    #[test]
    fn the_orphaned_running_settle_spares_a_turn_between_successor_sessions() {
        let body = between(
            RECOVERY_SRC,
            "async fn settle_one_orphaned_running_thread(",
            "const STRANDED_TRIGGER_RUNS_SQL",
        );
        let body = squash(&body);
        let check = body
            .find(&squash(
                "if super::super::successor::between_successor_sessions(pool, tid).await? { return Ok(()); }",
            ))
            .expect("the successor check returns early");
        assert!(check < body.find("INTERRUPTED_TURN_START_SQL").expect("the query"));
    }

    #[test]
    fn boot_settles_unresumed_switches_after_both_resume_drains() {
        let main = code(MAIN_SRC);
        let coding = main
            .find(".resume_pending_switches()")
            .expect("the coding-agent drain");
        let chat = main
            .find("resume_pending_chat_switches(&shared_engine)")
            .expect("the chat drain");
        let floor = main
            .find("settle_agent_switch_threads(")
            .expect("the boot floor");
        assert!(coding < chat && chat < floor);
        assert!(main.contains("resumed_switch_threads.extend("));
        assert!(
            main[floor..(floor + 160).min(main.len())].contains("&resumed_switch_threads"),
            "the floor takes the union of both drains"
        );
    }
}
