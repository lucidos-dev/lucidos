mod startup_sweep_branch_work {
    use crate::engine::agent_recovery::reconcile_thread_branch_work;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::git_ops::git_cmd;
    use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
    use crate::engine::thread_lifecycle::{ChangeStateKind, CodingAgentChangeState};
    use crate::test_support::{
        make_repo_and_worktree, read_change_state_kind, seed_change_state, setup_test_db,
        start_cc_session, teardown_test_db,
    };

    const UNPROPOSED: CodingAgentChangeState = CodingAgentChangeState::Unproposed { reason: None };
    use uuid::Uuid;

    #[tokio::test]
    async fn startup_sweep_refreshes_branch_work_for_active_cc_threads() {
        let branch = "claude-code/sweep-true";
        let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

        std::fs::write(wt.join("a.txt"), "hello").unwrap();
        git_cmd(&["add", "a.txt"], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "feat: add a"], &wt)
            .await
            .unwrap();

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, branch, None).await;

        assert_eq!(
            read_change_state_kind(&pool, thread_id).await,
            ChangeStateKind::None,
            "precondition: the SessionStarted upsert leaves the column default"
        );

        reconcile_thread_branch_work(&pool, thread_id, &repo_root, branch, &wt).await;

        assert_eq!(
            read_change_state_kind(&pool, thread_id).await,
            ChangeStateKind::Unproposed,
            "sweep must read commits beyond main on disk as unproposed work"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_leaves_change_state_none_when_branch_is_actually_fresh() {
        let branch = "claude-code/sweep-fresh";
        let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, branch, None).await;

        reconcile_thread_branch_work(&pool, thread_id, &repo_root, branch, &wt).await;

        assert_eq!(
            read_change_state_kind(&pool, thread_id).await,
            ChangeStateKind::None,
            "sweep must leave the state none when the branch has no commits beyond main"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_corrects_a_stale_unproposed_state_when_the_branch_has_no_net_diff() {
        let branch = "claude-code/sweep-stale-true";
        let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, branch, None).await;
        seed_change_state(&pool, thread_id, UNPROPOSED).await;

        reconcile_thread_branch_work(&pool, thread_id, &repo_root, branch, &wt).await;

        assert_eq!(
            read_change_state_kind(&pool, thread_id).await,
            ChangeStateKind::None,
            "an empty net diff must overwrite a stale unproposed state, not leave it"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_skips_terminated_cc_threads() {
        let branch = "claude-code/sweep-archived";
        let (_tmp, _repo_root, _wt) = make_repo_and_worktree(branch).await;

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, branch, None).await;

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ThreadArchived,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let workspace_path = tmp.path().to_path_buf();
        crate::engine::agent_recovery::refresh_branch_work_for_active_cc_threads(
            &pool,
            &workspace_path,
            &workspace_path,
        )
        .await;

        let (archive_state, change_state): (String, String) = sqlx::query_as(
            "SELECT archive_state, coding_agent_change_state FROM thread_summaries \
             WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            archive_state, "archived",
            "precondition: ThreadArchived sets archive_state='archived'"
        );
        assert_eq!(
            change_state, "none",
            "sweep must not mark an archived thread's branch as work (archive_state={archive_state})"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_writes_false_when_worktree_is_missing() {
        let branch = "claude-code/sweep-missing-wt";
        let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

        std::fs::write(wt.join("a.txt"), "hello").unwrap();
        git_cmd(&["add", "a.txt"], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "feat: add a"], &wt)
            .await
            .unwrap();

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

        seed_change_state(&pool, thread_id, UNPROPOSED).await;

        reconcile_thread_branch_work(&pool, thread_id, &repo_root, branch, &wt).await;

        assert_eq!(
            read_change_state_kind(&pool, thread_id).await,
            ChangeStateKind::None,
            "sweep must reset the state to none when the worktree dir is missing on disk"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_resolves_legacy_non_deterministic_worktree_path() {
        use crate::engine::agent_recovery::refresh_branch_work_for_active_cc_threads;

        let branch = "claude-code/sweep-legacy-path";

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

        let legacy_dir_name = format!("cc-{}", Uuid::new_v4().simple());
        let wt = worktrees_dir.join(&legacy_dir_name);
        git_cmd(
            &["worktree", "add", wt.to_str().unwrap(), "-b", branch],
            &repo,
        )
        .await
        .unwrap();

        std::fs::write(wt.join("a.txt"), "hello").unwrap();
        git_cmd(&["add", "a.txt"], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "feat: add a"], &wt)
            .await
            .unwrap();

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, branch, None).await;

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
                turn_end: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();

        seed_change_state(&pool, thread_id, CodingAgentChangeState::None).await;

        refresh_branch_work_for_active_cc_threads(&pool, &workspace, &repo).await;

        assert_eq!(
            read_change_state_kind(&pool, thread_id).await,
            ChangeStateKind::Unproposed,
            "sweep must resolve the legacy worktree path via CodingAgentIdled \
             (not deterministic_worktree_path) and detect the on-disk diff"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_resolves_external_repo_root_via_repository_store() {
        use crate::core::repositories::RepositoryStore;
        use crate::engine::agent_recovery::refresh_branch_work_for_active_cc_threads;

        let branch = "claude-code/sweep-external-repo";

        let workspace_tmp = tempfile::tempdir().unwrap();
        let workspace = workspace_tmp.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".lucidos/worktrees")).unwrap();

        let (_external_tmp, external_repo, external_wt) = make_repo_and_worktree(branch).await;
        std::fs::write(external_wt.join("a.txt"), "hello").unwrap();
        git_cmd(&["add", "a.txt"], &external_wt).await.unwrap();
        git_cmd(&["commit", "-m", "feat: add a"], &external_wt)
            .await
            .unwrap();

        let lucidos_tmp = tempfile::tempdir().unwrap();
        let lucidos_repo = lucidos_tmp.path().to_path_buf();
        git_cmd(&["init", "-b", "main"], &lucidos_repo)
            .await
            .unwrap();
        git_cmd(&["config", "user.email", "test@example.com"], &lucidos_repo)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Test"], &lucidos_repo)
            .await
            .unwrap();
        std::fs::write(lucidos_repo.join("seed.txt"), "x").unwrap();
        git_cmd(&["add", "."], &lucidos_repo).await.unwrap();
        git_cmd(&["commit", "-m", "init"], &lucidos_repo)
            .await
            .unwrap();

        let (pool, db_name) = setup_test_db().await;

        let (bus, _rx) = EventBus::new(pool.clone());

        let repo = RepositoryStore::register(
            &pool,
            &bus,
            "external-test",
            external_repo.to_str().unwrap(),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, branch, Some(repo.id.to_string())).await;

        seed_change_state(&pool, thread_id, UNPROPOSED).await;

        refresh_branch_work_for_active_cc_threads(&pool, &workspace, &lucidos_repo).await;

        assert_eq!(
            read_change_state_kind(&pool, thread_id).await,
            ChangeStateKind::Unproposed,
            "sweep must resolve external-repo root via cc_repo_id lookup, \
             find the worktree, detect the on-disk diff, and leave the state \
             unproposed (not fall back to lucidos_repo_root)"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn startup_sweep_resolves_app_thread_root_via_workspace_path() {
        use crate::engine::agent_recovery::refresh_branch_work_for_active_cc_threads;
        use crate::engine::thread_events::CodingAgentKind;

        let branch = "claude-code/app/habit-tracker/sweep";

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
        std::fs::write(wt.join("data/apps/habit-tracker/index.html"), "<h1>v2</h1>").unwrap();
        git_cmd(&["add", "."], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "edit app"], &wt).await.unwrap();

        let lucidos_tmp = tempfile::tempdir().unwrap();
        let lucidos_repo = lucidos_tmp.path().to_path_buf();
        git_cmd(&["init", "-b", "main"], &lucidos_repo)
            .await
            .unwrap();
        git_cmd(&["config", "user.email", "test@example.com"], &lucidos_repo)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Test"], &lucidos_repo)
            .await
            .unwrap();
        std::fs::write(lucidos_repo.join("seed.txt"), "x").unwrap();
        git_cmd(&["add", "."], &lucidos_repo).await.unwrap();
        git_cmd(&["commit", "-m", "init"], &lucidos_repo)
            .await
            .unwrap();

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                sandbox: None,
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
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();

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
                turn_end: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();

        seed_change_state(&pool, thread_id, CodingAgentChangeState::None).await;

        refresh_branch_work_for_active_cc_threads(&pool, &workspace, &lucidos_repo).await;

        assert_eq!(
            read_change_state_kind(&pool, thread_id).await,
            ChangeStateKind::Unproposed,
            "sweep must route app-kind threads to workspace_path, find the \
             branch diff, and leave the state unproposed (not fall back \
             to lucidos_repo_root, where the app branch does not exist)"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

mod settle_orphaned_running_sweep {
    use crate::engine::agent_recovery::recovery::settle_orphaned_running_threads;
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

    async fn abort_cause(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
        sqlx::query_scalar(
            "SELECT payload->>'cause' FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
        )
        .bind(thread_id.to_string())
        .fetch_optional(pool)
        .await
        .unwrap()
    }

    async fn bare_start(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str) {
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES ($1, $2, '{}'::jsonb, $3, 'thread', $3::text)",
        )
        .bind(Uuid::new_v4())
        .bind(event_type)
        .bind(thread_id)
        .execute(pool)
        .await
        .unwrap();
    }

    fn meta(channel: EventChannel) -> EventMeta {
        EventMeta {
            channel: Some(channel),
            ..EventMeta::NONE
        }
    }

    async fn message(bus: &EventBus, thread_id: Uuid, channel: EventChannel) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "hi".into(),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Agent,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: meta(channel),
        })
        .await
        .unwrap();
    }

    async fn tool_call(bus: &EventBus, thread_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentToolCalled {
                name: "Bash".into(),
                args: serde_json::json!({"command": "ls"}),
                description: String::new(),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                tool_use_id: format!("toolu-{thread_id}"),
                parent_tool_use_id: None,
                api_call_id: None,
            },
            meta: meta(EventChannel::ClaudeCode),
        })
        .await
        .unwrap();
    }

    async fn idled(bus: &EventBus, thread_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: None,
                coding_agent: crate::runtime::agent_runtime::CodingAgent::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
                turn_end: None,
            },
            meta: meta(EventChannel::ClaudeCode),
        })
        .await
        .unwrap();
    }

    async fn force_running(pool: &sqlx::PgPool, thread_id: Uuid) {
        sqlx::query("UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1")
            .bind(thread_id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn died_cc_turn(bus: &EventBus, thread_id: Uuid) {
        start_cc_session(bus, thread_id, "claude-code/orphan", None).await;
        message(bus, thread_id, EventChannel::ClaudeCode).await;
        tool_call(bus, thread_id).await;
    }

    #[tokio::test]
    async fn settles_a_died_turn_with_one_abort() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        died_cc_turn(&bus, thread_id).await;
        assert_eq!(
            status_of(&pool, thread_id).await.as_deref(),
            Some("running")
        );

        settle_orphaned_running_threads(&pool, &bus, &HashSet::new()).await;

        assert_eq!(status_of(&pool, thread_id).await.as_deref(), Some("failed"));
        assert_eq!(aborted_count(&pool, thread_id).await, 1);
        assert_eq!(
            abort_cause(&pool, thread_id).await.as_deref(),
            Some("recovery_after_restart"),
            "a turn the restart killed reads as interrupted, not as a stuck row"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn settles_a_died_chat_turn_with_one_abort() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        message(&bus, thread_id, EventChannel::Chat).await;
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::TextStreamed {
                text: "partial reply".into(),
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();

        settle_orphaned_running_threads(&pool, &bus, &HashSet::new()).await;

        assert_ne!(
            status_of(&pool, thread_id).await.as_deref(),
            Some("running")
        );
        assert_eq!(aborted_count(&pool, thread_id).await, 1);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn idles_an_ended_turn_without_an_abort() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        died_cc_turn(&bus, thread_id).await;
        idled(&bus, thread_id).await;
        force_running(&pool, thread_id).await;

        settle_orphaned_running_threads(&pool, &bus, &HashSet::new()).await;

        assert_eq!(status_of(&pool, thread_id).await.as_deref(), Some("idle"));
        assert_eq!(aborted_count(&pool, thread_id).await, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn settles_a_message_the_crash_dropped_with_one_abort() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        message(&bus, thread_id, EventChannel::Chat).await;

        settle_orphaned_running_threads(&pool, &bus, &HashSet::new()).await;

        assert_eq!(status_of(&pool, thread_id).await.as_deref(), Some("failed"));
        assert_eq!(aborted_count(&pool, thread_id).await, 1);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn idles_a_start_boot_will_run_without_an_abort() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        died_cc_turn(&bus, thread_id).await;
        idled(&bus, thread_id).await;
        bare_start(&pool, thread_id, "ChildThreadCompleted").await;
        force_running(&pool, thread_id).await;

        settle_orphaned_running_threads(&pool, &bus, &HashSet::new()).await;

        assert_eq!(status_of(&pool, thread_id).await.as_deref(), Some("idle"));
        assert_eq!(aborted_count(&pool, thread_id).await, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn an_unstarted_continuation_ends_with_an_abort_at_boot() {
        use crate::engine::agent_recovery::USER_CLICKED_CONTINUE_REASON;
        use crate::engine::agent_session::spawn_dispatcher::{SpawnDispatcher, SpawnRequest};
        use std::sync::Arc;

        let (pool, db_name) = setup_test_db().await;
        let (previous, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        died_cc_turn(&previous, thread_id).await;
        idled(&previous, thread_id).await;
        previous
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ContinuationRequested {
                    reason: USER_CLICKED_CONTINUE_REASON.to_string(),
                },
                meta: meta(EventChannel::ClaudeCode),
            })
            .await
            .unwrap();
        assert_eq!(
            status_of(&pool, thread_id).await.as_deref(),
            Some("running")
        );

        let (bus, _rx) = EventBus::new(pool.clone());
        let bus = Arc::new(bus);
        settle_orphaned_running_threads(&pool, &bus, &HashSet::new()).await;
        let (handle, mut spawn_rx) = SpawnDispatcher::spawn(pool.clone(), bus.clone());

        assert_eq!(
            aborted_count(&pool, thread_id).await,
            1,
            "a request nothing will start ends with an abort, not a bare idle"
        );
        assert_eq!(
            abort_cause(&pool, thread_id).await.as_deref(),
            Some("recovery_after_restart")
        );
        assert_eq!(status_of(&pool, thread_id).await.as_deref(), Some("failed"));

        let barrier = Uuid::new_v4();
        start_cc_session(&bus, barrier, "claude-code/barrier", None).await;
        let barrier_event = bus
            .emit(BusEvent::Thread {
                thread_id: barrier,
                event: ThreadEvent::ContinuationRequested {
                    reason: USER_CLICKED_CONTINUE_REASON.to_string(),
                },
                meta: meta(EventChannel::ClaudeCode),
            })
            .await
            .unwrap()
            .unwrap()
            .event_id;
        let sent = tokio::time::timeout(std::time::Duration::from_secs(5), spawn_rx.recv())
            .await
            .expect("the dispatcher sent nothing")
            .expect("channel open");
        assert_eq!(
            sent,
            SpawnRequest::Continue {
                thread_id: barrier,
                event_id: barrier_event,
            },
            "the settled request must not be driven"
        );

        handle.abort();
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn skips_thread_in_recovering_set() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        died_cc_turn(&bus, thread_id).await;

        let recovering: HashSet<Uuid> = [thread_id].into_iter().collect();
        settle_orphaned_running_threads(&pool, &bus, &recovering).await;

        assert_eq!(
            status_of(&pool, thread_id).await.as_deref(),
            Some("running")
        );
        assert_eq!(aborted_count(&pool, thread_id).await, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn leaves_a_parked_thread_alone() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        died_cc_turn(&bus, thread_id).await;
        sqlx::query(
            "UPDATE thread_summaries SET status = 'waiting_for_user_answer' WHERE thread_id = $1",
        )
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();

        settle_orphaned_running_threads(&pool, &bus, &HashSet::new()).await;

        assert_eq!(
            status_of(&pool, thread_id).await.as_deref(),
            Some("waiting_for_user_answer")
        );
        assert_eq!(aborted_count(&pool, thread_id).await, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

mod settle_stranded_trigger_runs_sweep {
    use crate::engine::agent_recovery::recovery::settle_stranded_trigger_runs;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent, TriggerInvocation};
    use crate::test_support::{setup_test_db, teardown_test_db};
    use uuid::Uuid;

    async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta {
                channel: Some(EventChannel::Trigger),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    async fn stranded_run(pool: &sqlx::PgPool, bus: &EventBus, go_to_review: bool) -> Uuid {
        let thread_id = Uuid::new_v4();
        emit(
            bus,
            thread_id,
            ThreadEvent::TriggerStarted {
                retry_of: None,
                provider: None,
                trigger_id: "t-stranded".into(),
                trigger_name: Some("watch".into()),
                prompt: None,
                invocation: Some(TriggerInvocation::Schedule),
                origin: None,
                go_to_review,
                model: None,
                reasoning_effort: None,
                continue_key: None,
            },
        )
        .await;
        sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE thread_id = $1")
            .bind(thread_id)
            .execute(pool)
            .await
            .unwrap();
        thread_id
    }

    async fn state_of(pool: &sqlx::PgPool, thread_id: Uuid) -> (String, String) {
        sqlx::query_as("SELECT status, archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn aborts(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<(String, bool)> {
        sqlx::query_as(
            "SELECT payload->>'cause', \
                    COALESCE(payload->>'request_event_id' = ( \
                        SELECT id::text FROM events \
                        WHERE aggregate_id = $1 AND event_type = 'TriggerStarted'), FALSE) \
             FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
        )
        .bind(thread_id.to_string())
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn abort_count(pool: &sqlx::PgPool, thread_id: Uuid) -> usize {
        aborts(pool, thread_id).await.len()
    }

    #[tokio::test]
    async fn settles_an_unattended_stranded_run_to_archived() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = stranded_run(&pool, &bus, false).await;
        assert_eq!(
            state_of(&pool, thread_id).await,
            ("idle".into(), "inbox".into())
        );

        settle_stranded_trigger_runs(&pool, &bus).await;

        assert_eq!(
            state_of(&pool, thread_id).await,
            ("failed".into(), "archived".into()),
            "the abort runs the unattended guard, which hides the run"
        );
        assert_eq!(
            aborts(&pool, thread_id).await,
            vec![("recovery_after_restart".to_string(), true)]
        );

        settle_stranded_trigger_runs(&pool, &bus).await;
        assert_eq!(
            abort_count(&pool, thread_id).await,
            1,
            "a second boot finds the run settled"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_review_run_settles_but_stays_in_the_inbox() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = stranded_run(&pool, &bus, true).await;

        settle_stranded_trigger_runs(&pool, &bus).await;

        assert_eq!(
            state_of(&pool, thread_id).await,
            ("failed".into(), "inbox".into())
        );
        assert_eq!(abort_count(&pool, thread_id).await, 1);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_pinned_run_settles_but_is_never_archived() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = stranded_run(&pool, &bus, false).await;
        emit(&bus, thread_id, ThreadEvent::ThreadSaved).await;

        settle_stranded_trigger_runs(&pool, &bus).await;

        assert_eq!(
            state_of(&pool, thread_id).await,
            ("failed".into(), "inbox".into())
        );
        assert_eq!(abort_count(&pool, thread_id).await, 1);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn leaves_a_parked_run_alone() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = stranded_run(&pool, &bus, false).await;
        sqlx::query(
            "UPDATE thread_summaries SET status = 'waiting_for_user_answer' WHERE thread_id = $1",
        )
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();

        settle_stranded_trigger_runs(&pool, &bus).await;

        assert_eq!(
            state_of(&pool, thread_id).await,
            ("waiting_for_user_answer".into(), "inbox".into())
        );
        assert_eq!(abort_count(&pool, thread_id).await, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn leaves_an_archived_run_alone() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = stranded_run(&pool, &bus, true).await;
        sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
            .bind(thread_id)
            .execute(&pool)
            .await
            .unwrap();

        settle_stranded_trigger_runs(&pool, &bus).await;

        assert_eq!(
            state_of(&pool, thread_id).await,
            ("idle".into(), "archived".into())
        );
        assert_eq!(abort_count(&pool, thread_id).await, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn leaves_a_completed_run_alone() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = stranded_run(&pool, &bus, true).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::TriggerCompleted {
                trigger_id: "t-stranded".into(),
                trigger_name: Some("watch".into()),
                result_summary: Some("done".into()),
            },
        )
        .await;

        settle_stranded_trigger_runs(&pool, &bus).await;

        assert_eq!(abort_count(&pool, thread_id).await, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

mod cut_trigger_fire_shapes {
    use crate::engine::agent_recovery::recovery::{
        settle_orphaned_running_threads, settle_stranded_trigger_runs,
        settle_unresumed_switch_threads,
    };
    use crate::engine::chat::process::trigger_process_result;
    use crate::engine::chat::recovery::{emit_restart_abort, orphan_threads_sql};
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{
        emit_response_aborted, emit_response_canceled, AbortCause, CancelCause, EventChannel,
        EventMeta, MessageOrigin, ThreadEvent, TriggerInvocation,
    };
    use crate::test_support::{setup_test_db, teardown_test_db};
    use std::collections::HashSet;
    use uuid::Uuid;

    enum Cut {
        ChatSweep,
        RunningSettle,
        StrandedSettle,
        UnkeptSwitch,
        GracefulStop,
    }

    async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) -> Uuid {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta {
                channel: Some(EventChannel::Trigger),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap()
        .event_id
    }

    async fn start(bus: &EventBus, thread_id: Uuid) -> Uuid {
        emit(
            bus,
            thread_id,
            ThreadEvent::TriggerStarted {
                retry_of: None,
                provider: None,
                trigger_id: "cut-fire".into(),
                trigger_name: Some("Nightly report".into()),
                prompt: Some("Prepare the report".into()),
                invocation: Some(TriggerInvocation::Schedule),
                origin: None,
                go_to_review: false,
                model: None,
                reasoning_effort: None,
                continue_key: None,
            },
        )
        .await
    }

    async fn abort(bus: &EventBus, thread_id: Uuid, start_id: Uuid, switch: bool) {
        emit_response_aborted(
            bus,
            thread_id,
            AbortCause::EngineShutdown,
            "This response was interrupted by an engine shutdown.".into(),
            vec![],
            None,
            None,
            EventMeta {
                request_event_id: Some(start_id),
                actor: Some(if switch {
                    MessageOrigin::Device {
                        device_id: "test-device".into(),
                    }
                } else {
                    MessageOrigin::system()
                }),
                ..EventMeta::NONE
            },
            "[Test] teardown boundary",
        )
        .await;
    }

    async fn characterize(cut: Cut) {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let start_id = start(&bus, thread_id).await;
        match cut {
            Cut::ChatSweep => {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                emit(
                    &bus,
                    thread_id,
                    ThreadEvent::TextStreamed {
                        text: "Partial report".into(),
                    },
                )
                .await;
                type Orphan = (
                    Uuid,
                    Option<String>,
                    Option<Uuid>,
                    Option<String>,
                    Option<String>,
                );
                let rows: Vec<Orphan> = sqlx::query_as(&orphan_threads_sql())
                    .bind(Vec::<Uuid>::new())
                    .fetch_all(&pool)
                    .await
                    .unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].0, thread_id);
                assert_eq!(rows[0].2, Some(start_id));
                emit_restart_abort(
                    &bus,
                    thread_id,
                    "Restart interrupted the report".into(),
                    Some(start_id),
                    Some(EventChannel::Trigger),
                )
                .await
                .unwrap();
            }
            Cut::RunningSettle => {
                settle_orphaned_running_threads(&pool, &bus, &HashSet::new()).await;
            }
            Cut::StrandedSettle => {
                sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE thread_id = $1")
                    .bind(thread_id)
                    .execute(&pool)
                    .await
                    .unwrap();
                settle_stranded_trigger_runs(&pool, &bus).await;
            }
            Cut::UnkeptSwitch => {
                abort(&bus, thread_id, start_id, true).await;
                settle_unresumed_switch_threads(&pool, &bus, &HashSet::new()).await;
            }
            Cut::GracefulStop => {
                abort(&bus, thread_id, start_id, false).await;
            }
        }

        crate::scheduler::cut_fires::tests::report_all(&pool, &bus).await;

        let state: (String, String, String) = sqlx::query_as(
            "SELECT status, archive_state, source FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, ("failed".into(), "inbox".into(), "trigger".into()));
        let terminal: (String, String) = sqlx::query_as(
            "SELECT payload->>'cause', payload->>'request_event_id' FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseAborted' ORDER BY sequence DESC LIMIT 1",
        ).bind(thread_id.to_string()).fetch_one(&pool).await.unwrap();
        assert_eq!(terminal.1, start_id.to_string());
        assert_eq!(
            terminal.0,
            if matches!(cut, Cut::GracefulStop) {
                "engine_shutdown"
            } else {
                "recovery_after_restart"
            }
        );
        let reports: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE event_type IN ('TriggerExecuted', 'NotificationCreated', 'TriggerRunInterrupted')",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(reports, 3, "boot reports a cut fire visibly");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn chat_sweep_reports_in_the_inbox() {
        characterize(Cut::ChatSweep).await;
    }

    #[tokio::test]
    async fn running_settle_reports_in_the_inbox() {
        characterize(Cut::RunningSettle).await;
    }

    #[tokio::test]
    async fn stranded_settle_reports_in_the_inbox() {
        characterize(Cut::StrandedSettle).await;
    }

    #[tokio::test]
    async fn unkept_switch_reports_in_the_inbox() {
        characterize(Cut::UnkeptSwitch).await;
    }

    #[tokio::test]
    async fn graceful_stop_reports_in_the_inbox() {
        characterize(Cut::GracefulStop).await;
    }

    #[tokio::test]
    async fn shutdown_abort_cannot_become_a_successful_trigger_result() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let start_id = start(&bus, thread_id).await;
        abort(&bus, thread_id, start_id, false).await;

        emit_response_canceled(
            &bus,
            &pool,
            thread_id,
            CancelCause::UserStop,
            String::new(),
            vec![],
            None,
            None,
            EventMeta {
                request_event_id: Some(start_id),
                ..EventMeta::NONE
            },
            "[Test] canceled loop",
        )
        .await;
        let result = crate::engine::agentic_loop::terminal_result(
            String::new(),
            vec![],
            Uuid::new_v4(),
            thread_id,
            false,
        );
        let error = trigger_process_result(&pool, result)
            .await
            .expect_err("an aborted trigger must not return success to the scheduler");
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::Interrupted,
        );
        let canceled: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'ResponseCanceled'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(canceled, 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_completed_turn_after_an_abort_is_still_successful() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let start_id = start(&bus, thread_id).await;
        abort(&bus, thread_id, start_id, false).await;
        start(&bus, thread_id).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::ResponseGenerated {
                text: "Report prepared".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
        )
        .await;
        let result = crate::engine::agentic_loop::terminal_result(
            "Report prepared".into(),
            vec![],
            Uuid::new_v4(),
            thread_id,
            false,
        );
        assert!(trigger_process_result(&pool, result).await.is_ok());

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

mod restart_boundary_reads {
    use crate::engine::agent_recovery::recovery::{
        boundary_abort_already_emitted, switch_was_user_initiated,
    };
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{
        AbortCause, ActorMode, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
    };
    use crate::test_support::{setup_test_db, teardown_test_db};
    use std::time::Duration;
    use uuid::Uuid;

    fn device_actor() -> MessageOrigin {
        MessageOrigin::Device {
            device_id: "dev-1".into(),
        }
    }

    async fn seed_cc_start(bus: &EventBus, thread_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "do the thing".into(),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    async fn seed_continuation_started(bus: &EventBus, thread_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationStarted {
                branch: "feature-branch".into(),
                origin: None,
                reason: Some("engine_restart_interrupt".into()),
                resumed_abort_id: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    async fn abort_with(bus: &EventBus, thread_id: Uuid, actor: Option<MessageOrigin>) {
        abort_with_cause(bus, thread_id, actor, AbortCause::EngineShutdown).await;
    }

    async fn abort_with_cause(
        bus: &EventBus,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
        cause: AbortCause,
    ) {
        crate::engine::thread_events::emit_response_aborted(
            bus,
            thread_id,
            cause,
            String::new(),
            vec![],
            None,
            None,
            EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                actor,
                ..EventMeta::NONE
            },
            "[test] teardown abort",
        )
        .await;
    }

    async fn tick() {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    #[tokio::test]
    async fn device_attributed_abort_is_user_initiated_switch() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_cc_start(&bus, thread_id).await;
        tick().await;
        abort_with(&bus, thread_id, Some(device_actor())).await;

        assert!(
            switch_was_user_initiated(&pool, thread_id).await,
            "a device-attributed teardown abort newer than the last start means a user Switch → auto-resume"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn system_missing_and_absent_aborts_are_not_a_switch() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let sys_thread = Uuid::new_v4();
        seed_cc_start(&bus, sys_thread).await;
        tick().await;
        abort_with(&bus, sys_thread, Some(MessageOrigin::system())).await;
        assert!(
            !switch_was_user_initiated(&pool, sys_thread).await,
            "a system-attributed abort must NOT auto-resume (crash path → manual Continue)"
        );

        let none_thread = Uuid::new_v4();
        seed_cc_start(&bus, none_thread).await;
        tick().await;
        abort_with(&bus, none_thread, None).await;
        assert!(
            !switch_was_user_initiated(&pool, none_thread).await,
            "an actor-less abort must NOT auto-resume"
        );

        let clean_thread = Uuid::new_v4();
        seed_cc_start(&bus, clean_thread).await;
        assert!(
            !switch_was_user_initiated(&pool, clean_thread).await,
            "no teardown abort means nothing to resume"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn device_abort_older_than_latest_start_is_not_a_switch() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        seed_cc_start(&bus, thread_id).await;
        tick().await;
        abort_with(&bus, thread_id, Some(device_actor())).await;
        tick().await;
        seed_cc_start(&bus, thread_id).await;

        assert!(
            !switch_was_user_initiated(&pool, thread_id).await,
            "a device abort older than the latest start has been consumed by a prior resume → no re-resume"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_switch_abort_withdrawn_by_the_floor_is_not_a_switch() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        seed_cc_start(&bus, thread_id).await;
        tick().await;
        abort_with(&bus, thread_id, Some(device_actor())).await;
        assert!(
            switch_was_user_initiated(&pool, thread_id).await,
            "precondition: the switch abort alone reads as a user switch"
        );
        tick().await;
        abort_with_cause(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::RecoveryAfterRestart,
        )
        .await;

        assert!(
            !switch_was_user_initiated(&pool, thread_id).await,
            "the floor withdrew this resume promise, so no later boot may auto-resume it"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn device_attributed_non_shutdown_abort_is_not_a_switch() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        for cause in [
            AbortCause::StaleSettle,
            AbortCause::SafetyNet,
            AbortCause::ProcessKilled,
            AbortCause::RecoveryAfterRestart,
        ] {
            let thread_id = Uuid::new_v4();
            seed_cc_start(&bus, thread_id).await;
            tick().await;
            abort_with_cause(&bus, thread_id, Some(device_actor()), cause).await;

            assert!(
                !switch_was_user_initiated(&pool, thread_id).await,
                "{cause:?} is not an engine-shutdown teardown — it must never auto-resume, \
                 even when a user button supplied the device actor"
            );
        }

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_teardown_abort_in_the_current_turn_suppresses_a_second_boundary() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let clean_thread = Uuid::new_v4();
        seed_cc_start(&bus, clean_thread).await;
        assert!(
            !boundary_abort_already_emitted(&pool, clean_thread).await,
            "a turn with no abort at all is owed its interruption boundary"
        );

        let preempted_thread = Uuid::new_v4();
        seed_cc_start(&bus, preempted_thread).await;
        tick().await;
        abort_with(&bus, preempted_thread, Some(device_actor())).await;
        assert!(
            boundary_abort_already_emitted(&pool, preempted_thread).await,
            "the teardown pre-emit already covers this turn"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_consumed_switch_abort_does_not_suppress_the_next_crash_boundary() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        seed_cc_start(&bus, thread_id).await;
        tick().await;
        abort_with(&bus, thread_id, Some(device_actor())).await;
        tick().await;
        seed_continuation_started(&bus, thread_id).await;

        assert!(
            !switch_was_user_initiated(&pool, thread_id).await,
            "precondition: the resume consumed the switch abort, so this crash \
             falls back to manual Continue"
        );
        assert!(
            !boundary_abort_already_emitted(&pool, thread_id).await,
            "the consumed switch abort belongs to the previous turn, so the turn \
             the resume opened is still owed its own interruption boundary"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
