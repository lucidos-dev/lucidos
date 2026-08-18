use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, AnswerKind, EventChannel, EventMeta, QuestionOption, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};
/// Emit MessageReceived for a CC-channel thread → status='running'.
/// Mirrors what `spawn_agent_thread` does before kicking off the bg task.
async fn seed_running_cc_thread(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "do the thing".into(),
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
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// Replay the production shape of the reported bug: a coding-agent thread asks
/// a question, its subprocess goes away, and hours later the user cancels the
/// card. The `Canceled` answer is what puts the projection back at `running`,
/// which is the state the settle then reads. `SessionStarted` leads, because
/// the lifecycle validator wants it ahead of any other CC-channel event.
async fn seed_question_canceled_after_agent_died(bus: &EventBus, thread_id: Uuid) {
    let cc_meta = || EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    let tool_use_id = "tu-settle-test#q0";
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-test".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
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
            }],
            worktree_path: None,
            multi_select: false,
        },
        meta: cc_meta(),
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: tool_use_id.into(),
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
        label: "Test Device".into(),
    }
}

/// User clicks Stop / Apply / Discard / Archive / Interrupt on a CC thread
/// that's stuck at status='running' (the background spawn task errored before
/// any terminal event could fire, or the Claude Code subprocess hadn't yet registered
/// in agent_sessions when the user pressed the button). The settle helper
/// emits `ResponseAborted` with `AbortCause::StaleSettle` and the user actor:
///   - `Aborted` (not `Canceled`) because no live response existed to cancel
///     — this is system-driven cleanup of stuck projection state.
///   - `cause=StaleSettle` so the frontend renders "Settled stuck response"
///     instead of "Paused by restart" (device actor's default abort summary) or
///     "Response interrupted" (system actor's default abort summary).
///   - User actor so the chip reads "You" (the user *did* push the button)
///     rather than "System".
///   - Thread status lands at `idle` (not `failed`): the projection branches
///     on `cause=StaleSettle` to use the cancel-style status mapping, so the
///     thread list doesn't show a red error indicator on a thread the user
///     just deliberately settled.
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

    // Exactly one ResponseAborted, zero ResponseCanceled — moving stale-settle
    // to abort means no ghost "Canceled the response" appears on a thread that
    // wasn't actually mid-response.
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

    // Thread status: stale-settle must land at `idle`, not the default
    // ResponseAborted bucket of `failed`. Otherwise the thread list shows a
    // red error indicator on a thread the user just deliberately settled.
    assert_eq!(
        read_status(&pool, thread_id).await.as_deref(),
        Some("idle"),
        "stale-settle must use the cancel-style status mapping (idle), not the \
             default ResponseAborted bucket (failed)"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Idempotency: settling a thread that's already non-running is a no-op
/// (so that double-clicks on the stop button don't pile up events).
#[tokio::test]
async fn settle_stuck_running_thread_no_op_when_already_settled() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_running_cc_thread(&bus, thread_id).await;
    // First settle transitions running → idle (the StaleSettle cause routes to
    // the cancel-style status mapping; see the test above).
    assert!(settle_stuck_running_thread(
        &pool,
        &bus,
        thread_id,
        Some(user_device_actor()),
        SettleTerminal::StuckProjection,
    )
    .await
    .unwrap());
    // Second settle should be a no-op.
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

/// The user cancels a question card on a thread whose agent is long gone. Every
/// step is the same as the stale-settle test above except the reason, and the
/// reason is the whole point: this `running` row is the cancel's OWN write, so
/// the turn ended by the user's hand and reads that way.
///
///   - `ResponseCanceled` (not `Aborted`), and `cause=UserStop`, so the summary
///     matches what a live agent's interrupt already produces. The same click
///     used to say "Settled stuck response" here and "Response canceled" on a
///     live thread.
///   - User actor, so the chip reads "You".
///   - Thread status lands at `idle`, exactly as the abort did.
#[tokio::test]
async fn settle_canceled_question_emits_canceled_user_stop_not_a_stale_abort() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_running_cc_thread(&bus, thread_id).await;
    seed_question_canceled_after_agent_died(&bus, thread_id).await;
    // The cancel-stamp itself is what leaves the projection `running`: the
    // status table maps `UserQuestionAnswered` to Running whatever the kind.
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

/// Idempotency for the cancel variant: the `running` re-check guards both
/// terminals, so a double-tapped Cancel cannot stack two `ResponseCanceled`
/// rows any more than it could stack two aborts.
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

/// Contrast test: a real (non-stale-settle) `ResponseAborted` still lands in
/// the `failed` bucket. The stale-settle special case in the projection
/// (`ResponseAborted { cause: StaleSettle }` → idle) must not over-apply to
/// other abort causes — engine shutdowns, safety-net crashes, etc. still
/// surface the red error indicator on the thread list.
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

/// A thread that the projection never knew about (no thread_summaries row)
/// is also a no-op — interrupt of an unknown id should not emit phantom
/// events for non-existent threads.
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
