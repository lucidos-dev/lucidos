use super::*;

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_covered_task_is_watched_whether_or_not_it_is_still_running() {
    let covered = ids(&["other", "mine"]);
    assert_eq!(settle_start("mine", &covered, true), Settled::Watched);
    assert_eq!(settle_start("mine", &covered, false), Settled::Watched);
}

#[test]
fn a_running_task_no_wait_covers_is_unwatched() {
    assert_eq!(
        settle_start("mine", &ids(&["other"]), true),
        Settled::Unwatched
    );
}

#[test]
fn a_task_that_finished_before_any_wait_comes_back_inline() {
    assert_eq!(settle_start("mine", &[], false), Settled::AlreadyFinished);
}

#[test]
fn a_wait_is_capped_whatever_the_caller_asks() {
    assert_eq!(agent_wait_secs(None), AGENT_WAIT_MAX_SECS);
    assert_eq!(agent_wait_secs(Some(100_000)), AGENT_WAIT_MAX_SECS);
    assert_eq!(agent_wait_secs(Some(5)), 5);
    const { assert!(AGENT_WAIT_MAX_SECS <= 230) };
}

#[test]
fn an_unproven_floor_match_points_the_agent_at_the_foreground() {
    use crate::engine::command_guard::Catastrophe;
    let proven = background_floor_refusal(Catastrophe::Proven("x"));
    let unproven = background_floor_refusal(Catastrophe::Unproven("x"));
    assert!(!proven.contains("foreground"));
    assert!(unproven.contains("cannot rule out"));
    assert!(unproven.contains("allow it once"));
}

#[test]
fn a_task_runs_up_to_the_agent_ceiling_and_defaults_to_the_hour() {
    assert_eq!(agent_task_timeout_secs(None), AGENT_BG_DEFAULT_TIMEOUT_SECS);
    assert_eq!(
        agent_task_timeout_secs(Some(5 * 3_600)),
        AGENT_BG_MAX_TIMEOUT_SECS
    );
    assert_eq!(agent_task_timeout_secs(Some(2 * 3_600)), 2 * 3_600);
    assert_eq!(agent_task_timeout_secs(Some(0)), 1);
    const { assert!(AGENT_BG_DEFAULT_TIMEOUT_SECS < AGENT_BG_MAX_TIMEOUT_SECS) };
    const { assert!(crate::llm::tools::BG_MAX_TIMEOUT_SECS < AGENT_BG_MAX_TIMEOUT_SECS) };
}

#[test]
fn discard_and_archive_abandon_background_tasks_and_nothing_else_does() {
    use crate::engine::types::StopReason;
    assert!(StopReason::Discard.abandons_background_tasks());
    assert!(StopReason::Archive.abandons_background_tasks());
    assert!(!StopReason::Apply.abandons_background_tasks());
    assert!(!StopReason::UserStop.abandons_background_tasks());
}

#[tokio::test]
async fn an_archive_lost_to_a_lag_still_kills_the_threads_task() {
    use crate::engine::event_bus::EventBus;
    use crate::engine::thread_events::ThreadEvent;
    use crate::test_support::{
        flood_past_bus_capacity, seed_chat_thread, seed_thread_event, setup_test_db,
        teardown_test_db,
    };

    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let registry = BackgroundBashRegistry::new();
    let thread_id = seed_chat_thread(&bus, "run the suite").await;
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::ResponseGenerated {
            text: "started".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;
    let (task_id, _rx) = registry
        .spawn(
            "sleep 30",
            60,
            std::path::Path::new("/tmp"),
            &[],
            Some(thread_id),
            None,
        )
        .await
        .expect("spawn");
    let (mut rx, mut replay) = LagReplay::subscribe(
        &bus,
        ReplayScope::Workspace,
        Some(WORK_ENDING_EVENT_TYPES.to_vec()),
    );

    seed_thread_event(&bus, thread_id, ThreadEvent::ThreadArchived).await;
    flood_past_bus_capacity(&bus);

    assert!(reap_ended_work(rx.recv().await, &mut replay, &registry, &pool).await);
    assert_eq!(
        registry.kill_for_thread(thread_id).await,
        0,
        "the archived thread's task {task_id} must already be stopped"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_lag_leaves_a_task_running_on_a_thread_archived_without_an_archive_event() {
    use crate::engine::event_bus::EventBus;
    use crate::test_support::{
        flood_past_bus_capacity, seed_chat_thread, setup_test_db, teardown_test_db,
    };

    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let registry = BackgroundBashRegistry::new();
    let thread_id = seed_chat_thread(&bus, "nightly sync").await;
    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();
    registry
        .spawn(
            "sleep 30",
            60,
            std::path::Path::new("/tmp"),
            &[],
            Some(thread_id),
            None,
        )
        .await
        .expect("spawn");
    let (mut rx, mut replay) = LagReplay::subscribe(
        &bus,
        ReplayScope::Workspace,
        Some(WORK_ENDING_EVENT_TYPES.to_vec()),
    );
    flood_past_bus_capacity(&bus);

    assert!(reap_ended_work(rx.recv().await, &mut replay, &registry, &pool).await);
    assert_eq!(
        registry.kill_for_thread(thread_id).await,
        1,
        "the task must still be running, so this cleanup is what stops it"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn the_reaper_replays_every_event_that_ends_a_threads_work() {
    use crate::engine::thread_events::ThreadEvent;
    let ending = [
        ThreadEvent::ThreadArchived,
        ThreadEvent::ThreadDiscarded { actor: None },
    ];
    for event in &ending {
        assert!(ends_the_threads_work(event));
        assert!(WORK_ENDING_EVENT_TYPES.contains(&event.event_type()));
    }
    assert_eq!(WORK_ENDING_EVENT_TYPES.len(), ending.len());
}

#[tokio::test]
async fn a_catastrophic_or_secret_reading_command_is_refused_before_it_starts() {
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};

    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let engine = LucidosEngine::new(
        workspace.path().to_path_buf(),
        &test_db_url(&db),
        crate::llm::ProviderBuildContext {
            default_model: "mock".into(),
            model_is_mock: true,
            vertex_project_id: String::new(),
            vertex_location: crate::llm::vertex::location_handle(String::new()),
            vertex_token_cache: None,
            model_registry: Default::default(),
            not_served: Default::default(),
            claude_gateway_access: None,
            chatgpt_plan_access: None,
        },
        std::sync::Arc::new(crate::engine::agent_recovery::EngineAgentHost),
    )
    .await
    .unwrap();
    let thread_id = Uuid::new_v4();
    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut session = crate::engine::AgentSession::for_test_with_sender(msg_tx);
    session.worktree_path = Some(workspace.path().to_path_buf());
    engine
        .agent_sessions
        .lock()
        .await
        .insert(thread_id, session);

    for (command, refusal) in [
        ("dropdb lucidos_example", "Refused: the command is"),
        (
            "cat ~/.lucidos/local-token",
            "Refused: background tasks cannot read Lucidos secrets.",
        ),
    ] {
        let err = engine
            .start_background_task_for_agent(
                thread_id,
                command,
                None,
                None,
                CompletionDelivery::Poll,
            )
            .await
            .expect_err(command);
        assert!(err.starts_with(refusal), "{command}: {err}");
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

mod guard_tests {
    use super::*;

    fn lucidos_cli_doc() -> String {
        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
        crate::core::knowhow::doc_family_text(&repo, "system-knowhow/lucidos-cli.md")
    }

    #[test]
    fn every_copy_of_the_task_timeout_states_the_engine_constants() {
        let (default, max) = (AGENT_BG_DEFAULT_TIMEOUT_SECS, AGENT_BG_MAX_TIMEOUT_SECS);
        let cli_doc = lucidos_cli_doc();
        let copies = [
            (
                "crates/lucidos-cli/src/main.rs",
                include_str!("../../../../lucidos-cli/src/main.rs"),
            ),
            ("system-knowhow/lucidos-cli.md", cli_doc.as_str()),
        ];
        for (path, text) in copies {
            assert!(
                text.contains(&format!("default {default}, maximum {max}")),
                "{path} must state the {default} s default and the {max} s ceiling"
            );
        }
    }

    #[test]
    fn every_copy_of_the_wait_cap_states_the_engine_constant() {
        let cap = AGENT_WAIT_MAX_SECS;
        let cli_doc = lucidos_cli_doc();
        let copies = [
            (
                "crates/lucidos-cli/src/main.rs",
                include_str!("../../../../lucidos-cli/src/main.rs"),
            ),
            ("system-knowhow/lucidos-cli.md", cli_doc.as_str()),
            (
                ".claude/skills/run-e2e/SKILL.md",
                include_str!("../../../../../.claude/skills/run-e2e/SKILL.md"),
            ),
            (
                ".claude/skills/run-tests/SKILL.md",
                include_str!("../../../../../.claude/skills/run-tests/SKILL.md"),
            ),
        ];
        for (path, text) in copies {
            assert!(
                text.contains(&format!("{cap} s")),
                "{path} must state the {cap} s cap"
            );
        }
        let cli = include_str!("../../../../lucidos-cli/src/background_task.rs");
        let deadline: u64 = cli
            .split("WAIT_RESPONSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(")
            .nth(1)
            .and_then(|rest| rest.split(')').next())
            .and_then(|n| n.parse().ok())
            .expect("the CLI's wait deadline");
        assert!(
            deadline > cap,
            "the CLI must wait longer than the engine's cap"
        );
    }

    #[test]
    fn a_coding_agents_background_task_never_gets_the_workspace_secrets() {
        use crate::test_support::source_scan::{engine_source, read_production_source};
        let this =
            read_production_source(&engine_source("engine/agent_session/background_task.rs"));
        assert!(this.contains("build_agent_task_env_vars("), "{this}");
        assert!(
            !this.contains("build_tool_env_vars") && !this.contains("build_script_env_vars"),
            "the agent route must not take the chat tool's env"
        );

        let scripts = read_production_source(&engine_source("engine/engine_impl/scripts.rs"));
        let start = scripts
            .find("async fn build_agent_task_env_vars(")
            .expect("the agent env builder");
        let body = &scripts[start..];
        let body = &body[..body.find("\n    }\n").expect("end of the builder")];
        for secret_source in ["secret_env_vars", "CredentialStore", "OAuthStore"] {
            assert!(
                !body.contains(secret_source),
                "the agent env must not reach `{secret_source}`:\n{body}"
            );
        }
    }

    #[test]
    fn stop_agent_kills_the_threads_tasks_whether_or_not_a_session_is_live() {
        let src = crate::test_support::source_scan::read_production_source(
            &crate::test_support::source_scan::engine_source("engine/claude_code/control.rs"),
        );
        let body = &src[src.find("async fn stop_agent(").expect("stop_agent")..];
        let kill = body
            .find("abandon_background_tasks(")
            .expect("stop_agent must kill the thread's background tasks");
        let refusal = body
            .find("stop_refusal(")
            .expect("stop_agent asks whether the stop is refused");
        let branch = body
            .find("if let Some(stop) = reserved")
            .expect("stop_agent branches on the session it reserved");
        assert!(refusal < kill, "a refused stop must kill nothing");
        assert!(kill < branch, "the kill must not depend on a live session");
        assert!(
            body[..kill].contains("abandons_background_tasks()"),
            "the kill is gated on the stop reason"
        );
    }

    #[test]
    fn the_reaper_fires_on_an_archived_or_discarded_thread_only() {
        use crate::engine::thread_events::ThreadEvent;
        assert!(ends_the_threads_work(&ThreadEvent::ThreadArchived));
        assert!(ends_the_threads_work(&ThreadEvent::ThreadDiscarded {
            actor: None
        }));
        assert!(!ends_the_threads_work(&ThreadEvent::ThreadSaved));

        let main = crate::test_support::source_scan::read_production_source(
            &crate::test_support::source_scan::engine_source(
                "../../lucidos-engine-bin/src/main.rs",
            ),
        );
        assert!(
            main.contains("agent_recovery::start_background_task_reaper(&shared_engine)"),
            "the engine must start the reaper at boot"
        );
    }

    #[test]
    fn every_user_discard_kills_the_threads_background_tasks() {
        let src = crate::test_support::source_scan::read_production_source(
            &crate::test_support::source_scan::engine_source("engine/change_ops/discard.rs"),
        );
        for entry in [
            "pub async fn discard_open_changes_for_thread(",
            "pub async fn discard_change(",
        ] {
            let body = &src[src.find(entry).expect(entry)..];
            let body = &body[..body.find("\n    }\n").expect("end of fn")];
            assert!(
                body.contains("abandon_background_tasks("),
                "{entry} must kill the thread's background tasks"
            );
        }
    }
}
