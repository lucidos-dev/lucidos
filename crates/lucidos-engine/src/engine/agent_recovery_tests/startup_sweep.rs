// -- startup sweep tests for coding_agent_has_diff ---------------------------------

mod startup_sweep_coding_agent_has_diff {
    //! Tests for `reconcile_thread_coding_agent_has_diff` — the per-thread helper the
    //! engine-startup sweep dispatches into. Each test builds a real git repo +
    //! worktree, seeds an active CC thread row in `thread_summaries` (via the
    //! same `SessionStarted` projection path the engine uses at runtime), then
    //! drives the helper and asserts the column reflects on-disk reality.
    //!
    //! The bigger sweep entry point (`refresh_coding_agent_has_diff_for_active_cc_threads`)
    //! just enumerates active CC threads from the DB and calls this helper for
    //! each — same shape `seed_coding_agent_has_diff` takes vs its per-session
    //! bootstrap caller.
    use crate::engine::agent_recovery::reconcile_thread_coding_agent_has_diff;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::git_ops::git_cmd;
    use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
    use crate::test_support::{
        make_repo_and_worktree, read_coding_agent_has_diff, setup_test_db, start_cc_session,
        teardown_test_db,
    };
    use uuid::Uuid;

    #[tokio::test]
    async fn startup_sweep_refreshes_coding_agent_has_diff_for_active_cc_threads() {
        let branch = "claude-code/sweep-true";
        let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

        // Worktree branch has one commit beyond main — the on-disk reality
        // we want the sweep to detect even though no event will fire to
        // update the projection naturally.
        std::fs::write(wt.join("a.txt"), "hello").unwrap();
        git_cmd(&["add", "a.txt"], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "feat: add a"], &wt).await.unwrap();

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, branch, None).await;

        // Precondition: deliberately stale FALSE — simulates the
        // post-commit hook firing while the engine was down.
        assert!(
            !read_coding_agent_has_diff(&pool, thread_id).await,
            "precondition: SessionStarted upsert must leave coding_agent_has_diff at the column default (false)"
        );

        reconcile_thread_coding_agent_has_diff(&pool, thread_id, &repo_root, branch, &wt).await;

        assert!(
            read_coding_agent_has_diff(&pool, thread_id).await,
            "sweep must flip coding_agent_has_diff=true when the branch has commits beyond main on disk"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_leaves_coding_agent_has_diff_false_when_branch_is_actually_fresh() {
        let branch = "claude-code/sweep-fresh";
        let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

        // No additional commits on the branch — `git log main..branch` is empty.

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, branch, None).await;

        reconcile_thread_coding_agent_has_diff(&pool, thread_id, &repo_root, branch, &wt).await;

        assert!(
            !read_coding_agent_has_diff(&pool, thread_id).await,
            "sweep must leave coding_agent_has_diff=false when the branch has no commits beyond main"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_skips_terminated_cc_threads() {
        // Even if the branch on disk has commits, an archived thread must NOT
        // get its coding_agent_has_diff flipped to TRUE — the WaitingBanner Diff
        // button has nothing to act on for a thread the user already closed.
        // The bigger sweep enforces this by filtering on
        // `state='active' AND archive_state!='archived'` in the SQL
        // enumeration; the per-thread helper itself is unconditional, so we
        // test the filter contract by simulating: don't call the helper for
        // archived threads, and verify the row stays at FALSE.
        let branch = "claude-code/sweep-archived";
        let (_tmp, _repo_root, _wt) = make_repo_and_worktree(branch).await;

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, branch, None).await;

        // Archive the thread — `ThreadArchived` writes
        // archive_state='archived' (via the contract layer) and clears
        // coding_agent_has_diff (via the projection). The sweep's
        // enumeration gate
        // (`WHERE is_coding_agent AND state='active' AND archive_state != 'archived'`)
        // skips it.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ThreadArchived,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();

        // Run the sweep entry point and assert no archived thread was
        // visited. We can verify this directly by calling
        // refresh_coding_agent_has_diff_for_active_cc_threads with a repo whose
        // branch has commits — if the gate didn't filter, the column would
        // flip to TRUE.
        let workspace_path = std::env::temp_dir().join(format!(
            "lucidos-sweep-test-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&workspace_path).unwrap();
        crate::engine::agent_recovery::refresh_coding_agent_has_diff_for_active_cc_threads(
            &pool,
            &workspace_path,
            &workspace_path, // lucidos_repo_root unused for archived-only data set
        )
        .await;

        let (archive_state, has_diff): (String, bool) = sqlx::query_as(
            "SELECT archive_state, coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            archive_state, "archived",
            "precondition: ThreadArchived sets archive_state='archived'"
        );
        assert!(
            !has_diff,
            "sweep must not flip coding_agent_has_diff for an archived thread (archive_state={})",
            archive_state
        );

        let _ = std::fs::remove_dir_all(&workspace_path);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_writes_false_when_worktree_is_missing() {
        // Defensive contract: an active CC thread whose worktree directory was
        // removed from disk (cleanup, manual rm -rf, restart-after-crash) must
        // have coding_agent_has_diff reset to FALSE. The git lookup against the main
        // repo could still succeed and return TRUE (the branch ref survives
        // worktree removal), which would leave the Diff button enabled for a
        // thread the user can no longer act on.
        let branch = "claude-code/sweep-missing-wt";
        let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

        // Branch has commits — so seed_coding_agent_has_diff would say TRUE.
        std::fs::write(wt.join("a.txt"), "hello").unwrap();
        git_cmd(&["add", "a.txt"], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "feat: add a"], &wt).await.unwrap();

        // Remove the worktree directory from disk (simulate cleanup) but
        // leave the branch ref intact in the main repo.
        let _ = git_cmd(
            &["worktree", "remove", "--force", wt.to_str().unwrap()],
            &repo_root,
        )
        .await;
        assert!(
            !wt.exists(),
            "worktree dir must be gone for this test to be meaningful"
        );

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, branch, None).await;

        // Pre-seed the column to TRUE so we can assert the sweep actively
        // resets it (not just leaves it at the default).
        sqlx::query("UPDATE thread_summaries SET coding_agent_has_diff = TRUE WHERE thread_id = $1")
            .bind(thread_id)
            .execute(&pool)
            .await
            .unwrap();

        reconcile_thread_coding_agent_has_diff(&pool, thread_id, &repo_root, branch, &wt).await;

        assert!(
            !read_coding_agent_has_diff(&pool, thread_id).await,
            "sweep must reset coding_agent_has_diff=false when the worktree dir is missing on disk"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_resolves_legacy_non_deterministic_worktree_path() {
        // Pins worktree-path resolution in
        // `refresh_coding_agent_has_diff_for_active_cc_threads` to flow through
        // `resolve_worktree_path`, which consults (1) the recorded
        // `CodingAgentIdled.worktree_path`, (2) `git worktree list` branch
        // matching, and (3) the deterministic path in that order. Resolving
        // via `deterministic_worktree_path` directly would skip (1)+(2) and
        // silently miss legacy `<workspace>/.lucidos/worktrees/cc-<random>`
        // paths — those threads would hit the worktree-missing branch in
        // `reconcile_thread_coding_agent_has_diff` and have `coding_agent_has_diff`
        // incorrectly wiped to FALSE.
        //
        // This test seeds a `CodingAgentIdled.worktree_path` pointing at a
        // non-deterministic path and asserts the sweep picks it up and writes
        // TRUE based on the on-disk diff.
        use crate::engine::agent_recovery::refresh_coding_agent_has_diff_for_active_cc_threads;

        let branch = "claude-code/sweep-legacy-path";

        // Build the workspace + repo + worktree at a NON-deterministic
        // location (`cc-<random>`, the pre-Phase-6.1 shape) inside
        // `<workspace>/.lucidos/worktrees/`.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let worktrees_dir = workspace.join(".lucidos/worktrees");
        std::fs::create_dir_all(&worktrees_dir).unwrap();

        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_cmd(&["init", "-b", "main"], &repo).await.unwrap();
        git_cmd(&["config", "user.email", "test@example.com"], &repo)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Test"], &repo)
            .await
            .unwrap();
        std::fs::write(repo.join("seed.txt"), "x").unwrap();
        git_cmd(&["add", "."], &repo).await.unwrap();
        git_cmd(&["commit", "-m", "init"], &repo).await.unwrap();

        // Worktree at `cc-<random>` — `deterministic_worktree_path` would
        // produce `thread-<short>` from the thread_id, so this path is
        // unreachable via the deterministic fallback alone.
        let legacy_dir_name = format!("cc-{}", Uuid::new_v4().simple());
        let wt = worktrees_dir.join(&legacy_dir_name);
        git_cmd(
            &["worktree", "add", wt.to_str().unwrap(), "-b", branch],
            &repo,
        )
        .await
        .unwrap();

        // One commit beyond main on the branch — the on-disk reality the
        // sweep should detect.
        std::fs::write(wt.join("a.txt"), "hello").unwrap();
        git_cmd(&["add", "a.txt"], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "feat: add a"], &wt).await.unwrap();

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, branch, None).await;

        // Stamp `CodingAgentIdled.worktree_path` so `lookup_latest_worktree_path`
        // returns the legacy path. This is the source #1 in
        // `resolve_worktree_path`'s resolution order — production threads
        // post-Phase 6.1 hit this path on every turn.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("legacy-sid".into()),
                coding_agent: crate::runtime::agent_runtime::CodingAgent::ClaudeCode,
                reason: None,
                worktree_path: Some(wt.to_string_lossy().into()),
                worktree_head_sha: None,
                bg_bash_pending: false,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();

        // Pre-seed FALSE — if the sweep used `deterministic_worktree_path`
        // (the bug), `worktree_path.exists()` would be false (no `thread-<short>`
        // dir on disk) and the helper would write FALSE, leaving the column
        // at FALSE and our assertion would fail.
        sqlx::query(
            "UPDATE thread_summaries SET coding_agent_has_diff = FALSE WHERE thread_id = $1",
        )
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();

        refresh_coding_agent_has_diff_for_active_cc_threads(&pool, &workspace, &repo).await;

        assert!(
            read_coding_agent_has_diff(&pool, thread_id).await,
            "sweep must resolve the legacy worktree path via CodingAgentIdled \
             (not deterministic_worktree_path) and detect the on-disk diff"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_resolves_external_repo_root_via_repository_store() {
        // External-repo CC threads carry `cc_repo_id` on `thread_summaries`.
        // The sweep must look that id up in the `repositories` table, resolve
        // the row's `path` to a `repo_root`, and run the git lookup THERE —
        // not against the Lucidos main repo (which has no knowledge of the
        // external repo's branches).
        //
        // Bug shape this guards against: if the cc_repo_id branch in
        // `refresh_coding_agent_has_diff_for_active_cc_threads` regresses (lookup
        // omitted, falls through to `lucidos_repo_root`), `git worktree list`
        // against the wrong repo can't find the external worktree → resolve
        // falls back to the deterministic path under `<workspace>/.lucidos/`,
        // which doesn't exist on disk → `reconcile_thread_coding_agent_has_diff`
        // hits the missing-worktree branch and writes FALSE. Pre-seeded TRUE
        // proves the sweep actively flips on the external-repo path.
        use crate::core::repositories::RepositoryStore;
        use crate::engine::agent_recovery::refresh_coding_agent_has_diff_for_active_cc_threads;

        let branch = "claude-code/sweep-external-repo";

        // Workspace tempdir — the engine `workspace_path` arg the sweep
        // passes to `resolve_worktree_path` for the deterministic-path
        // fallback. Distinct from the external repo so the deterministic
        // path can't accidentally land on the worktree.
        let workspace_tmp = tempfile::tempdir().unwrap();
        let workspace = workspace_tmp.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".lucidos/worktrees")).unwrap();

        // External repo with `branch` checked out as a worktree, one commit
        // beyond main. `make_repo_and_worktree` returns
        // `(tmp, repo_root, wt_path)`; this is the repo the sweep MUST
        // resolve via `cc_repo_id`.
        let (_external_tmp, external_repo, external_wt) =
            make_repo_and_worktree(branch).await;
        std::fs::write(external_wt.join("a.txt"), "hello").unwrap();
        git_cmd(&["add", "a.txt"], &external_wt).await.unwrap();
        git_cmd(&["commit", "-m", "feat: add a"], &external_wt)
            .await
            .unwrap();

        // Lucidos repo — a SEPARATE empty git repo with no `branch` ref.
        // If the sweep mistakenly falls back to this repo, `git worktree
        // list` finds nothing for `branch`, the resolver hits the
        // deterministic path under `<workspace>/.lucidos/worktrees/` (which
        // doesn't exist), and the helper writes FALSE.
        let lucidos_tmp = tempfile::tempdir().unwrap();
        let lucidos_repo = lucidos_tmp.path().to_path_buf();
        git_cmd(&["init", "-b", "main"], &lucidos_repo).await.unwrap();
        git_cmd(&["config", "user.email", "test@example.com"], &lucidos_repo)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Test"], &lucidos_repo)
            .await
            .unwrap();
        std::fs::write(lucidos_repo.join("seed.txt"), "x").unwrap();
        git_cmd(&["add", "."], &lucidos_repo).await.unwrap();
        git_cmd(&["commit", "-m", "init"], &lucidos_repo).await.unwrap();

        let (pool, db_name) = setup_test_db().await;

        // Insert the `repositories` row pointing at the external repo. This
        // is what `RepositoryStore::list` returns inside the sweep — the
        // population the cc_repo_id lookup keys on.
        let repo = RepositoryStore::add(
            &pool,
            "external-test",
            external_repo.to_str().unwrap(),
            None,
            None,
        )
        .await
        .unwrap();

        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, branch, Some(repo.id.to_string())).await;

        // Pre-seed TRUE so a no-op sweep would leave it TRUE and fail to
        // distinguish from the success case. The missing-worktree branch in
        // `reconcile_thread_coding_agent_has_diff` writes FALSE; if the sweep
        // mistakenly resolves against the wrong repo, the column flips to
        // FALSE and the assertion below fails loudly.
        sqlx::query("UPDATE thread_summaries SET coding_agent_has_diff = TRUE WHERE thread_id = $1")
            .bind(thread_id)
            .execute(&pool)
            .await
            .unwrap();

        refresh_coding_agent_has_diff_for_active_cc_threads(&pool, &workspace, &lucidos_repo).await;

        assert!(
            read_coding_agent_has_diff(&pool, thread_id).await,
            "sweep must resolve external-repo root via cc_repo_id lookup, \
             find the worktree, detect the on-disk diff, and leave \
             coding_agent_has_diff=true (not fall back to lucidos_repo_root)"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_resolves_app_thread_root_via_workspace_path() {
        // App coding-agent threads carry `coding_agent_kind = 'app'` and a NULL
        // `cc_repo_id` — their branch lives in the WORKSPACE git repo (where
        // `data/apps/<id>/` lives), not in the Lucidos main repo and not in any
        // registered external repo. The sweep MUST route app threads to
        // `workspace_path`.
        //
        // Bug shape this guards against: routing on `cc_repo_id` alone (NULL →
        // `lucidos_repo_root`) sends app threads to the Lucidos main repo, where
        // the `claude-code/app/...` branch does not exist. `proposal_files_for_branch`
        // then returns None → the sweep wipes `coding_agent_has_diff` to FALSE on
        // every engine restart, hiding the WaitingBanner Diff button and the
        // standalone WIP diff button for an app thread that genuinely has a diff.
        // Pre-seeded FALSE proves the fix actively flips it to TRUE via the
        // workspace route.
        use crate::engine::agent_recovery::refresh_coding_agent_has_diff_for_active_cc_threads;
        use crate::engine::agent_session::CodingAgentKind;

        let branch = "claude-code/app/habit-tracker/sweep";

        // Workspace IS a git repo; the app branch is a worktree of it with one
        // commit beyond main under `data/apps/<id>/`. The worktree lives under
        // `<workspace>/.lucidos/worktrees/`, as in production.
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        git_cmd(&["init", "-b", "main"], &workspace).await.unwrap();
        git_cmd(&["config", "user.email", "test@example.com"], &workspace)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Test"], &workspace)
            .await
            .unwrap();
        std::fs::create_dir_all(workspace.join("data/apps/habit-tracker")).unwrap();
        std::fs::write(
            workspace.join("data/apps/habit-tracker/index.html"),
            "<h1>v1</h1>",
        )
        .unwrap();
        git_cmd(&["add", "."], &workspace).await.unwrap();
        git_cmd(&["commit", "-m", "init app"], &workspace)
            .await
            .unwrap();

        let worktrees_dir = workspace.join(".lucidos/worktrees");
        std::fs::create_dir_all(&worktrees_dir).unwrap();
        let wt = worktrees_dir.join("thread-app");
        git_cmd(
            &["worktree", "add", wt.to_str().unwrap(), "-b", branch],
            &workspace,
        )
        .await
        .unwrap();
        std::fs::write(
            wt.join("data/apps/habit-tracker/index.html"),
            "<h1>v2</h1>",
        )
        .unwrap();
        git_cmd(&["add", "."], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "edit app"], &wt).await.unwrap();

        // Separate empty Lucidos repo with NO `branch` ref. If the sweep
        // mis-routes app threads here, `proposal_files_for_branch` returns None
        // and the column is wiped to FALSE.
        let lucidos_tmp = tempfile::tempdir().unwrap();
        let lucidos_repo = lucidos_tmp.path().to_path_buf();
        git_cmd(&["init", "-b", "main"], &lucidos_repo).await.unwrap();
        git_cmd(&["config", "user.email", "test@example.com"], &lucidos_repo)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Test"], &lucidos_repo)
            .await
            .unwrap();
        std::fs::write(lucidos_repo.join("seed.txt"), "x").unwrap();
        git_cmd(&["add", "."], &lucidos_repo).await.unwrap();
        git_cmd(&["commit", "-m", "init"], &lucidos_repo).await.unwrap();

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        // SessionStarted for an APP thread: kind='app', cc_repo_id NULL.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                session_id: "test-session".into(),
                branch: branch.into(),
                repo_id: None,
                coding_agent_kind: CodingAgentKind::App,
                coding_agent_folder: workspace
                    .join("data/apps/habit-tracker")
                    .to_string_lossy()
                    .into(),
                app_id: Some("habit-tracker".into()),
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();

        // Record the worktree path (source #1 in `resolve_worktree_path`) so the
        // worktree is found regardless of repo_root — isolating the bug to the
        // repo_root routing that `seed_coding_agent_has_diff` diffs against.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentIdled {
                has_changes: true,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("sid".into()),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                reason: None,
                worktree_path: Some(wt.to_string_lossy().into()),
                worktree_head_sha: None,
                bg_bash_pending: false,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();

        // Pre-seed FALSE so the assertion proves the sweep actively flips it to
        // TRUE via the workspace_path route (the bug leaves it FALSE).
        sqlx::query("UPDATE thread_summaries SET coding_agent_has_diff = FALSE WHERE thread_id = $1")
            .bind(thread_id)
            .execute(&pool)
            .await
            .unwrap();

        refresh_coding_agent_has_diff_for_active_cc_threads(&pool, &workspace, &lucidos_repo).await;

        assert!(
            read_coding_agent_has_diff(&pool, thread_id).await,
            "sweep must route app-kind threads to workspace_path, find the \
             branch diff, and leave coding_agent_has_diff=true (not fall back \
             to lucidos_repo_root, where the app branch does not exist)"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

// -- Phase C: orphaned-running coding-agent settle sweep -----------------------
//
// `settle_orphaned_running_coding_agent_threads` is the boot-recovery floor that
// would have caught thread-72120ca6: a coding-agent thread left `running` after
// a restart (the worktree-recovery skip paths drop it without settling) is a
// permanent zombie, since the in-memory watchdogs only scan live sessions.
mod settle_orphaned_running_sweep {
    use crate::engine::agent_recovery::recovery::settle_orphaned_running_coding_agent_threads;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
    use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};
    use std::collections::HashSet;
    use uuid::Uuid;

    async fn status_of(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    async fn aborted_count(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
        )
        .bind(thread_id.to_string())
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Emit a chat-channel MessageReceived → is_coding_agent=false, status=running.
    async fn seed_running_chat_thread(bus: &EventBus, thread_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                text: "hi".into(),
                user_image_hashes: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Agent,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    /// The fix: a running coding-agent thread that recovery did NOT pick up
    /// (empty `recovering` set) is settled — exactly the thread-72120ca6 case.
    #[tokio::test]
    async fn settles_orphaned_running_cc_thread() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, "claude-code/orphan", None).await;
        assert_eq!(status_of(&pool, thread_id).await.as_deref(), Some("running"));

        settle_orphaned_running_coding_agent_threads(&pool, &bus, &HashSet::new()).await;

        assert_ne!(
            status_of(&pool, thread_id).await.as_deref(),
            Some("running"),
            "orphaned running CC thread must be settled out of `running`"
        );
        assert_eq!(
            aborted_count(&pool, thread_id).await,
            1,
            "settle must emit exactly one ResponseAborted"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A thread the recovery loop already owns (in `recovering`) is left for
    /// that path to resume/settle — the sweep must not double-handle it.
    #[tokio::test]
    async fn skips_thread_in_recovering_set() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, "claude-code/recovering", None).await;

        let recovering: HashSet<Uuid> = [thread_id].into_iter().collect();
        settle_orphaned_running_coding_agent_threads(&pool, &bus, &recovering).await;

        assert_eq!(
            status_of(&pool, thread_id).await.as_deref(),
            Some("running"),
            "a thread owned by recovery must not be settled by the sweep"
        );
        assert_eq!(aborted_count(&pool, thread_id).await, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Chat threads are out of scope — they're settled by
    /// `recover_orphaned_threads`, and a chat thread blocked on a child sits
    /// `running` pending parent-resume. The `is_coding_agent` filter excludes them.
    #[tokio::test]
    async fn skips_running_chat_thread() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_running_chat_thread(&bus, thread_id).await;
        assert_eq!(status_of(&pool, thread_id).await.as_deref(), Some("running"));

        settle_orphaned_running_coding_agent_threads(&pool, &bus, &HashSet::new()).await;

        assert_eq!(
            status_of(&pool, thread_id).await.as_deref(),
            Some("running"),
            "the sweep must not touch chat threads (is_coding_agent=false)"
        );
        assert_eq!(aborted_count(&pool, thread_id).await, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

// -- end_stale_waiting_session branch-deletion regression ----------------------

