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

/// Like [`insert_event`] but lets the test pin the payload byte size — used
/// by the `count_events` tests to verify the `byte_total` aggregate.
async fn insert_event_with_payload(
    pool: &PgPool,
    id: Uuid,
    event_type: &str,
    created: DateTime<Utc>,
    payload: serde_json::Value,
) {
    sqlx::query("INSERT INTO events (id, event_type, payload, created) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(event_type)
        .bind(payload)
        .bind(created)
        .execute(pool)
        .await
        .expect("insert event with payload");
}

/// Like [`insert_event`] but attributes the row to a thread, for the
/// `thread_id` filter.
async fn insert_thread_event(
    pool: &PgPool,
    id: Uuid,
    event_type: &str,
    created: DateTime<Utc>,
    thread_id: Uuid,
) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, thread_id) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(event_type)
    .bind(json!({ "summary": "fixture" }))
    .bind(created)
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("insert thread event");
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
        .query_events_paged(
            EventQueryFilters {
                event_type: Some("PageBoundaryTest"),
                before_event_id: Some(ids[2]),
                ..Default::default()
            },
            10,
        )
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
        .query_events_paged(
            EventQueryFilters {
                event_type: Some("AfterCursorTest"),
                after_event_id: Some(ids[2]),
                ..Default::default()
            },
            10,
        )
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
            .query_events_paged(
                EventQueryFilters {
                    event_type: Some("PagingTest"),
                    before_event_id: before,
                    ..Default::default()
                },
                limit,
            )
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
        .query_events_paged(
            EventQueryFilters {
                before_event_id: Some(bogus),
                ..Default::default()
            },
            10,
        )
        .await
        .expect("query_events_paged should succeed");
    assert!(matches!(before_result, QueryEventsResult::CursorNotFound));

    let after_result = store
        .query_events_paged(
            EventQueryFilters {
                after_event_id: Some(bogus),
                ..Default::default()
            },
            10,
        )
        .await
        .expect("query_events_paged should succeed");
    assert!(matches!(after_result, QueryEventsResult::CursorNotFound));

    teardown_test_db(&db).await;
}

/// `count_events` with a type filter returns just the matching rows; the
/// `byte_total` is the `SUM(octet_length(payload::text))` over those rows,
/// which is what callers use to budget a subsequent `query_events` drill.
#[tokio::test]
async fn count_events_with_type_filter_returns_matching_count_and_byte_total() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let ts = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let payload = json!({ "summary": "x".repeat(100) });
    for _ in 0..3 {
        insert_event_with_payload(&pool, Uuid::new_v4(), "Matching", ts, payload.clone()).await;
    }
    insert_event_with_payload(&pool, Uuid::new_v4(), "Other", ts, payload.clone()).await;

    let (count, byte_total) = store
        .count_events(Some("Matching"), None, None)
        .await
        .expect("count_events");
    assert_eq!(count, 3, "only the 3 Matching rows count");
    assert!(
        byte_total > 0,
        "byte_total must be > 0 for non-empty payload"
    );
    // The byte_total should be the sum across the 3 matching rows. Each row's
    // payload serializes to the same length (deterministic JSON), so the
    // total must be evenly divisible by the per-row size.
    assert_eq!(byte_total % 3, 0, "even split across 3 identical payloads");

    teardown_test_db(&db).await;
}

/// `count_events` with no rows in the window must return `(0, 0)` — never
/// `(0, NULL)` leaking out as a panic from `unwrap_or`.
#[tokio::test]
async fn count_events_returns_zero_bytes_when_no_matches() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let (count, byte_total) = store
        .count_events(Some("Nonexistent"), None, None)
        .await
        .expect("count_events");
    assert_eq!(count, 0);
    assert_eq!(byte_total, 0);

    teardown_test_db(&db).await;
}

/// `count_events_by_type` returns a per-type breakdown sorted by count desc
/// (with `event_type` ASC as the tiebreaker). This is the LLM's "what's
/// noisy this window" view — order matters because the LLM relies on the
/// sort to decide which types to drill into first.
#[tokio::test]
async fn count_events_by_type_returns_breakdown_sorted_by_count_desc() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let ts = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let payload = json!({ "summary": "fixture" });
    for _ in 0..5 {
        insert_event_with_payload(&pool, Uuid::new_v4(), "Noisy", ts, payload.clone()).await;
    }
    for _ in 0..2 {
        insert_event_with_payload(&pool, Uuid::new_v4(), "Quiet", ts, payload.clone()).await;
    }
    insert_event_with_payload(&pool, Uuid::new_v4(), "Singleton", ts, payload.clone()).await;

    let rows = store
        .count_events_by_type(None, None)
        .await
        .expect("count_events_by_type");

    let types: Vec<&str> = rows.iter().map(|(t, _, _)| t.as_str()).collect();
    let counts: Vec<i64> = rows.iter().map(|(_, c, _)| *c).collect();
    assert_eq!(
        types,
        vec!["Noisy", "Quiet", "Singleton"],
        "count desc order"
    );
    assert_eq!(counts, vec![5, 2, 1]);
    for (_, _, byte_total) in &rows {
        assert!(*byte_total > 0, "byte_total per type must be > 0");
    }

    teardown_test_db(&db).await;
}

/// Time-window filtering: only events strictly after `since` and strictly
/// before `until` count. Mirrors `query_events` semantics so a `count_events`
/// + `query_events` call pair sees the same population.
#[tokio::test]
async fn count_events_respects_since_until_window() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let payload = json!({ "summary": "fixture" });
    let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let t1 = Utc.timestamp_opt(1_700_001_000, 0).unwrap();
    let t2 = Utc.timestamp_opt(1_700_002_000, 0).unwrap();

    insert_event_with_payload(&pool, Uuid::new_v4(), "Windowed", t0, payload.clone()).await;
    insert_event_with_payload(&pool, Uuid::new_v4(), "Windowed", t1, payload.clone()).await;
    insert_event_with_payload(&pool, Uuid::new_v4(), "Windowed", t2, payload.clone()).await;

    // `since = t0` is strict (> t0), so only t1 and t2 should count.
    let (count, _) = store
        .count_events(Some("Windowed"), Some(t0), None)
        .await
        .expect("count_events with since");
    assert_eq!(count, 2, "since is strictly greater than");

    // `until = t2` is strict (< t2), so only t0 and t1 should count.
    let (count, _) = store
        .count_events(Some("Windowed"), None, Some(t2))
        .await
        .expect("count_events with until");
    assert_eq!(count, 2, "until is strictly less than");

    // Combined: only t1 is in the open interval (t0, t2).
    let (count, _) = store
        .count_events(Some("Windowed"), Some(t0), Some(t2))
        .await
        .expect("count_events with since+until");
    assert_eq!(count, 1, "open interval excludes both endpoints");

    teardown_test_db(&db).await;
}

/// The read path behind "we talked about this": once thread search has found
/// the thread, its messages come back by filtering on it.
#[tokio::test]
async fn query_events_can_be_restricted_to_one_thread() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let ts = Utc.timestamp_opt(1_700_000_500, 0).unwrap();
    let wanted = Uuid::new_v4();
    let other = Uuid::new_v4();
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    insert_thread_event(&pool, mine, "ThreadFilterTest", ts, wanted).await;
    insert_thread_event(&pool, theirs, "ThreadFilterTest", ts, other).await;

    let got = match store
        .query_events_paged(
            EventQueryFilters {
                event_type: Some("ThreadFilterTest"),
                thread_id: Some(wanted),
                ..Default::default()
            },
            10,
        )
        .await
        .expect("query_events_paged should succeed")
    {
        QueryEventsResult::Events(e) => e,
        QueryEventsResult::CursorNotFound => panic!("no cursor was passed"),
    };

    assert_eq!(got.len(), 1, "only the named thread's row");
    assert_eq!(got[0].id, mine);

    teardown_test_db(&db).await;
}

/// The filter may only ever NARROW. Every existing caller (the CLI, triggers,
/// the SDK, the app UI bridge) sends no `thread_id`, so an omitted filter that
/// changed the result set would silently change all of them.
#[tokio::test]
async fn omitting_the_thread_filter_returns_every_thread() {
    let (pool, db) = setup_test_db().await;
    let store = EventStore::new(pool.clone());

    let ts = Utc.timestamp_opt(1_700_000_600, 0).unwrap();
    insert_thread_event(
        &pool,
        Uuid::new_v4(),
        "ThreadFilterOmitted",
        ts,
        Uuid::new_v4(),
    )
    .await;
    insert_thread_event(
        &pool,
        Uuid::new_v4(),
        "ThreadFilterOmitted",
        ts,
        Uuid::new_v4(),
    )
    .await;
    // A row with NO thread at all (a domain event) must still come back: the
    // filter is `IS NULL OR thread_id = $9`, not a join.
    insert_event(&pool, Uuid::new_v4(), "ThreadFilterOmitted", ts).await;

    let got = match store
        .query_events_paged(
            EventQueryFilters {
                event_type: Some("ThreadFilterOmitted"),
                ..Default::default()
            },
            10,
        )
        .await
        .expect("query_events_paged should succeed")
    {
        QueryEventsResult::Events(e) => e,
        QueryEventsResult::CursorNotFound => panic!("no cursor was passed"),
    };

    assert_eq!(got.len(), 3, "two threaded rows plus the unthreaded one");

    teardown_test_db(&db).await;
}
