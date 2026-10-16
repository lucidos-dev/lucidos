use super::*;
use crate::engine::event_bus::EventBus;
use crate::test_support::{
    flood_past_bus_capacity, seed_chat_thread, seed_thread_event, setup_test_db, teardown_test_db,
};

/// I9. A lag that drops a thread's archive still ends that thread's waits, so
/// none stays live behind the archive for the rescan to deliver.
#[tokio::test]
async fn a_lag_that_drops_an_archive_still_ends_the_threads_waits() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let (mut rx, mut replay) = LagReplay::subscribe(
        &bus,
        ReplayScope::Workspace,
        Some(WAIT_ENDING_EVENT_TYPES.to_vec()),
    );

    let archived = seed_chat_thread(&bus, "archive me").await;
    seed_thread_event(&bus, archived, ThreadEvent::ThreadArchived).await;
    flood_past_bus_capacity(&bus);

    let step = dispatcher_step(rx.recv().await, &mut replay, &pool)
        .await
        .expect("the bus is open");
    assert!(step.rescan, "a lag still rescans every wait");
    assert!(step.offer.is_none());
    assert_eq!(
        step.ended,
        vec![(archived, EventWaitCancelCause::ThreadArchived)]
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A live discard ends its thread's waits, and every live frame, that one
/// included, is still offered to the waits.
#[tokio::test]
async fn a_live_discard_ends_the_threads_waits_and_is_offered() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let (mut rx, mut replay) = LagReplay::subscribe(
        &bus,
        ReplayScope::Workspace,
        Some(WAIT_ENDING_EVENT_TYPES.to_vec()),
    );

    let thread_id = seed_chat_thread(&bus, "hello").await;
    let discard = seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::ThreadDiscarded { actor: None },
    )
    .await;

    let mut ended = Vec::new();
    loop {
        let step = dispatcher_step(rx.recv().await, &mut replay, &pool)
            .await
            .expect("the bus is open");
        assert!(!step.rescan, "nothing lagged");
        ended.extend(step.ended);
        let offered = step.offer.expect("every live frame is offered");
        if offered.event_id == discard {
            break;
        }
    }
    assert_eq!(
        ended,
        vec![(thread_id, EventWaitCancelCause::ThreadDiscarded)]
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The live gate and the replay read the same list, so every event that ends a
/// thread's waits must be on it.
#[test]
fn the_dispatcher_replays_every_event_that_ends_a_threads_waits() {
    let ending = [
        ThreadEvent::ThreadArchived,
        ThreadEvent::ThreadDiscarded { actor: None },
    ];
    for event in &ending {
        assert!(WAIT_ENDING_EVENT_TYPES.contains(&event.event_type()));
        let frame = EmittedEvent {
            event_id: uuid::Uuid::new_v4(),
            seq: None,
            created: Utc::now(),
            typed: BusEvent::Thread {
                thread_id: uuid::Uuid::new_v4(),
                event: event.clone(),
                meta: crate::engine::thread_events::EventMeta::NONE,
            },
            aggregate: None,
            depth: 0,
            emitting_trigger_id: None,
        };
        assert!(wait_ending(&frame).is_some());
    }
    assert_eq!(WAIT_ENDING_EVENT_TYPES.len(), ending.len());
}
