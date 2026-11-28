use super::PreEmittedOrigin;
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::engine::LucidosEngine;
use crate::test_support::{
    boot_mock_engine, message_received, run_on_engine_runtime, seed_chat_thread, setup_test_db,
    teardown_test_db,
};
use uuid::Uuid;

async fn remove(engine: &LucidosEngine, thread_id: Uuid, message: Uuid) {
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::QueuedMessageRemoved {
                removed_message_id: message,
                restore_to_compose: false,
                compose: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
}

async fn submit(
    engine: &LucidosEngine,
    thread_id: Uuid,
    message: Uuid,
    pre_emitted_origin: Option<PreEmittedOrigin>,
) {
    let event_id = message.to_string();
    engine
        .process_message_with_steps(
            "taken back",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(false),
            Some(&event_id),
            Some(thread_id),
            None,
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            pre_emitted_origin,
            None,
            None,
            None,
            crate::engine::FollowUpUrgency::Normal,
            None,
            None,
        )
        .await
        .expect("a message the user took back ends its run without an error");
}

async fn count(
    engine: &LucidosEngine,
    thread_id: Uuid,
    event_type: &str,
    message: Option<Uuid>,
) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = $2 \
           AND ($3::uuid IS NULL OR id = $3 \
             OR payload->>'request_event_id' = $3::text \
             OR payload->>'injected_message_id' = $3::text)",
    )
    .bind(thread_id)
    .bind(event_type)
    .bind(message)
    .fetch_one(engine.pool())
    .await
    .unwrap()
}

async fn assert_quiet(engine: &LucidosEngine, thread_id: Uuid) {
    assert_eq!(count(engine, thread_id, "ResponseFailed", None).await, 0);
    assert_eq!(count(engine, thread_id, "ResponseGenerated", None).await, 0);
    assert!(!engine.processing_thread_ids().contains(&thread_id));
}

#[test]
fn a_message_taken_back_before_recording_never_records() {
    run_on_engine_runtime(
        "engine::chat::process::withdrawn_tests::a_message_taken_back_before_recording_never_records",
        unrecorded_case,
    );
}

async fn unrecorded_case() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let thread_id = seed_chat_thread(&engine.event_bus, "hello").await;
    let message = Uuid::new_v4();
    remove(&engine, thread_id, message).await;

    submit(&engine, thread_id, message, None).await;

    assert_eq!(
        count(&engine, thread_id, "MessageReceived", Some(message)).await,
        0
    );
    assert_eq!(
        count(&engine, thread_id, "ChatTurnSettingsResolved", None).await,
        0
    );
    assert_quiet(&engine, thread_id).await;
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[test]
fn a_recorded_follow_up_taken_back_after_its_turn_ended_anchors_no_turn() {
    run_on_engine_runtime(
        "engine::chat::process::withdrawn_tests::a_recorded_follow_up_taken_back_after_its_turn_ended_anchors_no_turn",
        recorded_follow_up_case,
    );
}

async fn recorded_follow_up_case() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let thread_id = seed_chat_thread(&engine.event_bus, "hello").await;
    let message = Uuid::new_v4();
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: message_received("follow-up"),
            meta: EventMeta {
                event_id: Some(message),
                channel: Some(EventChannel::Chat),
                request_event_id: Some(Uuid::new_v4()),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    remove(&engine, thread_id, message).await;

    submit(
        &engine,
        thread_id,
        message,
        Some(PreEmittedOrigin::Message(message)),
    )
    .await;

    assert_eq!(
        count(
            &engine,
            thread_id,
            "ChatTurnSettingsResolved",
            Some(message)
        )
        .await,
        0
    );
    assert_quiet(&engine, thread_id).await;
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[test]
fn a_message_taken_back_never_answers_an_open_card() {
    run_on_engine_runtime(
        "engine::chat::process::withdrawn_tests::a_message_taken_back_never_answers_an_open_card",
        open_card_case,
    );
}

async fn open_card_case() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let thread_id = seed_chat_thread(&engine.event_bus, "hello").await;
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::UserQuestionAsked {
                tool_use_id: "toolu_open".into(),
                cc_session_id: String::new(),
                question: "Which one?".into(),
                options: vec![],
                worktree_path: None,
                multi_select: false,
                owner_approval: None,
                permission_request: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    let message = Uuid::new_v4();
    remove(&engine, thread_id, message).await;

    submit(&engine, thread_id, message, None).await;

    assert_eq!(
        count(&engine, thread_id, "UserQuestionAnswered", None).await,
        0
    );
    assert_eq!(
        count(&engine, thread_id, "MessageReceived", Some(message)).await,
        0
    );
    assert_quiet(&engine, thread_id).await;
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

async fn status(engine: &LucidosEngine, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(engine.pool())
        .await
        .unwrap()
}

async fn record_follow_up(engine: &LucidosEngine, thread_id: Uuid) -> Uuid {
    let message = Uuid::new_v4();
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: message_received("follow-up"),
            meta: EventMeta {
                event_id: Some(message),
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    message
}

#[test]
fn a_starter_taken_back_after_the_last_turn_ended_settles_the_thread() {
    run_on_engine_runtime(
        "engine::chat::process::withdrawn_tests::a_starter_taken_back_after_the_last_turn_ended_settles_the_thread",
        settles_case,
    );
}

async fn settles_case() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let thread_id = seed_chat_thread(&engine.event_bus, "hello").await;
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseGenerated {
                text: "hi".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    let message = record_follow_up(&engine, thread_id).await;
    assert_eq!(status(&engine, thread_id).await, "running");

    remove(&engine, thread_id, message).await;
    submit(
        &engine,
        thread_id,
        message,
        Some(PreEmittedOrigin::Message(message)),
    )
    .await;

    assert_eq!(status(&engine, thread_id).await, "idle");
    assert_eq!(count(&engine, thread_id, "ResponseFailed", None).await, 0);
    assert_eq!(
        count(&engine, thread_id, "ResponseGenerated", None).await,
        1,
        "only the earlier turn replied"
    );
    assert!(!engine.processing_thread_ids().contains(&thread_id));
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[test]
fn a_follow_up_taken_back_during_a_live_turn_keeps_it_running() {
    run_on_engine_runtime(
        "engine::chat::process::withdrawn_tests::a_follow_up_taken_back_during_a_live_turn_keeps_it_running",
        live_turn_case,
    );
}

async fn live_turn_case() {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let thread_id = seed_chat_thread(&engine.event_bus, "hello").await;
    let message = record_follow_up(&engine, thread_id).await;

    remove(&engine, thread_id, message).await;

    assert_eq!(status(&engine, thread_id).await, "running");
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[test]
fn a_typed_message_waits_for_the_permission_answer_and_runs_once() {
    run_on_engine_runtime(
        "engine::chat::process::withdrawn_tests::a_typed_message_waits_for_the_permission_answer_and_runs_once",
        permission_held_case,
    );
}

async fn permission_held_case() {
    use crate::engine::thread_events::MessageOrigin;
    use crate::engine::thread_queue::{ExecutableEntry, ThreadQueueRequest};
    use serde_json::json;

    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let thread_id = seed_chat_thread(&engine.event_bus, "hello").await;
    let message = Uuid::new_v4();
    let request_id = Uuid::new_v4().to_string();
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CommandPermissionRequested {
                request_id: request_id.clone(),
                tool_use_id: "permission-tool".into(),
                tool_name: "bash".into(),
                command: "pwd".into(),
                summary: "Read the working directory".into(),
                decision_trace: None,
                grant_head: None,
                grant_offer: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();

    let request: ThreadQueueRequest = serde_json::from_value(json!({
        "type": "agent-chat",
        "thread_id": thread_id,
        "event_id": message,
        "message": "Please check the totals",
        "mode": "human",
        "use_coding_agent": false,
        "origin": MessageOrigin::Device { device_id: "test-device".into() },
    }))
    .unwrap();
    let task = tokio::spawn(engine.clone().execute_thread_queue_entry(ExecutableEntry {
        id: Uuid::new_v4(),
        request,
    }));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while count(&engine, thread_id, "MessageReceived", Some(message)).await == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the message becomes visible while the card waits");
    assert!(!task.is_finished());
    assert_eq!(
        count(&engine, thread_id, "CommandPermissionResolved", None).await,
        0
    );
    assert_eq!(
        count(
            &engine,
            thread_id,
            "ChatTurnSettingsResolved",
            Some(message)
        )
        .await,
        0
    );

    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CommandPermissionResolved {
                request_id,
                allowed: false,
                reason: None,
                persist_scope: None,
                standing_deny: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15), task)
        .await
        .expect("answering releases the send")
        .unwrap()
        .unwrap();
    assert_eq!(
        count(&engine, thread_id, "MessageReceived", Some(message)).await,
        1
    );
    assert_eq!(
        count(&engine, thread_id, "ResponseGenerated", Some(message)).await,
        1
    );
    engine.pool().close().await;
    teardown_test_db(&db).await;
}
