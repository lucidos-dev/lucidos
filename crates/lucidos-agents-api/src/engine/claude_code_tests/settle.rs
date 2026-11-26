use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, AnswerKind, EventChannel, EventMeta, QuestionOption, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};
async fn seed_running_cc_thread(bus: &EventBus, thread_id: Uuid) {
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
            mode: ActorMode::Agent,
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

async fn seed_question_canceled_after_agent_died(bus: &EventBus, thread_id: Uuid) {
    let cc_meta = || EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    let tool_use_id = "tu-settle-test#q0";
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-test".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta(),
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.into(),
            cc_session_id: "sid-test".into(),
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
        meta: cc_meta(),
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: tool_use_id.into(),
            quiet: false,
            answer: AnswerKind::Canceled,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            actor: Some(user_device_actor()),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn read_status(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

fn user_device_actor() -> crate::engine::thread_events::MessageOrigin {
    crate::engine::thread_events::MessageOrigin::Device {
        device_id: "test-device".into(),
    }
}

#[tokio::test]
async fn settle_stuck_running_thread_emits_aborted_stale_settle_with_user_actor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_running_cc_thread(&bus, thread_id).await;
    assert_eq!(
        read_status(&pool, thread_id).await.as_deref(),
        Some("running")
    );

    let did_emit = settle_stuck_running_thread(
        &pool,
        &bus,
        thread_id,
        Some(user_device_actor()),
        SettleTerminal::StuckProjection,
    )
    .await
    .unwrap();
    assert!(did_emit, "stuck running thread should be settled");

    let aborted_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        aborted_count, 1,
        "exactly one ResponseAborted must be persisted"
    );

    let canceled_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseCanceled'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        canceled_count, 0,
        "stale-settle is an abort, not a cancel — no live response existed to cancel"
    );

    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        payload["cause"], "stale_settle",
        "cause must be stale_settle so the summary reads 'Settled stuck response'"
    );
    assert_eq!(
        payload["actor"]["kind"], "device",
        "actor.kind must be 'device' (user from a known device) so the chip reads 'You'"
    );
    assert_eq!(payload["actor"]["device_id"], "test-device");

    assert_eq!(
        read_status(&pool, thread_id).await.as_deref(),
        Some("idle"),
        "stale-settle must use the cancel-style status mapping (idle), not the \
             default ResponseAborted bucket (failed)"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn settle_stuck_running_thread_no_op_when_already_settled() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_running_cc_thread(&bus, thread_id).await;
    assert!(settle_stuck_running_thread(
        &pool,
        &bus,
        thread_id,
        Some(user_device_actor()),
        SettleTerminal::StuckProjection,
    )
    .await
    .unwrap());
    assert!(!settle_stuck_running_thread(
        &pool,
        &bus,
        thread_id,
        Some(user_device_actor()),
        SettleTerminal::StuckProjection,
    )
    .await
    .unwrap());

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "second settle must not emit a duplicate event");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn settle_canceled_question_emits_canceled_user_stop_not_a_stale_abort() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_running_cc_thread(&bus, thread_id).await;
    seed_question_canceled_after_agent_died(&bus, thread_id).await;
    assert_eq!(
        read_status(&pool, thread_id).await.as_deref(),
        Some("running"),
        "precondition: the Canceled answer is what the settle will read as running"
    );

    let did_emit = settle_stuck_running_thread(
        &pool,
        &bus,
        thread_id,
        Some(user_device_actor()),
        SettleTerminal::CanceledQuestion,
    )
    .await
    .unwrap();
    assert!(did_emit, "the canceled question's turn should be settled");

    let aborted_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        aborted_count, 0,
        "a canceled question is not a stale settle: the user ended the turn"
    );

    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseCanceled'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("exactly one ResponseCanceled must be persisted");
    assert_eq!(
        payload["cause"], "user_stop",
        "cause must be user_stop so the summary matches the live-agent interrupt"
    );
    assert_eq!(
        payload["actor"]["kind"], "device",
        "actor.kind must be 'device' so the chip reads 'You'"
    );
    assert_eq!(payload["actor"]["device_id"], "test-device");

    assert_eq!(
        read_status(&pool, thread_id).await.as_deref(),
        Some("idle"),
        "a canceled turn lands idle, same as the abort it replaces"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn settle_canceled_question_no_op_when_already_settled() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_running_cc_thread(&bus, thread_id).await;
    seed_question_canceled_after_agent_died(&bus, thread_id).await;
    assert!(settle_stuck_running_thread(
        &pool,
        &bus,
        thread_id,
        Some(user_device_actor()),
        SettleTerminal::CanceledQuestion,
    )
    .await
    .unwrap());
    assert!(!settle_stuck_running_thread(
        &pool,
        &bus,
        thread_id,
        Some(user_device_actor()),
        SettleTerminal::CanceledQuestion,
    )
    .await
    .unwrap());

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseCanceled'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "second settle must not emit a duplicate event");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn response_aborted_non_stale_settle_lands_in_failed_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_running_cc_thread(&bus, thread_id).await;

    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: crate::engine::thread_events::ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::SafetyNet,
        },
        meta: crate::engine::thread_events::EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(
        read_status(&pool, thread_id).await.as_deref(),
        Some("failed"),
        "non-stale-settle aborts must keep the default 'failed' bucket"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn settle_stuck_running_thread_no_op_for_unknown_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let did_emit = settle_stuck_running_thread(
        &pool,
        &bus,
        Uuid::new_v4(),
        Some(user_device_actor()),
        SettleTerminal::StuckProjection,
    )
    .await
    .unwrap();
    assert!(!did_emit);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_settled_canceled_question_on_a_coding_agent_thread_carries_its_channel() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_running_cc_thread(&bus, thread_id).await;
    seed_question_canceled_after_agent_died(&bus, thread_id).await;
    let did_emit = settle_stuck_running_thread(
        &pool,
        &bus,
        thread_id,
        Some(user_device_actor()),
        SettleTerminal::CanceledQuestion,
    )
    .await
    .unwrap();
    assert!(did_emit);

    let channel: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'channel' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseCanceled'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("exactly one ResponseCanceled must be persisted");
    assert_eq!(channel.as_deref(), Some("claude_code"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}
