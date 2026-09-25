//! A re-entry that lands while a question is open waits behind the answer, so
//! it must not move the thread off `waiting_for_user_answer`. See
//! `docs/plans/2026-09-24-a-delivery-never-unparks-a-question.md`.

use super::super::*;
use super::*;

async fn emit_question(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: "tu-1".into(),
            cc_session_id: String::new(),
            question: "How should I handle the proxy timeout?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// The anchor an event-wait delivery persists before queueing its re-entry.
async fn emit_delivery_anchor(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserPromptInjected {
            text: "An event you subscribed to has arrived".into(),
            mode: ActorMode::Agent,
            origin: None,
            injected_message_id: None,
            delivered_event_id: Some(Uuid::new_v4()),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn read_status(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_delivery_keeps_a_thread_waiting_for_its_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit_thread_message(&bus, thread_id, None, "run the loop").await;
    emit_question(&bus, thread_id).await;
    emit_delivery_anchor(&bus, thread_id).await;

    assert_eq!(
        read_status(&pool, thread_id).await,
        "waiting_for_user_answer",
        "the delivery waits behind the answer, so the question still needs \
         the user: `running` would put out its needs-attention badge"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_delivery_wakes_an_idle_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    emit_thread_message(&bus, thread_id, None, "run the loop").await;
    sqlx::query("UPDATE thread_summaries SET status = 'idle' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();
    emit_delivery_anchor(&bus, thread_id).await;

    assert_eq!(read_status(&pool, thread_id).await, "running");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
