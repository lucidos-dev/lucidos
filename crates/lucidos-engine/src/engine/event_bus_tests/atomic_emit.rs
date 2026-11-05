//! An atomic emit writes all of its events in one transaction, or none.

use super::*;
use crate::test_support::refuse_event_type;

/// Split a `thread_message` into the pair `emit_atomically` takes.
fn message_pair(thread_id: Uuid, text: &str) -> (ThreadEvent, EventMeta) {
    match thread_message(thread_id, None, text) {
        BusEvent::Thread { event, meta, .. } => (event, meta),
        BusEvent::System(_) => unreachable!("thread_message builds a thread event"),
    }
}

fn understanding() -> (ThreadEvent, EventMeta) {
    (
        ThreadEvent::WorkingUnderstandingWritten {
            document: "the job so far".into(),
        },
        EventMeta::NONE,
    )
}

async fn event_count(pool: &PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn message_count(pool: &PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT message_count::bigint FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_refused_second_event_takes_the_first_with_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;
    refuse_event_type(&pool, "WorkingUnderstandingWritten").await;

    let outcome = bus
        .emit_atomically(
            thread_id,
            vec![message_pair(thread_id, "first"), understanding()],
        )
        .await;

    assert!(outcome.is_err(), "the refused event must fail the call");
    assert_eq!(
        event_count(&pool, thread_id).await,
        1,
        "no row may stand alone"
    );
    assert_eq!(
        message_count(&pool, thread_id).await,
        1,
        "the first event's projection must roll back too"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn each_event_broadcasts_in_order_with_its_own_snapshot() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;
    let mut rx = bus.subscribe();

    let results = bus
        .emit_atomically(
            thread_id,
            vec![
                message_pair(thread_id, "one"),
                message_pair(thread_id, "two"),
            ],
        )
        .await
        .unwrap()
        .expect("nothing suppresses a message");

    let mut seen = Vec::new();
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::Thread {
            event: ThreadEvent::MessageReceived { text, .. },
            ..
        } = &emitted.typed
        {
            let count = emitted.aggregate.as_ref().map(|a| a.message_count);
            seen.push((emitted.event_id, emitted.seq, text.clone(), count));
        }
    }
    let expected: Vec<_> = results
        .iter()
        .zip([("one", 2), ("two", 3)])
        .map(|(r, (text, count))| (r.event_id, Some(r.seq), text.to_string(), Some(count)))
        .collect();
    assert_eq!(seen, expected);
    assert!(results[0].seq < results[1].seq);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_suppressed_event_drops_the_whole_atomic_emit() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;

    // No read was requested, so Validate drops this sighting.
    let stale_sighting = (
        ThreadEvent::ThreadReplySeen { seen_version: 999 },
        EventMeta::NONE,
    );
    let outcome = bus
        .emit_atomically(thread_id, vec![understanding(), stale_sighting])
        .await
        .unwrap();

    assert!(outcome.is_none());
    assert_eq!(event_count(&pool, thread_id).await, 1);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_transient_event_cannot_join_an_atomic_emit() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;

    let outcome = bus
        .emit_atomically(
            thread_id,
            vec![
                understanding(),
                (ThreadEvent::PreambleCompleted, EventMeta::NONE),
            ],
        )
        .await;

    assert!(outcome.is_err());
    assert_eq!(event_count(&pool, thread_id).await, 1);
    pool.close().await;
    teardown_test_db(&db_name).await;
}
