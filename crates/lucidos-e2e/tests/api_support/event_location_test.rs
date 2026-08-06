//! Verify `GET /api/v1/events/:eid/location`, the lookup behind the event-wait
//! step's "show it".
//!
//! An `EventWaitDelivered` carries the matched event's id but not its thread,
//! and a wait exists precisely because the thread is watching something
//! happening SOMEWHERE ELSE, so resolving that thread is what makes the jump
//! possible at all.
//!
//! Three answers, and the frontend words each one differently, so they must stay
//! distinguishable on the wire:
//!
//! 1. A thread event returns its owning `thread_id`.
//! 2. A workspace *domain event* returns `thread_id: null`. It belongs to no
//!    conversation, which is a real answer rather than a missing one.
//! 3. An unknown event id returns 404.

use crate::support::{base_url, db_url, http_client, seed_chat_thread_summary, unique_marker};
use serde_json::Value;
use uuid::Uuid;

async fn pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to e2e DB")
}

async fn location(event_id: Uuid) -> reqwest::Response {
    let url = format!("{}/api/v1/events/{}/location", base_url(), event_id);
    http_client()
        .get(&url)
        .send()
        .await
        .expect("location request failed")
}

/// A step folded into a turn resolves to its thread. `CodingAgentIdled` is the
/// event an event wait matches in practice, and it stamps no element of its own
/// in the transcript, so the thread id is the whole answer the card gets.
#[tokio::test]
async fn location_returns_the_owning_thread_for_a_thread_event() {
    let pool = pool().await;
    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let marker = unique_marker("api-event-location");
    seed_chat_thread_summary(&pool, thread_id, "idle").await;

    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'CodingAgentIdled', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(event_id)
    .bind(serde_json::json!({ "coding_agent": "claude-code", "marker": marker }))
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("seed CodingAgentIdled");

    let resp = location(event_id).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.expect("invalid JSON");
    assert_eq!(body["thread_id"], Value::String(thread_id.to_string()));
}

/// A domain event has `aggregate = 'domain'`, so the insert leaves `thread_id`
/// NULL and it renders in no transcript. The endpoint must say so explicitly
/// rather than 404, which is how the card tells "nowhere to open it" apart from
/// "that event is gone".
#[tokio::test]
async fn location_returns_null_thread_for_a_workspace_domain_event() {
    let pool = pool().await;
    let event_id = Uuid::new_v4();
    let marker = unique_marker("api-event-location-domain");

    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'ReleaseBuildFinished', $2, NOW(), 'ReleaseBuildFinished', 'domain', NULL)",
    )
    .bind(event_id)
    .bind(serde_json::json!({ "summary": format!("domain event for {marker}") }))
    .execute(&pool)
    .await
    .expect("seed domain event");

    let resp = location(event_id).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.expect("invalid JSON");
    assert_eq!(
        body["thread_id"],
        Value::Null,
        "a domain event must report a null thread, not omit the key"
    );
}

#[tokio::test]
async fn location_404s_on_an_unknown_event_id() {
    let _pool = pool().await;
    assert_eq!(location(Uuid::new_v4()).await.status(), 404);
}

#[tokio::test]
async fn location_400s_on_a_malformed_event_id() {
    let _pool = pool().await;
    let url = format!("{}/api/v1/events/not-a-uuid/location", base_url());
    let resp = http_client()
        .get(&url)
        .send()
        .await
        .expect("location request failed");
    assert_eq!(resp.status(), 400);
}
