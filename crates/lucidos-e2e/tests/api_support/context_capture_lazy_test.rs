//! Verify the lazy-load contract for `ContextCaptured`:
//!
//! 1. `GET /api/v1/threads/:tid/events` strips `sections` + `tools` from
//!    `ContextCaptured` payloads and stamps a `sections_stripped: true`
//!    marker. Keeps the lightweight inline-chip fields.
//! 2. `GET /api/v1/events/:eid/context` returns the original `sections` +
//!    `tools` on demand.
//! 3. `GET /api/v1/threads/:tid/events?include_context=true` opts back into
//!    the full payload — the export-thread path uses this so bug-report
//!    dumps stay complete.
//!
//! Together these stop a heavy chat thread (one captured prompt per turn,
//! ~50 kB sections each) from shipping all snapshots up-front; the modal
//! fetches the full breakdown only when the user opens it.

use crate::support::{base_url, db_url, http_client, seed_chat_thread_summary, unique_marker};
use serde_json::Value;
use uuid::Uuid;

/// Seed a chat thread + a ContextCaptured event with real `sections` and
/// `tools` arrays. Returns `(thread_id, event_id)`.
///
/// The sections are written in the PRE-RENAME shape, spelling a section's
/// budget delta `char_count`. That is what months of stored rows look like.
/// The read paths serve them verbatim. So this fixture is what proves an old
/// capture still reaches the Context Viewer with a size it can read.
async fn seed_thread_with_capture(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let marker = unique_marker("api-context-capture-lazy");

    seed_chat_thread_summary(pool, thread_id, "idle").await;

    // Seed a MessageReceived so the thread has at least one prior event,
    // matching how real threads look on the snapshot endpoint.
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

/// Snapshot endpoint must omit `sections` + `tools` and stamp the marker.
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
    // Inline chip still needs these.
    assert_eq!(payload["producer"], "main_llm");
    assert_eq!(payload["model"], "claude-opus-4-7");
    assert_eq!(payload["context_window"], 200_000);
    assert_eq!(payload["estimated_total_tokens"], 4_242);
    assert!(payload["usage"].is_object(), "usage preserved for chip");
}

/// New endpoint returns the original `sections` + `tools` for the event.
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
    // The stored key was `char_count`, and the endpoint serves sections
    // verbatim, so nothing but the boundary rename puts it where the viewer
    // reads it.
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

/// Non-ContextCaptured event id → 404 (handler refuses to leak arbitrary event payloads).
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

/// Unknown event id → 404.
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

/// `?include_context=true` opts back into full sections — covers the
/// export-thread path so bug-report dumps stay complete after the strip.
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
    // Same boundary rename as the lazy fetch. A bug-report dump that spelled
    // the size two ways depending on when the row was written is a dump
    // nobody can read with one query.
    assert_eq!(payload["sections"][0]["budget_delta_chars"], 12_345);
    assert!(payload["sections"][0].get("char_count").is_none());
}

/// A capture written today passes through untouched, both keys intact.
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
    // The one row where the two part. The delta is what the loop added; the
    // region is the whole message array.
    assert_eq!(sections[1]["budget_delta_chars"], 600);
    assert_eq!(sections[1]["content_chars"], 645_368);
}
