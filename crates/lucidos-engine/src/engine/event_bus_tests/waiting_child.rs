//! A child that ends its turn holding an event wait has not finished
//! (ADR 0254).
//!
//! The turn ends because arming a wait and finishing is the event-wait
//! contract. The wait wakes the child for another turn, and that turn reports.
//! Each test pins one invariant of the addendum in
//! `docs/plans/2026-09-23-stopped-child.md`.

use super::super::ChildSettle;
use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::engine::thread_events::EventWaitCancelCause;

async fn emit_wait_started(bus: &EventBus, thread_id: Uuid) -> Uuid {
    let wait_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitStarted {
            wait_id,
            tool_use_id: format!("toolu_{wait_id}"),
            on: vec![EventSubscription {
                event_type: "BenchSlotFreed".into(),
                condition: None,
            }],
            reason: "waiting for the bench slot".into(),
            armed_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            watermark: 0,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    wait_id
}

async fn emit_wait_canceled(
    bus: &EventBus,
    thread_id: Uuid,
    wait_id: Uuid,
    cause: EventWaitCancelCause,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitCanceled {
            wait_id,
            cause,
            on: vec![],
            reason: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// The wake an expiry or a delivery sends: the wait resolves, and the
/// engine's re-entry prompt opens the next turn.
async fn wake_from_wait(bus: &EventBus, thread_id: Uuid, wait_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitExpired { wait_id },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserPromptInjected {
            text: "An event you subscribed to has arrived".into(),
            mode: ActorMode::Engine,
            origin: None,
            injected_message_id: None,
            delivered_event_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn callback_pending(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT parent_callback_pending FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The reported shape. A coding-agent child arms a wait for a shared slot and
/// idles. The idle must not reach the parent as a success.
#[tokio::test]
async fn a_child_that_idles_holding_a_wait_sends_no_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_wait_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, true, None).await;

    assert_eq!(count_completion_cards(&pool, parent_id).await, 0);
    assert_eq!(drain_callbacks(&mut callback_rx), 0);
    assert!(
        callback_pending(&pool, child_id).await,
        "the turn the wait wakes is still owed to the parent"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The wait fires, the child works another turn and finishes. That turn is
/// the one the parent hears about, once.
#[tokio::test]
async fn the_turn_the_wait_wakes_reports_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    let wait_id = emit_wait_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;

    wake_from_wait(&bus, child_id, wait_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;

    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    assert_eq!(
        newest_completion_card(&pool, parent_id).await.0,
        "no_changes"
    );
    assert_eq!(drain_callbacks(&mut callback_rx), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Stop waiting wakes nothing. With no wait left on an idle child, the card it
/// held back is due now, or the parent never hears.
#[tokio::test]
async fn stopping_the_last_wait_on_an_idle_child_sends_the_held_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    let first = emit_wait_started(&bus, child_id).await;
    let second = emit_wait_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;

    emit_wait_canceled(&bus, child_id, first, EventWaitCancelCause::UserStop).await;
    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        0,
        "one wait is still live, so the child can still wake"
    );

    emit_wait_canceled(&bus, child_id, second, EventWaitCancelCause::UserStop).await;
    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    assert_eq!(
        newest_completion_card(&pool, parent_id).await.0,
        "no_changes"
    );
    assert_eq!(drain_callbacks(&mut callback_rx), 1);
    assert!(!callback_pending(&pool, child_id).await);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An agent standing its own wait down mid-turn is not the end of anything:
/// the running turn's own terminal reports.
#[tokio::test]
async fn a_stand_down_on_a_running_child_sends_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    let wait_id = emit_wait_started(&bus, child_id).await;
    emit_wait_canceled(
        &bus,
        child_id,
        wait_id,
        EventWaitCancelCause::AgentStandDown,
    )
    .await;

    assert_eq!(count_completion_cards(&pool, parent_id).await, 0);
    assert_eq!(drain_callbacks(&mut callback_rx), 0);

    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "the turn's own idle reports"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Archiving a waiting child ends it, so the parent gets the canceled card,
/// even while the archive has not yet cancelled the child's wait.
#[tokio::test]
async fn archiving_a_waiting_child_sends_one_canceled_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    let wait_id = emit_wait_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;

    bus.settle_child(child_id, ChildSettle::Archived).await;
    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    assert_eq!(newest_completion_card(&pool, parent_id).await.0, "canceled");
    assert_eq!(drain_callbacks(&mut callback_rx), 1);

    // The archive then stands the wait down. That owes nothing more.
    emit_wait_canceled(
        &bus,
        child_id,
        wait_id,
        EventWaitCancelCause::ThreadArchived,
    )
    .await;
    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A discard of a waiting child's change does not end the child: the wait
/// still wakes it, and that turn reports.
#[tokio::test]
async fn discarding_a_waiting_childs_change_sends_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_wait_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, true, None).await;

    bus.settle_child(child_id, ChildSettle::Discarded).await;
    assert_eq!(count_completion_cards(&pool, parent_id).await, 0);
    assert_eq!(drain_callbacks(&mut callback_rx), 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A failed turn reports at once, wait or not: a failure is worth knowing
/// whatever the wait brings later.
#[tokio::test]
async fn a_failed_turn_holding_a_wait_still_reports() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_wait_started(&bus, child_id).await;
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseFailed {
            error: "the build broke".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    assert_eq!(newest_completion_card(&pool, parent_id).await.0, "failure");
    assert_eq!(drain_callbacks(&mut callback_rx), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A child that crashed while holding a wait is not idle. Ending its last
/// wait must not report its crashed turn as a success.
#[tokio::test]
async fn stopping_the_last_wait_on_a_crashed_child_sends_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    let wait_id = emit_wait_started(&bus, child_id).await;
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::SafetyNet,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    emit_wait_canceled(&bus, child_id, wait_id, EventWaitCancelCause::UserStop).await;
    assert_eq!(count_completion_cards(&pool, parent_id).await, 0);
    assert_eq!(drain_callbacks(&mut callback_rx), 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
