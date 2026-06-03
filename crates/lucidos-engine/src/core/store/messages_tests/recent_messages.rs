//! DB-backed regression tests for `EventStore::get_recent_messages`.
//!
//! Unlike the sibling pure-function tests of `build_session_messages`, these
//! exercise the actual SQL CTE against a real Postgres, because the bug under
//! test lives entirely in the query's `ORDER BY`.

use crate::core::store::EventStore;
use crate::test_support::{setup_test_db, teardown_test_db};
use chrono::{DateTime, TimeZone, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Insert a `MessageReceived` event for `thread_id` stamped at `created`. The
/// explicit timestamp is what lets each test decouple a thread's recency from
/// its UUID's lexicographic order.
async fn insert_message_at(pool: &PgPool, thread_id: Uuid, created: DateTime<Utc>, text: &str) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, thread_id, aggregate, aggregate_id) \
         VALUES ($1, 'MessageReceived', $2, $3, $4, 'thread', $4::text)",
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({ "text": text }))
    .bind(created)
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("insert MessageReceived");
}

/// Regression for the `recent_threads` CTE ordering bug: it did
/// `ORDER BY thread_id` (UUID lexicographic) instead of by recency, so
/// `get_recent_messages` returned an arbitrary thread set rather than the
/// most-recently-active threads. This read feeds both `GET /api/v1/messages`
/// and new-thread chat-context loading, so leaking the wrong threads in is a
/// real correctness bug, not a cosmetic one.
///
/// The three threads are built so UUID order is the EXACT REVERSE of recency:
///   - `oldest` → UUID sorts FIRST  (0000…0001), activity OLDEST
///   - `middle` → UUID sorts MIDDLE (8888…),      activity MIDDLE
///   - `newest` → UUID sorts LAST   (ffff…),      activity NEWEST
///
/// With `limit = 2`:
///   - Buggy `ORDER BY thread_id`        picks the two UUID-smallest → {oldest, middle}
///   - Correct `ORDER BY MAX(created) DESC` picks the two most recent → {newest, middle}
///
/// Asserting `newest` is present and `oldest` is absent fails loudly on the old
/// ordering and passes only on the recency fix. A test that would pass under
/// both orderings would be useless here, so the assertions pin recency directly.
#[tokio::test]
async fn get_recent_messages_selects_most_recent_threads_not_uuid_order() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let oldest = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let middle = Uuid::parse_str("88888888-8888-8888-8888-888888888888").unwrap();
    let newest = Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff").unwrap();

    let base = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    // Recency ascending: oldest < middle < newest.
    insert_message_at(&pool, oldest, base, "oldest thread").await;
    insert_message_at(&pool, middle, base + chrono::Duration::hours(1), "middle thread").await;
    insert_message_at(&pool, newest, base + chrono::Duration::hours(2), "newest thread").await;

    let messages = store
        .get_recent_messages(2, None)
        .await
        .expect("get_recent_messages");

    let returned: std::collections::HashSet<String> =
        messages.iter().filter_map(|m| m.thread_id.clone()).collect();

    assert!(
        returned.contains(&newest.to_string()),
        "the most-recently-active thread must be selected; \
         ORDER BY thread_id wrongly drops it. got {:?}",
        returned
    );
    assert!(
        !returned.contains(&oldest.to_string()),
        "the least-recently-active thread must NOT be selected under limit=2; got {:?}",
        returned
    );
    assert_eq!(
        returned,
        std::collections::HashSet::from([newest.to_string(), middle.to_string()]),
        "exactly the two most-recently-active threads must come back; got {:?}",
        returned
    );

    teardown_test_db(&db).await;
}

/// The `before` cursor branch shares the same CTE and must order by recency
/// too. With a cutoff above all three messages, `limit = 1` must return the
/// single most-recent thread (`newest`), never the UUID-smallest (`oldest`).
#[tokio::test]
async fn get_recent_messages_before_cursor_orders_by_recency() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let oldest = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let middle = Uuid::parse_str("88888888-8888-8888-8888-888888888888").unwrap();
    let newest = Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff").unwrap();

    let base = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    insert_message_at(&pool, oldest, base, "oldest thread").await;
    insert_message_at(&pool, middle, base + chrono::Duration::hours(1), "middle thread").await;
    insert_message_at(&pool, newest, base + chrono::Duration::hours(2), "newest thread").await;

    let cutoff = base + chrono::Duration::hours(3);
    let messages = store
        .get_recent_messages(1, Some(cutoff))
        .await
        .expect("get_recent_messages before cutoff");

    let returned: std::collections::HashSet<String> =
        messages.iter().filter_map(|m| m.thread_id.clone()).collect();

    assert_eq!(
        returned,
        std::collections::HashSet::from([newest.to_string()]),
        "before-cursor branch must select the single most-recent thread; got {:?}",
        returned
    );

    teardown_test_db(&db).await;
}
