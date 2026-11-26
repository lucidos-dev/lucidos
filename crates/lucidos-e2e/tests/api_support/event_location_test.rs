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
