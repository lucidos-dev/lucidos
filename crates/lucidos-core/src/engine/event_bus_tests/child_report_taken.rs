//! A child report the parent's agent took mid-turn is handled, so the boot
//! sweep does not wake the parent for it again.

use super::super::*;
use super::*;

/// A coding-agent parent whose child just finished. Returns the parent and
/// its completion card.
async fn parent_with_a_finished_child(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    callback_rx: &mut tokio::sync::mpsc::UnboundedReceiver<ParentCallback>,
) -> (Uuid, Uuid) {
    let (parent_id, child_id) = (Uuid::new_v4(), Uuid::new_v4());
    start_cc_session(bus, parent_id, "parent", None).await;
    spawn_child_of(bus, parent_id, child_id, EventChannel::ClaudeCode, None).await;
    emit_cc_session_started(bus, child_id).await;
    emit_cc_idle(bus, child_id, false, None).await;
    assert_eq!(drain_callbacks(callback_rx), 1, "baseline: one live wake");
    let card_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM events WHERE thread_id = $1 AND event_type = 'ChildThreadCompleted'",
    )
    .bind(parent_id)
    .fetch_one(pool)
    .await
    .unwrap();
    (parent_id, card_id)
}

/// The control: the same card with no take is re-fired.
#[tokio::test]
async fn an_untaken_report_is_refired_after_a_restart() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    parent_with_a_finished_child(&bus, &pool, &mut callback_rx).await;
    drop(callback_rx);
    drop(bus);

    let (bus2, _rx2) = EventBus::new(pool.clone());
    assert_eq!(bus2.refire_unprocessed_child_completions().await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The card followed by its `ChildReportTaken`: the sweep selects nothing.
#[tokio::test]
async fn a_taken_report_is_not_refired_after_a_restart() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, card_id) = parent_with_a_finished_child(&bus, &pool, &mut callback_rx).await;
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::ChildReportTaken {
            input_event_id: card_id,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    drop(callback_rx);
    drop(bus);

    let (bus2, mut rx2) = EventBus::new(pool.clone());
    assert_eq!(bus2.refire_unprocessed_child_completions().await, 0);
    assert_eq!(drain_callbacks(&mut rx2), 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
