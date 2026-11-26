use crate::core::store::EventStore;
use crate::test_support::{setup_test_db, teardown_test_db};
use chrono::{DateTime, TimeZone, Utc};
use sqlx::PgPool;
use uuid::Uuid;

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

#[tokio::test]
async fn get_recent_messages_selects_most_recent_threads_not_uuid_order() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let oldest = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let middle = Uuid::parse_str("88888888-8888-8888-8888-888888888888").unwrap();
    let newest = Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff").unwrap();

    let base = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    insert_message_at(&pool, oldest, base, "oldest thread").await;
    insert_message_at(
        &pool,
        middle,
        base + chrono::Duration::hours(1),
        "middle thread",
    )
    .await;
    insert_message_at(
        &pool,
        newest,
        base + chrono::Duration::hours(2),
        "newest thread",
    )
    .await;

    let messages = store
        .get_recent_messages(2, None)
        .await
        .expect("get_recent_messages");

    let returned: std::collections::HashSet<String> = messages
        .iter()
        .filter_map(|m| m.thread_id.clone())
        .collect();

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

#[tokio::test]
async fn get_recent_messages_before_cursor_orders_by_recency() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let oldest = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let middle = Uuid::parse_str("88888888-8888-8888-8888-888888888888").unwrap();
    let newest = Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff").unwrap();

    let base = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    insert_message_at(&pool, oldest, base, "oldest thread").await;
    insert_message_at(
        &pool,
        middle,
        base + chrono::Duration::hours(1),
        "middle thread",
    )
    .await;
    insert_message_at(
        &pool,
        newest,
        base + chrono::Duration::hours(2),
        "newest thread",
    )
    .await;

    let cutoff = base + chrono::Duration::hours(3);
    let messages = store
        .get_recent_messages(1, Some(cutoff))
        .await
        .expect("get_recent_messages before cutoff");

    let returned: std::collections::HashSet<String> = messages
        .iter()
        .filter_map(|m| m.thread_id.clone())
        .collect();

    assert_eq!(
        returned,
        std::collections::HashSet::from([newest.to_string()]),
        "before-cursor branch must select the single most-recent thread; got {:?}",
        returned
    );

    teardown_test_db(&db).await;
}
