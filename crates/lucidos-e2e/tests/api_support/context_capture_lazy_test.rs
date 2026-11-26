use crate::support::{base_url, db_url, http_client, seed_chat_thread_summary, unique_marker};
use serde_json::Value;
use uuid::Uuid;

async fn seed_thread_with_capture(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let marker = unique_marker("api-context-capture-lazy");

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
         VALUES ($1, 'ContextCaptured', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(event_id)
    .bind(serde_json::json!({
        "producer": "main_llm",
        "model": "claude-opus-4-7",
        "context_window": 200_000,
        "sections": [
            { "name": "system", "char_count": 12_345, "content": "A".repeat(12_345), "role": "system" },
            { "name": "history", "char_count": 4_321, "role": "prior_message" },
        ],
        "tools": ["search_memory", "edit_file"],
        "estimated_total_tokens": 4_242,
        "usage": {
            "input_tokens": 4_100,
            "output_tokens": 50,
            "cache_read_tokens": 0,
            "cache_creation_tokens": 0,
        },
        "trimmed": false,
    }))
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("seed ContextCaptured");

    (thread_id, event_id)
}

async fn pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to e2e DB")
}

#[tokio::test]
async fn snapshot_strips_context_capture_sections_and_tools() {
    let pool = pool().await;
    let (thread_id, event_id) = seed_thread_with_capture(&pool).await;

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
    let captured = events
        .iter()
        .find(|e| e["event_type"] == "ContextCaptured")
        .unwrap_or_else(|| panic!("ContextCaptured not present in snapshot for {thread_id}"));
    assert_eq!(captured["event_id"], Value::String(event_id.to_string()));

    let payload = &captured["payload"];
    assert!(
        payload.get("sections").is_none(),
        "sections should be stripped from snapshot payload, got: {payload}"
    );
    assert!(
        payload.get("tools").is_none(),
        "tools should be stripped from snapshot payload, got: {payload}"
    );
    assert_eq!(payload["sections_stripped"], Value::Bool(true));
    assert_eq!(payload["producer"], "main_llm");
    assert_eq!(payload["model"], "claude-opus-4-7");
    assert_eq!(payload["context_window"], 200_000);
    assert_eq!(payload["estimated_total_tokens"], 4_242);
    assert!(payload["usage"].is_object(), "usage preserved for chip");
}

#[tokio::test]
async fn context_endpoint_returns_full_sections_and_tools() {
    let pool = pool().await;
    let (_thread_id, event_id) = seed_thread_with_capture(&pool).await;

    let client = http_client();
    let url = format!("{}/api/v1/events/{}/context", base_url(), event_id);
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("context request failed");
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("invalid JSON");
    let sections = body["sections"].as_array().expect("sections array");
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0]["name"], "system");
    assert_eq!(sections[0]["budget_delta_chars"], 12_345);
    assert!(
        sections[0].get("char_count").is_none(),
        "the old key must not reach the client: {}",
        sections[0]
    );
    assert!(
        sections[0].get("content_chars").is_none(),
        "nobody measured the region when this row was written"
    );
    assert_eq!(
        sections[0]["content"].as_str().map(|s| s.len()),
        Some(12_345),
        "full section body returned"
    );
    assert_eq!(sections[1]["name"], "history");
    assert_eq!(sections[1]["budget_delta_chars"], 4_321);
    assert_eq!(sections[1]["role"], "prior_message");

    let tools = body["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0], "search_memory");
    assert_eq!(tools[1], "edit_file");
}

#[tokio::test]
async fn context_endpoint_rejects_non_context_captured_event() {
    let pool = pool().await;
    let thread_id = Uuid::new_v4();
    let msg_event = Uuid::new_v4();
    let marker = unique_marker("api-context-capture-lazy-non-cc");
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
    let url = format!("{}/api/v1/events/{}/context", base_url(), msg_event);
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("context request failed");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn context_endpoint_404_on_unknown_event_id() {
    let _pool = pool().await;
    let client = http_client();
    let unknown = Uuid::new_v4();
    let url = format!("{}/api/v1/events/{}/context", base_url(), unknown);
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("context request failed");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn snapshot_include_context_true_preserves_sections() {
    let pool = pool().await;
    let (thread_id, _event_id) = seed_thread_with_capture(&pool).await;

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

    let captured = body["events"]
        .as_array()
        .expect("events array")
        .iter()
        .find(|e| e["event_type"] == "ContextCaptured")
        .expect("ContextCaptured present");
    let payload = &captured["payload"];
    assert!(
        payload["sections"].is_array(),
        "include_context=true must keep sections inline, got: {payload}"
    );
    assert_eq!(payload["sections"].as_array().unwrap().len(), 2);
    assert!(payload["tools"].is_array());
    assert!(
        payload.get("sections_stripped").is_none(),
        "include_context=true must NOT stamp sections_stripped, got: {payload}"
    );
    assert_eq!(payload["sections"][0]["budget_delta_chars"], 12_345);
    assert!(payload["sections"][0].get("char_count").is_none());
}

#[tokio::test]
async fn context_endpoint_serves_both_sizes_of_a_current_capture() {
    let pool = pool().await;
    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let marker = unique_marker("api-context-capture-current");
    seed_chat_thread_summary(&pool, thread_id, "idle").await;
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'ContextCaptured', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(event_id)
    .bind(serde_json::json!({
        "producer": "main_llm",
        "model": format!("claude-opus-4-7 {marker}"),
        "context_window": 200_000,
        "sections": [
            { "name": "System Instructions", "budget_delta_chars": 900, "content_chars": 900, "role": "system" },
            { "name": "Conversation", "budget_delta_chars": 600, "content_chars": 645_368, "role": "user" },
        ],
        "tools": [],
        "estimated_total_tokens": 600,
        "trimmed": false,
    }))
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("seed ContextCaptured");

    let client = http_client();
    let url = format!("{}/api/v1/events/{}/context", base_url(), event_id);
    let body: Value = client
        .get(&url)
        .send()
        .await
        .expect("context request failed")
        .json()
        .await
        .expect("invalid JSON");

    let sections = body["sections"].as_array().expect("sections array");
    assert_eq!(sections[0]["budget_delta_chars"], 900);
    assert_eq!(sections[0]["content_chars"], 900);
    assert_eq!(sections[1]["budget_delta_chars"], 600);
    assert_eq!(sections[1]["content_chars"], 645_368);
}
