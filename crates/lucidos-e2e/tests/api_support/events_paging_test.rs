//! Paging on `GET /api/v1/threads/:id/events`.
//!
//! One reported thread held 12,982 events and answered with 7,446,426 bytes, to
//! paint a tail the render had already windowed to about 0.12 MB. The endpoint
//! now serves the newest page and a cursor, and the client backfills as the
//! reader scrolls up.
//!
//! The invariant these guard is that a SHORT thread never notices. The median
//! thread in the reporting workspace is 198 events, so the overwhelming
//! majority arrive whole in one response, exactly as before.
//!
//! Plan: `docs/plans/2026-09-20-a-long-thread-opens-without-its-whole-history.md`.

use crate::support::{base_url, db_url, http_client, seed_chat_thread_summary};
use serde_json::Value;
use uuid::Uuid;

async fn pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to e2e DB")
}

/// A thread of `n` chat messages, one second apart so the clock orders them.
async fn seed_thread(pool: &sqlx::PgPool, n: i32) -> Uuid {
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(pool, thread_id, "idle").await;
    for i in 0..n {
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
             VALUES ($1, 'MessageReceived', $2, NOW() + ($4 || ' seconds')::interval, $3::text, 'thread', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(serde_json::json!({ "text": format!("message {i}"), "channel": "chat" }))
        .bind(thread_id)
        .bind(i.to_string())
        .execute(pool)
        .await
        .expect("seed MessageReceived");
    }
    thread_id
}

/// The snapshot, with query params encoded by the client rather than by hand. A
/// `created` timestamp carries a `+` offset, which a raw query string would
/// deliver as a space.
async fn snapshot(thread_id: Uuid, params: &[(&str, String)]) -> Value {
    let url = format!("{}/api/v1/threads/{}/events", base_url(), thread_id);
    let res = http_client()
        .get(&url)
        .query(params)
        .send()
        .await
        .expect("snapshot request");
    assert!(
        res.status().is_success(),
        "snapshot status {}",
        res.status()
    );
    res.json().await.expect("snapshot json")
}

/// A thread shorter than the page is served whole, and says nothing remains.
/// `hasMore` is skipped when false, so its absence IS the answer.
#[tokio::test]
async fn a_short_thread_is_served_whole_with_nothing_older() {
    let pool = pool().await;
    let thread_id = seed_thread(&pool, 5).await;

    let body = snapshot(thread_id, &[("limit", "50".into())]).await;
    let events = body["events"].as_array().expect("events array");
    assert_eq!(events.len(), 5, "the whole thread fits in the page");
    assert!(
        body.get("hasMore").is_none() || body["hasMore"] == Value::Bool(false),
        "nothing older to fetch: {:?}",
        body.get("hasMore")
    );
}

/// The unpaged read is byte-identical to the paged one that covers the thread.
/// That is the promise to every existing caller, the export path included.
#[tokio::test]
async fn a_short_thread_reads_the_same_paged_or_not() {
    let pool = pool().await;
    let thread_id = seed_thread(&pool, 5).await;

    let unpaged = snapshot(thread_id, &[]).await;
    let paged = snapshot(thread_id, &[("limit", "50".into())]).await;
    assert_eq!(
        unpaged["events"], paged["events"],
        "a thread that fits must read identically either way"
    );
}

/// A long thread answers with the NEWEST page and says older ones remain.
#[tokio::test]
async fn a_long_thread_answers_with_its_newest_page() {
    let pool = pool().await;
    let thread_id = seed_thread(&pool, 12).await;

    let body = snapshot(thread_id, &[("limit", "4".into())]).await;
    let events = body["events"].as_array().expect("events array");
    assert_eq!(events.len(), 4, "one page");
    assert_eq!(body["hasMore"], Value::Bool(true), "eight older remain");
    // Oldest-first within the page, and it is the TAIL of the thread.
    assert_eq!(events[0]["payload"]["text"], "message 8");
    assert_eq!(events[3]["payload"]["text"], "message 11");
}

/// The cursor walks back with no overlap and no gap. An overlap would double a
/// turn in the transcript; a gap would lose one.
#[tokio::test]
async fn the_cursor_walks_back_over_every_event_once() {
    let pool = pool().await;
    let thread_id = seed_thread(&pool, 12).await;

    let mut seen: Vec<String> = Vec::new();
    let mut params: Vec<(&str, String)> = vec![("limit", "5".into())];
    loop {
        let body = snapshot(thread_id, &params).await;
        let events = body["events"].as_array().expect("events array").clone();
        let mut texts: Vec<String> = events
            .iter()
            .map(|e| {
                e["payload"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        texts.append(&mut seen);
        seen = texts;
        if body["hasMore"] != Value::Bool(true) {
            break;
        }
        let oldest = &events[0];
        params = vec![
            ("limit", "5".into()),
            (
                "before_created",
                oldest["created"].as_str().expect("created").to_string(),
            ),
            (
                "before_seq",
                oldest["sequence"].as_i64().expect("sequence").to_string(),
            ),
        ];
    }

    let expected: Vec<String> = (0..12).map(|i| format!("message {i}")).collect();
    assert_eq!(seen, expected, "every event once, oldest to newest");
}

/// `after` walks forward and `limit` walks backward, so asking both is a caller
/// bug rather than a combination to resolve.
#[tokio::test]
async fn after_and_limit_together_are_refused() {
    let pool = pool().await;
    let thread_id = seed_thread(&pool, 3).await;

    let url = format!(
        "{}/api/v1/threads/{}/events?after=1&limit=2",
        base_url(),
        thread_id
    );
    let res = http_client().get(&url).send().await.expect("request");
    assert_eq!(res.status(), 400, "the two are mutually exclusive");
}

/// Half a cursor cannot order anything, because rows are ordered by the pair.
#[tokio::test]
async fn half_a_cursor_is_refused() {
    let pool = pool().await;
    let thread_id = seed_thread(&pool, 3).await;

    let url = format!(
        "{}/api/v1/threads/{}/events?limit=2&before_seq=5",
        base_url(),
        thread_id
    );
    let res = http_client().get(&url).send().await.expect("request");
    assert_eq!(
        res.status(),
        400,
        "before_created and before_seq travel together"
    );
}
