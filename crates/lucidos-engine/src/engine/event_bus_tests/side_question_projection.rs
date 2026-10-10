//! A side question is a card beside the thread, never a turn (ADR 0320). Its
//! four events persist and change no column of the thread's summary row.

use super::super::*;
use super::*;

async fn summary_row(pool: &PgPool, thread_id: Uuid) -> serde_json::Value {
    sqlx::query_scalar("SELECT to_jsonb(ts) FROM thread_summaries ts WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn record(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn record_every_side_question_event(bus: &EventBus, thread_id: Uuid) {
    let side_question_id = Uuid::new_v4();
    record(
        bus,
        thread_id,
        ThreadEvent::SideQuestionAsked {
            side_question_id,
            question: "what does this function return?".into(),
            image_hashes: vec![],
        },
    )
    .await;
    record(
        bus,
        thread_id,
        ThreadEvent::SideQuestionAnswered {
            side_question_id,
            answer: "A string.".into(),
        },
    )
    .await;
    record(
        bus,
        thread_id,
        ThreadEvent::SideQuestionFailed {
            side_question_id: Uuid::new_v4(),
            error: "no answer".into(),
        },
    )
    .await;
    record(
        bus,
        thread_id,
        ThreadEvent::SideQuestionDismissed { side_question_id },
    )
    .await;
}

#[tokio::test]
async fn a_side_question_changes_no_column_of_a_running_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "", None).await;
    emit_cc_message_received(&bus, thread_id, None, "do the work").await;
    let before = summary_row(&pool, thread_id).await;
    assert_eq!(before["status"], "running");

    record_every_side_question_event(&bus, thread_id).await;

    assert_eq!(summary_row(&pool, thread_id).await, before);
    let persisted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type LIKE 'SideQuestion%'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, 4, "every side-question event must persist");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_side_question_changes_no_column_of_an_archived_idle_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "", None).await;
    emit_cc_message_received(&bus, thread_id, None, "do the work").await;
    emit_cc_idle(&bus, thread_id, false, None).await;
    record(&bus, thread_id, ThreadEvent::ThreadArchived).await;
    let before = summary_row(&pool, thread_id).await;
    assert_eq!(before["archive_state"], "archived");

    record_every_side_question_event(&bus, thread_id).await;

    assert_eq!(summary_row(&pool, thread_id).await, before);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The boot sweep re-fires a completion card that is the parent's last word.
/// A side question after it is no reaction to the child, so the card stays
/// unprocessed and is re-fired.
#[tokio::test]
async fn a_side_question_does_not_hide_an_unprocessed_card_from_the_boot_sweep() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_cc_idle(&bus, child_id, false, None).await;
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        1,
        "baseline: one live wake"
    );
    record_every_side_question_event(&bus, parent_id).await;
    drop(callback_rx);
    drop(bus);

    let (bus2, mut rx2) = EventBus::new(pool.clone());
    assert_eq!(bus2.refire_unprocessed_child_completions().await, 1);
    assert_eq!(drain_callbacks(&mut rx2), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
