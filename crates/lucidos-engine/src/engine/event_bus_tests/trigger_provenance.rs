//! An events row records what the trigger matcher read off its broadcast: the
//! dispatch depth and the emitting trigger. A lagged scheduler replays the row
//! and must decide exactly as the live frame did.

use super::super::*;
use super::*;
use crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH;

/// The two provenance columns of one row, read back as the matcher sees them.
async fn stored_provenance(pool: &PgPool, event_id: Uuid) -> (Option<i32>, Option<String>) {
    sqlx::query_as("SELECT trigger_chain_depth, emitting_trigger_id FROM events WHERE id = $1")
        .bind(event_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Emit `event` as trigger `t-1`'s fire, at chain depth 2, and return the row.
async fn emit_inside_a_fire(bus: &EventBus, event: BusEvent) -> Uuid {
    EVENT_TRIGGER_DEPTH
        .scope(2, bus.emit_as_trigger(event, Some("t-1".into())))
        .await
        .unwrap()
        .expect("persisted")
        .event_id
}

#[tokio::test]
async fn a_system_row_records_the_fires_depth_and_trigger() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let event_id = emit_inside_a_fire(
        &bus,
        BusEvent::System(SystemEvent::TriggerExecuted {
            trigger_id: "t-1".into(),
            payload: serde_json::json!({}),
        }),
    )
    .await;

    assert_eq!(
        stored_provenance(&pool, event_id).await,
        (Some(2), Some("t-1".to_string()))
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_thread_row_records_the_fires_depth_and_trigger() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let event_id = emit_inside_a_fire(&bus, thread_message(Uuid::new_v4(), None, "fired")).await;

    assert_eq!(
        stored_provenance(&pool, event_id).await,
        (Some(2), Some("t-1".to_string()))
    );
    teardown_test_db(&db).await;
}

/// A domain event dispatches at its variant's depth, not the frame's, so the
/// row records the variant's.
#[tokio::test]
async fn a_domain_row_records_its_variants_depth() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let event_id = bus
        .emit(BusEvent::System(SystemEvent::DomainEvent {
            event_type: "BuildObserved".into(),
            payload: serde_json::json!({"summary": "green"}),
            depth: 1,
            transient: false,
            actor: None,
        }))
        .await
        .unwrap()
        .expect("persisted")
        .event_id;

    assert_eq!(stored_provenance(&pool, event_id).await, (Some(1), None));
    teardown_test_db(&db).await;
}

/// Outside any fire both columns stay `NULL`, which is how every older row
/// reads too.
#[tokio::test]
async fn an_ordinary_row_records_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let event_id = bus
        .emit(thread_message(Uuid::new_v4(), None, "an ordinary turn"))
        .await
        .unwrap()
        .expect("persisted")
        .event_id;

    assert_eq!(stored_provenance(&pool, event_id).await, (None, None));
    teardown_test_db(&db).await;
}
