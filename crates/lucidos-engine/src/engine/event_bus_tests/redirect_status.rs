//! A redirect cancel keeps the thread `running`
//! (`docs/plans/2026-10-20-a-redirect-keeps-the-thread-running.md`).
//!
//! The engine interrupts the turn to carry the work on. The gap before the
//! redirected turn starts is therefore not idle. A user Stop is the control:
//! it ends the work and settles the thread.

use super::*;
use crate::engine::thread_events::{CancelCause, SessionEndReason};

async fn status_of(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn emit_session_ended(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionEnded {
            reason: SessionEndReason::Panic,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn emit_continuation_requested(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ContinuationRequested {
            reason: crate::engine::agent_recovery::USER_CLICKED_CONTINUE_REASON.to_string(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// A Claude Code thread mid-turn.
async fn running_cc_thread(bus: &EventBus, pool: &PgPool) -> Uuid {
    let thread_id = Uuid::new_v4();
    emit_cc_message_received(bus, thread_id, None, "fix it").await;
    emit_cc_session_started(bus, thread_id).await;
    assert_eq!(status_of(pool, thread_id).await, "running");
    thread_id
}

/// The effort-restart event order: cancel, the run's idle, the session end,
/// then the resume. Each step before the resume must leave the thread working.
#[tokio::test]
async fn a_redirect_keeps_the_thread_running_until_the_redirected_turn_ends() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    for cause in CancelCause::ALL.into_iter().filter(|c| c.is_redirect()) {
        let thread_id = running_cc_thread(&bus, &pool).await;

        emit_response_canceled_with_cause(&bus, thread_id, cause).await;
        assert_eq!(
            status_of(&pool, thread_id).await,
            "running",
            "a {cause:?} cancel does not end the thread's work"
        );

        emit_cc_idled(&bus, thread_id, false, None).await;
        assert_eq!(
            status_of(&pool, thread_id).await,
            "running",
            "the interrupted run's idle, after a {cause:?} cancel"
        );

        emit_session_ended(&bus, thread_id).await;
        assert_eq!(
            status_of(&pool, thread_id).await,
            "running",
            "the interrupted run's session end, after a {cause:?} cancel"
        );

        emit_continuation_requested(&bus, thread_id).await;
        assert_eq!(status_of(&pool, thread_id).await, "running");

        emit_cc_idled(&bus, thread_id, false, None).await;
        assert_eq!(
            status_of(&pool, thread_id).await,
            "idle",
            "the redirected turn's own idle settles the thread"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A user Stop is not a redirect: the cancel settles the thread, and the idle
/// after it keeps it settled.
#[tokio::test]
async fn a_user_stop_still_settles_the_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    for cause in CancelCause::ALL.into_iter().filter(|c| !c.is_redirect()) {
        let thread_id = running_cc_thread(&bus, &pool).await;

        emit_response_canceled_with_cause(&bus, thread_id, cause).await;
        assert_eq!(
            status_of(&pool, thread_id).await,
            "idle",
            "a {cause:?} cancel"
        );

        emit_cc_idled(&bus, thread_id, false, None).await;
        emit_session_ended(&bus, thread_id).await;
        assert_eq!(status_of(&pool, thread_id).await, "idle");
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The hold lasts until the next start event. A user message that lands in the
/// gap starts a turn of its own, and that turn's idle settles the thread.
#[tokio::test]
async fn a_start_event_ends_the_redirect_hold() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = running_cc_thread(&bus, &pool).await;
    emit_response_canceled_with_cause(&bus, thread_id, CancelCause::EffortRestart).await;
    emit_cc_message_received(&bus, thread_id, None, "and also this").await;
    emit_cc_idled(&bus, thread_id, false, None).await;

    assert_eq!(status_of(&pool, thread_id).await, "idle");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
