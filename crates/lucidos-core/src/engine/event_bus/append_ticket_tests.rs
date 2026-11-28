use super::*;
use crate::engine::event_bus::test_rows::{emit_row, row_frame};
use crate::engine::event_bus::{BusEvent, EventBus, HistoricalReplay};
use crate::engine::thread_events::EventMeta;
use crate::test_support::{message_received, refuse_event_type, setup_test_db, teardown_test_db};
use uuid::Uuid;

#[test]
fn an_append_split_around_a_subscription_owes_only_its_later_rows() {
    let (event_tx, _keep_open) = broadcast::channel(16);
    let tickets = AppendTickets::default();
    let ticket = tickets.open();
    ticket.learned(5);
    ticket.learned(6);
    ticket.send(&event_tx, row_frame(5));

    let (_receiver, start) = tickets.subscribe(&event_tx);
    ticket.send(&event_tx, row_frame(6));

    assert_eq!(start.floor, Some(6));
    assert_eq!(start.in_flight.len(), 1);
    assert_eq!(start.in_flight[0].sent_since(), vec![6]);
    assert!(!start.in_flight[0].ended());
    drop(ticket);
    assert!(start.in_flight[0].ended());
}

#[test]
fn a_subscription_before_any_learned_sequence_has_no_floor() {
    let (event_tx, _keep_open) = broadcast::channel(16);
    let tickets = AppendTickets::default();
    let _drawing = tickets.open();

    let (_receiver, start) = tickets.subscribe(&event_tx);
    assert_eq!(start.floor, None);
    assert!(start.in_flight.is_empty());
}

#[test]
fn an_ended_append_leaves_the_register() {
    let (event_tx, _keep_open) = broadcast::channel(16);
    let tickets = AppendTickets::default();
    let ticket = tickets.open();
    ticket.learned(3);
    drop(ticket);

    let (_receiver, start) = tickets.subscribe(&event_tx);
    assert_eq!(start.floor, Some(3));
    assert!(start.in_flight.is_empty());
    assert_eq!(tickets.lowest_learned(), Some(3));
}

#[tokio::test]
async fn a_failed_append_ends_its_ticket() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    refuse_event_type(&pool, "MessageReceived").await;

    let refused = bus
        .emit(BusEvent::Thread {
            thread_id: Uuid::new_v4(),
            event: message_received("refused"),
            meta: EventMeta::NONE,
        })
        .await;
    assert!(refused.is_err());
    let (_receiver, start) = bus.append_tickets.subscribe(&bus.event_tx);
    assert!(start.in_flight.is_empty());

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_skipped_backfill_ends_its_ticket() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let payload = serde_json::json!({});
    let replay = || HistoricalReplay {
        event_id: Uuid::nil(),
        aggregate: "test",
        aggregate_id: "backfill",
        event_type: "BuildObserved",
        payload: &payload,
        thread_id: None,
        created: None,
        broadcast: true,
    };
    let inserted = bus.replay_historical_event(replay()).await.unwrap();
    let skipped = bus.replay_historical_event(replay()).await.unwrap();

    assert!(inserted.is_some());
    assert_eq!(skipped, None);
    let (_receiver, start) = bus.append_tickets.subscribe(&bus.event_tx);
    assert!(start.in_flight.is_empty());
    assert_eq!(start.floor, inserted);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_subscription_starts_at_the_last_row_sent() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let seq = emit_row(&bus).await;

    let (_receiver, start) = bus.append_tickets.subscribe(&bus.event_tx);
    assert_eq!(start.floor, Some(seq));
    assert!(start.in_flight.is_empty());

    pool.close().await;
    teardown_test_db(&db).await;
}
