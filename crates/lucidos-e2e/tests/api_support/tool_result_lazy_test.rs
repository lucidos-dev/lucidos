use crate::support::{base_url, db_url, http_client, seed_chat_thread_summary, unique_marker};
use serde_json::Value;
use uuid::Uuid;

async fn seed_thread_with_tool_result(pool: &sqlx::PgPool) -> (Uuid, Uuid, String) {
    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let marker = unique_marker("api-tool-result-lazy");
    let result_text = format!("bash stdout for {marker}\n").repeat(200);

    seed_chat_thread_summary(pool, thread_id, "idle").await;

    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'MessageReceived', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({ "text": format!("seed for {marker}"), "channel": "chat" }))
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("seed MessageReceived");

    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'ToolResult', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(event_id)
    .bind(serde_json::json!({
        "name": "run_bash",
        "result": result_text,
        "images": ["sha-abc", "sha-def"],
    }))
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("seed ToolResult");

    (thread_id, event_id, result_text)
}

async fn pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to e2e DB")
}

#[tokio::test]
async fn snapshot_strips_tool_result_content_and_stamps_marker() {
    let pool = pool().await;
    let (thread_id, event_id, _result_text) = seed_thread_with_tool_result(&pool).await;

    let client = http_client();
    let url = format!("{}/api/v1/threads/{}/events", base_url(), thread_id);
    let body: Value = client
        .get(&url)
        .send()
        .await
        .expect("snapshot request failed")
        .json()
        .await
        .expect("invalid JSON");

    let events = body["events"].as_array().expect("events array");
    let tool_row = events
        .iter()
        .find(|e| e["event_type"] == "ToolResult")
        .unwrap_or_else(|| panic!("ToolResult not present in snapshot for {thread_id}"));
    assert_eq!(tool_row["event_id"], Value::String(event_id.to_string()));

    let payload = &tool_row["payload"];
    assert!(
        payload.get("result").is_none(),
        "result should be stripped from snapshot payload, got: {payload}"
    );
    assert_eq!(payload["result_stripped"], Value::Bool(true));
    assert_eq!(payload["name"], "run_bash");
    assert_eq!(payload["images"], serde_json::json!(["sha-abc", "sha-def"]));
}

#[tokio::test]
async fn tool_result_endpoint_returns_full_result() {
    let pool = pool().await;
    let (_thread_id, event_id, result_text) = seed_thread_with_tool_result(&pool).await;

    let client = http_client();
    let url = format!("{}/api/v1/events/{}/tool-result", base_url(), event_id);
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("tool-result request failed");
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("invalid JSON");
    assert_eq!(body["result"], Value::String(result_text));
}

#[tokio::test]
async fn tool_result_endpoint_returns_null_for_image_only_results() {
    let pool = pool().await;
    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let marker = unique_marker("api-tool-result-lazy-image-only");
    seed_chat_thread_summary(&pool, thread_id, "idle").await;
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'MessageReceived', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({ "text": format!("seed for {marker}"), "channel": "chat" }))
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("seed MessageReceived");
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'ToolResult', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(event_id)
    .bind(serde_json::json!({ "name": "generate_image", "images": ["sha-img"] }))
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("seed image-only ToolResult");

    let client = http_client();
    let url = format!("{}/api/v1/events/{}/tool-result", base_url(), event_id);
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("tool-result request failed");
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("invalid JSON");
    assert_eq!(body["result"], Value::Null);
}

#[tokio::test]
async fn tool_result_endpoint_rejects_non_tool_result_event() {
    let pool = pool().await;
    let thread_id = Uuid::new_v4();
    let msg_event = Uuid::new_v4();
    let marker = unique_marker("api-tool-result-lazy-non-tr");
    seed_chat_thread_summary(&pool, thread_id, "idle").await;
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'MessageReceived', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(msg_event)
    .bind(serde_json::json!({ "text": format!("seed for {marker}"), "channel": "chat" }))
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("seed MessageReceived");

    let client = http_client();
    let url = format!("{}/api/v1/events/{}/tool-result", base_url(), msg_event);
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("tool-result request failed");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn tool_result_endpoint_404_on_unknown_event_id() {
    let _pool = pool().await;
    let client = http_client();
    let unknown = Uuid::new_v4();
    let url = format!("{}/api/v1/events/{}/tool-result", base_url(), unknown);
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("tool-result request failed");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn snapshot_include_context_true_preserves_tool_result() {
    let pool = pool().await;
    let (thread_id, _event_id, result_text) = seed_thread_with_tool_result(&pool).await;

    let client = http_client();
    let url = format!(
        "{}/api/v1/threads/{}/events?include_context=true",
        base_url(),
        thread_id
    );
    let body: Value = client
        .get(&url)
        .send()
        .await
        .expect("snapshot request failed")
        .json()
        .await
        .expect("invalid JSON");

    let tool_row = body["events"]
        .as_array()
        .expect("events array")
        .iter()
        .find(|e| e["event_type"] == "ToolResult")
        .expect("ToolResult present");
    let payload = &tool_row["payload"];
    assert_eq!(payload["result"], Value::String(result_text));
    assert!(
        payload.get("result_stripped").is_none(),
        "include_context=true must not stamp the strip marker"
    );
}
