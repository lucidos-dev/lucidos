use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
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
///     instead of "Restarted" (device actor's default abort summary) or
///     "Response interrupted" (system actor's default abort summary).
///   - User actor so the chip reads "You" (the user *did* push the button)
///     rather than "⚙ System".
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

    let did_emit = settle_stuck_running_thread(&pool, &bus, thread_id, Some(user_device_actor()))
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
        payload["actor"]["kind"],
        "device",
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
    // First settle transitions running → failed.
    assert!(
        settle_stuck_running_thread(&pool, &bus, thread_id, Some(user_device_actor()))
            .await
            .unwrap()
    );
    // Second settle should be a no-op.
    assert!(
        !settle_stuck_running_thread(&pool, &bus, thread_id, Some(user_device_actor()))
            .await
            .unwrap()
    );

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

    let did_emit =
        settle_stuck_running_thread(&pool, &bus, Uuid::new_v4(), Some(user_device_actor()))
            .await
            .unwrap();
    assert!(!did_emit);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

