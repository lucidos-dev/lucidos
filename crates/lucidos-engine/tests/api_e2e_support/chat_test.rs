use crate::support::{base_url, db_url, http_client, unique_marker};
use uuid::Uuid;

#[tokio::test]
#[ignore]
async fn chat_stream_returns_event_id() {
    let client = http_client();
    let url = format!("{}/api/chat/stream", base_url());
    let marker = unique_marker("api-chat");

    let body = serde_json::json!({
        "message": format!("Say exactly: \"hello {marker}\""),
        "mode": "human",
    });

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Chat stream request failed");

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());

    let result: serde_json::Value = resp.json().await.expect("Invalid JSON response");
    assert!(
        result["event_id"].is_string(),
        "Response should contain event_id"
    );
    let event_id = result["event_id"].as_str().unwrap();
    assert!(!event_id.is_empty(), "event_id should not be empty");
}

#[tokio::test]
#[ignore]
async fn chat_stream_with_thread_id() {
    let client = http_client();
    let url = format!("{}/api/chat/stream", base_url());
    let marker = unique_marker("api-thread");

    // First message creates a thread
    let body1 = serde_json::json!({
        "message": format!("Say exactly: \"first {marker}\""),
        "mode": "human",
    });
    let resp1: serde_json::Value = client
        .post(&url)
        .json(&body1)
        .send()
        .await
        .expect("First chat request failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert!(
        resp1["event_id"].is_string(),
        "First message should return event_id"
    );

    // Wait for the response to complete
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // Look up what thread was created by checking the threads API
    let threads_url = format!("{}/api/threads", base_url());
    let threads: serde_json::Value = client
        .get(&threads_url)
        .send()
        .await
        .expect("Threads request failed")
        .json()
        .await
        .expect("Invalid JSON");

    // Should have at least one thread in history
    let history = threads["history"]
        .as_array()
        .expect("history should be array");
    assert!(!history.is_empty(), "Should have at least one thread");

    // Get the most recent thread
    let thread_id = history[0]["thread_id"].as_str().unwrap();

    // Send a follow-up to the same thread
    let body2 = serde_json::json!({
        "message": format!("Say exactly: \"second {marker}\""),
        "mode": "human",
        "thread_id": thread_id,
    });
    let resp2 = client
        .post(&url)
        .json(&body2)
        .send()
        .await
        .expect("Follow-up chat request failed");

    assert_eq!(resp2.status(), 200);
    let result2: serde_json::Value = resp2.json().await.expect("Invalid JSON");
    assert!(result2["event_id"].is_string());
}

#[tokio::test]
#[ignore]
async fn chat_empty_message_is_rejected() {
    let client = http_client();
    let url = format!("{}/api/chat/stream", base_url());

    let body = serde_json::json!({
        "message": "",
        "mode": "human",
    });

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Request failed");

    // Empty message should be rejected (400) or handled gracefully (not 500)
    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 200,
        "Empty message should return 400 or 200, got {status}"
    );
}

/// Poll thread_summaries for the row whose first_message contains `marker`.
/// Returns the (parent_thread_id, initiator) pair. Panics on timeout.
async fn poll_thread_summary_by_marker(
    pool: &sqlx::PgPool,
    marker: &str,
    max_secs: u64,
) -> (Option<Uuid>, String) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(max_secs);
    let pattern = format!("%{}%", marker);
    loop {
        let row: Option<(Option<Uuid>, String)> = sqlx::query_as(
            "SELECT parent_thread_id, initiator FROM thread_summaries WHERE first_message LIKE $1 LIMIT 1",
        )
        .bind(&pattern)
        .fetch_optional(pool)
        .await
        .expect("DB query failed");

        if let Some(row) = row {
            return row;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for thread_summaries row matching marker {marker} after {max_secs}s",
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Refactor regression: POST /api/chat/stream used to hardcode parent_thread_id=NULL
/// and initiator='user', causing CC-spawned child threads to be mislabeled as
/// user-initiated roots. Verify the wire fields actually reach thread_summaries
/// when the caller explicitly identifies as system.
#[tokio::test]
#[ignore]
async fn parent_thread_id_propagates_to_projection() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let url = format!("{}/api/chat/stream", base_url());
    let parent_uuid = Uuid::new_v4();
    let spawning_event = Uuid::new_v4();
    let marker = unique_marker("api-parent-prop");

    let body = serde_json::json!({
        "message": format!("noop test message {marker}"),
        "mode": "agent",
        "parent_thread_id": parent_uuid.to_string(),
        "spawning_event_id": spawning_event.to_string(),
    });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("POST failed");
    assert_eq!(resp.status(), 200);

    let (parent, initiator) = poll_thread_summary_by_marker(&pool, &marker, 15).await;
    assert_eq!(
        parent,
        Some(parent_uuid),
        "parent_thread_id from request must reach projection"
    );
    assert_eq!(
        initiator, "system",
        "mode=agent must mark the thread system-initiated"
    );
}

/// mode=human keeps threads user-initiated (regression check for the default path).
#[tokio::test]
#[ignore]
async fn human_mode_means_user_initiated() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let url = format!("{}/api/chat/stream", base_url());
    let marker = unique_marker("api-no-parent");

    let body = serde_json::json!({
        "message": format!("noop test message {marker}"),
        "mode": "human",
    });
    client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("POST failed");

    let (parent, initiator) = poll_thread_summary_by_marker(&pool, &marker, 15).await;
    assert_eq!(
        parent, None,
        "human-mode requests have NULL parent_thread_id in projection"
    );
    assert_eq!(initiator, "user", "mode=human produces initiator=user");
}

#[tokio::test]
#[ignore]
async fn invalid_parent_thread_id_returns_400() {
    let client = http_client();
    let url = format!("{}/api/chat/stream", base_url());
    let body = serde_json::json!({
        "message": "test",
        "mode": "agent",
        "parent_thread_id": "not-a-valid-uuid",
    });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Request failed");
    assert_eq!(
        resp.status(),
        400,
        "invalid parent_thread_id must return 400, not silently fall back"
    );
}

/// `mode` is mandatory — requests omitting it must be rejected (not silently
/// defaulted to human) so the spawn-thread skill and other callers cannot
/// accidentally produce mislabeled threads.
#[tokio::test]
#[ignore]
async fn missing_mode_returns_400() {
    let client = http_client();
    let url = format!("{}/api/chat/stream", base_url());
    let body = serde_json::json!({
        "message": "test",
    });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Request failed");
    let status = resp.status().as_u16();
    assert!(
        (400..500).contains(&status),
        "missing mode must be rejected as a bad request, got {status}",
    );
}

/// Regression: mobile screenshots in base64 routinely exceed axum's 2 MiB
/// default body limit, surfacing as "Failed to send message: Failed to buffer
/// the request body" toast. /api/chat/stream must accept large image payloads.
#[tokio::test]
#[ignore]
async fn chat_stream_accepts_large_image_payload() {
    let client = http_client();
    let url = format!("{}/api/chat/stream", base_url());

    // 5 MiB of base64 — well above axum's 2 MiB default, well below our 100 MiB cap.
    let large_base64 = "A".repeat(5 * 1024 * 1024);
    let body = serde_json::json!({
        "message": "ignore — body limit regression test",
        "mode": "human",
        "images": [{
            "base64": large_base64,
            "mime_type": "image/png",
        }],
    });

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Request failed");
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert_eq!(
        status, 200,
        "chat_stream must accept >2 MiB body (mobile screenshots). Got {status}: {text}",
    );
}
