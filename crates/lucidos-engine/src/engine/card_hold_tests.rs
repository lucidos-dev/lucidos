//! While a card is open, no turn starts on its thread (ADR 0803). One test
//! per wake, and the close hands them back.

use super::*;
use crate::core::store::thread_hold::may_start_turn;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::permission::{menu, raise_request, Action, RequestCard};
use crate::engine::thread_events::{AnswerKind, EventChannel, EventMeta};
use crate::test_support::{seed_idle_coding_thread, setup_test_db, teardown_test_db};

async fn raise_card(bus: &EventBus, thread_id: Uuid) -> String {
    let action = Action::CreateWorkspace;
    raise_request(
        bus,
        thread_id,
        RequestCard {
            question: "Create the workspace?".into(),
            header: "Permission",
            action,
            breadth: Some("demo".into()),
            options: menu::options(action, thread_id, "Nothing was created."),
            batch: None,
        },
        EventChannel::ClaudeCode,
    )
    .await
    .unwrap()
}

async fn close(bus: &EventBus, thread_id: Uuid, card: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: card.into(),
            answer: AnswerKind::Superseded,
            quiet: true,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn cards_raised(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 \
           AND event_type IN ('UserQuestionAsked', 'UserQuestionQueued')",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Every engine wake shares one admission: an event-wait delivery, a
/// turn-end nudge, a context-ceiling re-entry and a report read. An open card
/// holds each one and raises nothing more. The close lets them run.
#[tokio::test]
async fn an_open_card_holds_every_engine_step() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_idle_coding_thread(&bus, None).await;
    let card = raise_card(&bus, thread).await;
    assert!(!may_start_turn(&pool, thread).await);
    assert_eq!(
        cards_raised(&pool, thread).await,
        1,
        "a held step asks nothing"
    );

    close(&bus, thread, &card).await;
    assert!(may_start_turn(&pool, thread).await);
    teardown_test_db(&db).await;
}

/// The card's close finds the card it closed by its own event, once. A later
/// event closes nothing more, so held wakes are handed over a single time.
#[tokio::test]
async fn a_close_names_the_card_it_closed_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_idle_coding_thread(&bus, None).await;
    let card = raise_card(&bus, thread).await;
    let posted: i64 = sqlx::query_scalar(
        "SELECT sequence FROM events WHERE thread_id = $1 AND event_type = 'UserQuestionAsked'",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    close(&bus, thread, &card).await;
    let answered: (i64, serde_json::Value) = sqlx::query_as(
        "SELECT sequence, payload FROM events WHERE thread_id = $1 \
           AND event_type = 'UserQuestionAnswered'",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    let event = ThreadEvent::from_stored("UserQuestionAnswered", answered.1).unwrap();
    assert_eq!(
        closed_card_posted_at(&pool, thread, &event, answered.0)
            .await
            .unwrap(),
        Some(posted)
    );
    assert_eq!(
        closed_card_posted_at(&pool, thread, &event, answered.0 + 1)
            .await
            .unwrap(),
        None,
        "nothing was open just before a later event"
    );
    teardown_test_db(&db).await;
}
