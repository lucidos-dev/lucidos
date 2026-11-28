use super::{orphan_threads_sql, orphan_tool_calls_sql, ORPHAN_THREADS_SQL, ORPHAN_TOOL_CALLS_SQL};
use crate::core::EventRow;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventMeta, QuestionOption, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn restore_settlement_ends_running_and_parked_turns_once_and_preserves_fresh_work() {
    use crate::engine::restore_boot::settle_restored_threads;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let running = Uuid::new_v4();
    let parked = Uuid::new_v4();
    let continued = Uuid::new_v4();
    emit_orphan_turn(&bus, continued).await;
    emit_orphan_turn(&bus, running).await;
    emit_orphan_turn(&bus, parked).await;
    emit_unanswered_question(&bus, parked, "restore-question").await;
    bus.emit(BusEvent::Thread {
        thread_id: running,
        event: ThreadEvent::ApplyNowStarted,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    settle_restored_threads(&pool, &bus).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'ResponseAborted'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "normal boots preserve question parks");
    sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) SELECT $1, 'snapshot', now(), MAX(sequence) FROM events")
        .bind(Uuid::new_v4()).execute(&pool).await.unwrap();
    let fresh = Uuid::new_v4();
    emit_orphan_turn(&bus, fresh).await;
    emit_orphan_turn(&bus, continued).await;
    bus.emit(BusEvent::Thread {
        thread_id: continued,
        event: ThreadEvent::ApplyNowStarted,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    crate::test_support::refuse_event_type(&pool, "ResponseAborted").await;
    assert!(settle_restored_threads(&pool, &bus).await.is_err());
    sqlx::query("DROP TRIGGER refuse_event ON events")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION refuse_event()")
        .execute(&pool)
        .await
        .unwrap();
    crate::test_support::refuse_event_type(&pool, "ApplyNowEnded").await;
    assert!(settle_restored_threads(&pool, &bus).await.is_err());
    sqlx::query("DROP TRIGGER refuse_event ON events")
        .execute(&pool)
        .await
        .unwrap();
    for _ in 0..2 {
        settle_restored_threads(&pool, &bus).await.unwrap();
        let settled: Vec<Uuid> = sqlx::query_scalar("SELECT thread_id FROM events WHERE event_type = 'ResponseAborted' AND payload->>'cause' = 'snapshot_restored' ORDER BY thread_id").fetch_all(&pool).await.unwrap();
        let mut expected = vec![running, parked];
        expected.sort();
        assert_eq!(settled, expected);
        assert!(
            !crate::engine::question_park_sql::thread_has_unanswered_question(&pool, parked).await
        );
        assert_eq!(
            crate::engine::agent_question::lookup_pending_question_tool_use_id(&pool, parked).await,
            None
        );
        let closed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE thread_id = $1 \
             AND event_type = 'UserQuestionAnswered' AND payload->>'tool_use_id' = 'restore-question'",
        )
        .bind(parked)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(closed, 1);
        assert!(orphan_ids(&pool).await.contains(&fresh));
        assert!(orphan_ids(&pool).await.contains(&continued));
        assert!(!orphan_ids(&pool).await.contains(&running));
        let applying: bool =
            sqlx::query_scalar("SELECT apply_in_flight FROM thread_summaries WHERE thread_id = $1")
                .bind(running)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!applying);
    }
    sqlx::query("ALTER TABLE snapshot_restore_boundary RENAME TO hidden_restore_boundary")
        .execute(&pool)
        .await
        .unwrap();
    assert!(settle_restored_threads(&pool, &bus).await.is_err());
    sqlx::query("ALTER TABLE hidden_restore_boundary RENAME TO snapshot_restore_boundary")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    teardown_test_db(&db).await;
}

async fn emit_unanswered_question(bus: &EventBus, thread_id: Uuid, tool_use_id: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.into(),
            cc_session_id: String::new(),
            question: "Pick one".into(),
            options: vec![QuestionOption {
                id: "opt-0".into(),
                label: "A".into(),
                description: None,
                preview: None,
                widget: None,
                quiet: false,
                quiet_reply: None,
            }],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            permission_request: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn tick() {
    tokio::time::sleep(Duration::from_millis(1)).await;
}

async fn emit_orphan_turn(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "stuck request".into(),
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
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TextStreamed {
            text: "partial reply".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn emit_orphan_tool_call(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "trigger a tool".into(),
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
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolCalled {
            name: "Read".into(),
            args: json!({"path": "/tmp/x"}),
            description: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn archive(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

type OrphanThreadRow = (
    Uuid,
    Option<String>,
    Option<Uuid>,
    Option<String>,
    Option<String>,
);

#[tokio::test]
async fn a_turn_woken_by_a_child_completion_is_an_orphan_when_it_dies() {
    use crate::engine::thread_events::{AbortCause, ChildCompletionStatus};
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    emit_orphan_turn(&bus, thread_id).await;
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: AbortCause::EngineShutdown,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChildThreadCompleted {
            child_thread_id: Uuid::new_v4(),
            child_thread_title: Some("child".into()),
            status: ChildCompletionStatus::Success,
            summary: "child did the thing".into(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
            integrated_branch: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TextStreamed {
            text: "woken reply".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(&orphan_threads_sql())
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    let hit = rows.iter().find(|r| r.0 == thread_id);
    assert_eq!(
        hit.and_then(|r| r.3.as_deref()),
        Some("ChildThreadCompleted"),
        "the wake turn must be found, anchored on the completion"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_tool_call_rows_come_back_grouped_by_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    emit_orphan_tool_call(&bus, first).await;
    tick().await;
    emit_orphan_tool_call(&bus, second).await;
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id: first,
        event: ThreadEvent::ToolCalled {
            name: "Read".into(),
            args: json!({"path": "/tmp/y"}),
            description: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let rows: Vec<EventRow> = sqlx::query_as(&orphan_tool_calls_sql())
        .fetch_all(&pool)
        .await
        .unwrap();
    let mut ids: Vec<Uuid> = rows
        .iter()
        .filter_map(|r| r.thread_id)
        .filter(|t| *t == first || *t == second)
        .collect();
    assert_eq!(ids.len(), 3);
    ids.dedup();
    assert_eq!(ids.len(), 2, "the linear walk needs one run per thread");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn the_synthetic_tool_result_names_the_call_it_pairs_with() {
    let orphan_id = Uuid::new_v4();
    let event = super::interrupted_tool_result("Read".into(), orphan_id);
    match event {
        ThreadEvent::ToolResult {
            tool_called_event_id,
            success,
            ..
        } => {
            assert_eq!(tool_called_event_id, Some(orphan_id));
            assert!(!success);
        }
        other => panic!("expected a ToolResult, got {other:?}"),
    }
}

#[test]
fn boot_pairs_orphan_tool_calls_before_the_chat_sweep_aborts() {
    const MAIN_SRC: &str = include_str!("../../../../lucidos-engine-bin/src/main.rs");
    let pair = MAIN_SRC
        .find("agent_recovery::recover_orphan_tool_calls(")
        .expect("boot pairs orphan tool calls");
    let sweep = MAIN_SRC
        .find("agent_recovery::recover_orphaned_threads(")
        .expect("boot runs the chat sweep");
    assert!(pair < sweep);
}

#[tokio::test]
async fn orphan_threads_query_includes_unarchived_orphan() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    emit_orphan_turn(&bus, thread_id).await;

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(ORPHAN_THREADS_SQL)
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    let hit = rows.iter().find(|(tid, _, _, _, _)| *tid == thread_id);
    assert!(
        hit.is_some(),
        "unarchived orphan must be visible to the recovery sweep — got {:?}",
        rows.iter().map(|r| r.0).collect::<Vec<_>>()
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn died_while_archived(pool: &sqlx::PgPool, thread_id: Uuid) {
    sqlx::query("UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn orphan_ids(pool: &sqlx::PgPool) -> Vec<Uuid> {
    let rows: Vec<OrphanThreadRow> = sqlx::query_as(ORPHAN_THREADS_SQL)
        .bind(Vec::<Uuid>::new())
        .fetch_all(pool)
        .await
        .unwrap();
    rows.into_iter().map(|r| r.0).collect()
}

#[tokio::test]
async fn orphan_threads_query_includes_an_archived_thread_that_died_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let archived_id = Uuid::new_v4();
    emit_orphan_turn(&bus, archived_id).await;
    archive(&bus, archived_id).await;
    died_while_archived(&pool, archived_id).await;

    assert!(orphan_ids(&pool).await.contains(&archived_id));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_threads_query_leaves_archived_history_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let archived_id = Uuid::new_v4();
    emit_orphan_turn(&bus, archived_id).await;
    archive(&bus, archived_id).await;

    assert!(!orphan_ids(&pool).await.contains(&archived_id));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_threads_query_excludes_threads_owned_by_cc_recovery() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let cc_owned_id = Uuid::new_v4();
    emit_orphan_turn(&bus, cc_owned_id).await;

    let control_id = Uuid::new_v4();
    emit_orphan_turn(&bus, control_id).await;

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(ORPHAN_THREADS_SQL)
        .bind(vec![cc_owned_id])
        .fetch_all(&pool)
        .await
        .unwrap();
    let returned: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    assert!(
        returned.contains(&control_id),
        "a non-excluded orphan must still be returned — got {:?}",
        returned
    );
    assert!(
        !returned.contains(&cc_owned_id),
        "an orphan in exclude_thread_ids (owned by CC recovery) must NOT be returned — got {:?}",
        returned
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_threads_query_skips_question_parked_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parked_id = Uuid::new_v4();
    emit_orphan_turn(&bus, parked_id).await;
    tick().await;
    emit_unanswered_question(&bus, parked_id, "toolu_parked").await;

    let control_id = Uuid::new_v4();
    emit_orphan_turn(&bus, control_id).await;

    let base_rows: Vec<OrphanThreadRow> = sqlx::query_as(ORPHAN_THREADS_SQL)
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        base_rows.iter().any(|(tid, _, _, _, _)| *tid == parked_id),
        "precondition: unguarded base query must see the parked orphan"
    );

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(&orphan_threads_sql())
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    let returned: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    assert!(
        returned.contains(&control_id),
        "a non-question orphan must still be recovered — got {:?}",
        returned
    );
    assert!(
        !returned.contains(&parked_id),
        "a thread parked on an unanswered question must NOT be swept — got {:?}",
        returned
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_threads_query_recovers_answered_then_interrupted_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let answered_id = Uuid::new_v4();
    emit_orphan_turn(&bus, answered_id).await;
    tick().await;
    emit_unanswered_question(&bus, answered_id, "toolu_answered").await;
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id: answered_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: "toolu_answered".into(),
            quiet: false,
            answer: crate::engine::thread_events::AnswerKind::Selected {
                option_id: "opt-0".into(),
            },
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id: answered_id,
        event: ThreadEvent::TextStreamed {
            text: "resumed work".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(&orphan_threads_sql())
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        rows.iter().any(|(tid, _, _, _, _)| *tid == answered_id),
        "an answered-then-interrupted thread is a genuine orphan and must still be recovered"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_tool_calls_query_skips_question_parked_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parked_id = Uuid::new_v4();
    emit_orphan_tool_call(&bus, parked_id).await;
    tick().await;
    emit_unanswered_question(&bus, parked_id, "toolu_parked_tc").await;

    let control_id = Uuid::new_v4();
    emit_orphan_tool_call(&bus, control_id).await;

    let rows: Vec<EventRow> = sqlx::query_as(&orphan_tool_calls_sql())
        .fetch_all(&pool)
        .await
        .unwrap();
    let returned: std::collections::HashSet<Uuid> =
        rows.iter().filter_map(|r| r.thread_id).collect();
    assert!(
        returned.contains(&control_id),
        "a non-question thread's orphan ToolCalled must still be paired — got {:?}",
        returned
    );
    assert!(
        !returned.contains(&parked_id),
        "a question-parked thread's ToolCalled must NOT be swept — got {:?}",
        returned
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn tool_call_thread_ids(pool: &sqlx::PgPool) -> std::collections::HashSet<Uuid> {
    let rows: Vec<EventRow> = sqlx::query_as(ORPHAN_TOOL_CALLS_SQL)
        .fetch_all(pool)
        .await
        .unwrap();
    rows.iter().filter_map(|r| r.thread_id).collect()
}

#[tokio::test]
async fn orphan_tool_calls_query_takes_archived_threads_only_while_running() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let died = Uuid::new_v4();
    emit_orphan_tool_call(&bus, died).await;
    archive(&bus, died).await;
    died_while_archived(&pool, died).await;

    let history = Uuid::new_v4();
    emit_orphan_tool_call(&bus, history).await;
    archive(&bus, history).await;

    let returned = tool_call_thread_ids(&pool).await;
    assert!(returned.contains(&died), "got {returned:?}");
    assert!(!returned.contains(&history), "got {returned:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

mod switch_resume {
    use super::tick;
    use crate::engine::chat::recovery::switch_resume_candidates;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{
        AbortCause, ActorMode, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
    };
    use crate::test_support::{setup_test_db, teardown_test_db};
    use uuid::Uuid;

    fn device_actor() -> MessageOrigin {
        MessageOrigin::Device {
            device_id: "dev-1".into(),
        }
    }

    async fn seed_turn(bus: &EventBus, thread_id: Uuid, channel: Option<EventChannel>) {
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
                channel,
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    async fn emit_abort(
        bus: &EventBus,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
        cause: AbortCause,
    ) {
        crate::engine::thread_events::emit_response_aborted(
            bus,
            thread_id,
            cause,
            "interrupted".into(),
            vec![],
            None,
            None,
            EventMeta {
                actor,
                ..EventMeta::NONE
            },
            "[test] teardown abort",
        )
        .await;
    }

    async fn candidates_contain(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
        switch_resume_candidates(pool).await.contains(&thread_id)
    }

    #[tokio::test]
    async fn restored_switch_aborts_never_resume_but_new_switches_do() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let old = Uuid::new_v4();
        seed_turn(&bus, old, None).await;
        emit_abort(&bus, old, Some(device_actor()), AbortCause::EngineShutdown).await;
        sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) SELECT $1, 'snapshot', now(), MAX(sequence) FROM events")
            .bind(Uuid::new_v4()).execute(&pool).await.unwrap();
        for _ in 0..2 {
            assert!(!candidates_contain(&pool, old).await);
            assert!(!crate::engine::agent_recovery::switch_was_user_initiated(&pool, old).await);
        }
        let fresh = Uuid::new_v4();
        seed_turn(&bus, fresh, None).await;
        emit_abort(
            &bus,
            fresh,
            Some(device_actor()),
            AbortCause::EngineShutdown,
        )
        .await;
        assert!(candidates_contain(&pool, fresh).await);
        assert!(crate::engine::agent_recovery::switch_was_user_initiated(&pool, fresh).await);
        sqlx::query("ALTER TABLE snapshot_restore_boundary RENAME TO hidden_restore_boundary")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!candidates_contain(&pool, fresh).await);
        assert!(!crate::engine::agent_recovery::switch_was_user_initiated(&pool, fresh).await);
        sqlx::query("ALTER TABLE hidden_restore_boundary RENAME TO snapshot_restore_boundary")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn device_shutdown_abort_makes_a_chat_thread_a_resume_candidate() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_turn(&bus, thread_id, None).await;
        tick().await;
        emit_abort(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::EngineShutdown,
        )
        .await;

        assert!(
            candidates_contain(&pool, thread_id).await,
            "a chat thread interrupted by a user Switch to new version must auto-resume, \
             not wait for a manual Continue click"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_withdrawn_switch_resume_is_not_a_candidate_on_a_later_boot() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_turn(&bus, thread_id, None).await;
        tick().await;
        emit_abort(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::EngineShutdown,
        )
        .await;
        assert!(
            candidates_contain(&pool, thread_id).await,
            "precondition: the switch abort alone makes the thread a candidate"
        );
        tick().await;
        emit_abort(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::RecoveryAfterRestart,
        )
        .await;

        assert!(
            !candidates_contain(&pool, thread_id).await,
            "the floor withdrew this resume promise, so no later boot may auto-resume it"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn crash_and_user_stop_shaped_aborts_are_not_candidates() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let sys = Uuid::new_v4();
        seed_turn(&bus, sys, None).await;
        tick().await;
        emit_abort(
            &bus,
            sys,
            Some(MessageOrigin::system()),
            AbortCause::EngineShutdown,
        )
        .await;

        let settle = Uuid::new_v4();
        seed_turn(&bus, settle, None).await;
        tick().await;
        emit_abort(&bus, settle, Some(device_actor()), AbortCause::StaleSettle).await;

        let parked = Uuid::new_v4();
        seed_turn(&bus, parked, None).await;

        let found = switch_resume_candidates(&pool).await;
        assert!(
            !found.contains(&sys),
            "a system-attributed abort is the crash path → manual Continue"
        );
        assert!(
            !found.contains(&settle),
            "a StaleSettle abort carries the actor of a user Stop/Archive button, \
             not of a switch — resuming would re-run work the user abandoned"
        );
        assert!(
            !found.contains(&parked),
            "a thread with no abort (question-parked) is not an interrupted turn"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_gateway_picker_restart_makes_a_chat_thread_a_resume_candidate() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let headers =
            crate::api::gateway_stamp::test_support::stamped_device_headers("picker-device");
        let actor = crate::api::actor::user_actor(&headers, None);
        assert!(
            matches!(actor, Some(MessageOrigin::Device { .. })),
            "the picker's device-id header must resolve to a Device actor"
        );

        let thread_id = Uuid::new_v4();
        seed_turn(&bus, thread_id, None).await;
        tick().await;
        emit_abort(&bus, thread_id, actor, AbortCause::EngineShutdown).await;

        assert!(
            candidates_contain(&pool, thread_id).await,
            "a chat thread interrupted by a picker Restart must auto-resume, exactly \
             as one interrupted by the in-workspace Switch does"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn an_already_resumed_thread_is_not_a_candidate_again() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_turn(&bus, thread_id, None).await;
        tick().await;
        emit_abort(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::EngineShutdown,
        )
        .await;
        assert!(candidates_contain(&pool, thread_id).await);

        tick().await;
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationStarted {
                branch: String::new(),
                origin: None,
                reason: None,
                resumed_abort_id: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();

        assert!(
            !candidates_contain(&pool, thread_id).await,
            "the switch abort has been consumed by a resume — a second boot must not re-resume it"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_thread_woken_by_an_event_mid_teardown_is_a_resume_candidate() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_turn(&bus, thread_id, None).await;
        tick().await;
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseGenerated {
                text: "The watcher is armed and I am done for now.".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();

        tick().await;
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::PromptInjected {
                text: "An event you subscribed to has arrived.".into(),
                mode: ActorMode::Agent,
                origin: None,
                injected_message_id: None,
                delivered_event_id: Some(Uuid::new_v4()),
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();

        tick().await;
        emit_abort(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::EngineShutdown,
        )
        .await;

        assert!(
            candidates_contain(&pool, thread_id).await,
            "a wake turn interrupted by the same user switch as its siblings must \
             auto-resume too: when it became in-flight is not a property of the teardown"
        );
        let status: String =
            sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            status, "paused",
            "and it must read as paused, not as the red 'failed' the user reported"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn coding_agent_threads_are_left_to_the_worktree_recovery_pass() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_turn(&bus, thread_id, Some(EventChannel::ClaudeCode)).await;
        tick().await;
        emit_abort(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::EngineShutdown,
        )
        .await;

        let source: Option<String> =
            sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert_eq!(
            source.as_deref(),
            Some("claude_code"),
            "fixture precondition: the thread must project as a coding-agent thread"
        );

        assert!(
            !candidates_contain(&pool, thread_id).await,
            "the chat pass must not claim a coding-agent thread"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_chat_thread_flagged_as_a_coding_agent_is_still_a_candidate() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_turn(&bus, thread_id, None).await;
        tick().await;
        emit_abort(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::EngineShutdown,
        )
        .await;
        sqlx::query("UPDATE thread_summaries SET is_coding_agent = true WHERE thread_id = $1")
            .bind(thread_id)
            .execute(&pool)
            .await
            .unwrap();

        assert!(candidates_contain(&pool, thread_id).await);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn archived_threads_are_not_candidates() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread_id = Uuid::new_v4();
        seed_turn(&bus, thread_id, None).await;
        tick().await;
        emit_abort(
            &bus,
            thread_id,
            Some(device_actor()),
            AbortCause::EngineShutdown,
        )
        .await;
        assert!(candidates_contain(&pool, thread_id).await);

        super::archive(&bus, thread_id).await;

        assert!(
            !candidates_contain(&pool, thread_id).await,
            "an archived thread must not be revived by the auto-resume"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

async fn emit_event_wait_subscription(bus: &EventBus, thread_id: Uuid, wait_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolCalled {
            name: "await_event".into(),
            args: json!({ "reason": "waiting for a change" }),
            description: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitStarted {
            background_timing: None,
            wait_id,
            tool_use_id: format!("toolu_{}", wait_id.simple()),
            on: vec![crate::core::event_subscription::EventSubscription {
                event_type: "ChangeProposed".into(),
                condition: None,
            }],
            reason: "waiting for a change".into(),
            armed_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            watermark: 0,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap()
    .expect("EventWaitStarted must persist");
    tick().await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolResult {
            name: "await_event".into(),
            result: "Subscribed to ChangeProposed. Nothing is blocking.".into(),
            images: vec![],
            success: true,
            tool_called_event_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn orphan_threads_query_recovers_a_thread_that_holds_a_subscription() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let subscribed_id = Uuid::new_v4();
    emit_orphan_turn(&bus, subscribed_id).await;
    tick().await;
    emit_event_wait_subscription(&bus, subscribed_id, Uuid::new_v4()).await;

    let rows: Vec<OrphanThreadRow> = sqlx::query_as(&orphan_threads_sql())
        .bind(Vec::<Uuid>::new())
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        rows.iter().any(|r| r.0 == subscribed_id),
        "a crashed turn is an orphan whether or not the thread is watching for \
         something; only an unanswered question is preserved"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_await_event_call_is_never_an_orphan_tool_call() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let subscribed_id = Uuid::new_v4();
    emit_orphan_turn(&bus, subscribed_id).await;
    tick().await;
    emit_event_wait_subscription(&bus, subscribed_id, Uuid::new_v4()).await;

    let rows: Vec<EventRow> = sqlx::query_as(&orphan_tool_calls_sql())
        .fetch_all(&pool)
        .await
        .unwrap();
    let mine: Vec<EventRow> = rows
        .into_iter()
        .filter(|r| r.thread_id == Some(subscribed_id))
        .collect();
    assert!(
        mine.iter().any(|r| r.event_type == "ToolCalled"),
        "precondition: the await_event call is in the sweep's candidate set"
    );
    let orphans = crate::core::store::find_orphan_tool_called_ids(&mine);
    assert!(
        orphans.is_empty(),
        "await_event pairs its own call, so no synthetic result is owed: {orphans:?}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
