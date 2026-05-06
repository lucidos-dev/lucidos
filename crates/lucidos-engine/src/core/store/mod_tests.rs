use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};
use chrono::TimeZone;
use serde_json::json;
use uuid::Uuid;

async fn insert_event(pool: &PgPool, id: Uuid, event_type: &str, created: DateTime<Utc>) {
    sqlx::query("INSERT INTO events (id, event_type, payload, created) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(event_type)
        .bind(json!({ "summary": "fixture" }))
        .bind(created)
        .execute(pool)
        .await
        .expect("insert event");
}

/// `before_event_id` returns events strictly older than the cursor under
/// `(created, id)` lexicographic ordering. When five events share one
/// timestamp and the middle one is the cursor, only the two with smaller
/// `id`s come back (under `created DESC, id DESC` order: `ids[1]` then
/// `ids[0]`), and the cursor itself is never re-fetched.
#[tokio::test]
async fn query_events_before_cursor_returns_strictly_older_events_at_same_timestamp() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let ts = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let mut ids: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();
    ids.sort();

    for id in &ids {
        insert_event(&pool, *id, "PageBoundaryTest", ts).await;
    }

    let result = store
        .query_events_paged(Some("PageBoundaryTest"), None, None, Some(ids[2]), None, 10)
        .await
        .expect("query_events_paged should succeed");

    let events = match result {
        QueryEventsResult::Events(e) => e,
        QueryEventsResult::CursorNotFound => panic!("cursor must exist"),
    };

    let returned: Vec<Uuid> = events.iter().map(|e| e.id).collect();
    assert_eq!(
        returned,
        vec![ids[1], ids[0]],
        "expected strictly-older events in DESC (created, id) order"
    );

    teardown_test_db(&db).await;
}

/// Symmetric to the before-cursor test: `after_event_id` returns strictly
/// newer events under `(created, id)` ordering, never including the cursor.
#[tokio::test]
async fn query_events_after_cursor_returns_strictly_newer_events_at_same_timestamp() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let ts = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let mut ids: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();
    ids.sort();

    for id in &ids {
        insert_event(&pool, *id, "AfterCursorTest", ts).await;
    }

    let result = store
        .query_events_paged(Some("AfterCursorTest"), None, None, None, Some(ids[2]), 10)
        .await
        .expect("query_events_paged should succeed");

    let events = match result {
        QueryEventsResult::Events(e) => e,
        QueryEventsResult::CursorNotFound => panic!("cursor must exist"),
    };

    let returned: Vec<Uuid> = events.iter().map(|e| e.id).collect();
    assert_eq!(
        returned,
        vec![ids[4], ids[3]],
        "expected strictly-newer events in DESC (created, id) order"
    );

    teardown_test_db(&db).await;
}

/// Walking a 7-event history newest-first in pages of 3 yields 3/3/1
/// rows with no overlap and no missing events — what plugins like
/// browser-learning need when folding months of history into knowhow.
#[tokio::test]
async fn query_events_pages_with_before_cursor_no_overlap() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let mut ids = Vec::new();
    for i in 0..7 {
        let id = Uuid::new_v4();
        ids.push(id);
        insert_event(
            &pool,
            id,
            "PagingTest",
            Utc.timestamp_opt(1_700_000_000 + i, 0).unwrap(),
        )
        .await;
    }
    // ids[0] oldest, ids[6] newest. DESC walk: ids[6], ids[5], ..., ids[0].

    async fn page(store: &EventStore, before: Option<Uuid>, limit: i64) -> Vec<Uuid> {
        match store
            .query_events_paged(Some("PagingTest"), None, None, before, None, limit)
            .await
            .expect("query_events")
        {
            QueryEventsResult::Events(e) => e.into_iter().map(|r| r.id).collect(),
            QueryEventsResult::CursorNotFound => panic!("cursor must exist"),
        }
    }

    let page1 = page(&store, None, 3).await;
    assert_eq!(page1, vec![ids[6], ids[5], ids[4]], "page 1");

    let page2 = page(&store, Some(*page1.last().unwrap()), 3).await;
    assert_eq!(page2, vec![ids[3], ids[2], ids[1]], "page 2");

    let page3 = page(&store, Some(*page2.last().unwrap()), 3).await;
    assert_eq!(page3, vec![ids[0]], "page 3");

    let mut all: Vec<Uuid> = page1.into_iter().chain(page2).chain(page3).collect();
    let total = all.len();
    all.sort();
    all.dedup();
    assert_eq!(total, 7, "must visit every event exactly once");
    assert_eq!(all.len(), total, "no duplicates across pages");

    teardown_test_db(&db).await;
}

/// A cursor uuid that doesn't resolve to an event must surface as
/// `CursorNotFound` so the HTTP layer can return 404 instead of silently
/// returning the unfiltered history.
#[tokio::test]
async fn query_events_returns_cursor_not_found_when_uuid_missing() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    insert_event(
        &pool,
        Uuid::new_v4(),
        "Existing",
        Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    )
    .await;

    let bogus = Uuid::new_v4();

    let before_result = store
        .query_events_paged(None, None, None, Some(bogus), None, 10)
        .await
        .expect("query_events_paged should succeed");
    assert!(matches!(before_result, QueryEventsResult::CursorNotFound));

    let after_result = store
        .query_events_paged(None, None, None, None, Some(bogus), 10)
        .await
        .expect("query_events_paged should succeed");
    assert!(matches!(after_result, QueryEventsResult::CursorNotFound));

    teardown_test_db(&db).await;
}
