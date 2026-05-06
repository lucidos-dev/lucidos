use crate::support::{
    base_url, db_url, http_client, poll_thread_summary_by_marker, unique_marker,
};
use uuid::Uuid;

#[tokio::test]
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

/// Refactor regression: POST /api/chat/stream used to hardcode parent_thread_id=NULL
/// and initiator='user', causing CC-spawned child threads to be mislabeled as
/// user-initiated roots. Verify the wire fields actually reach thread_summaries
/// when the caller explicitly identifies as system.
#[tokio::test]
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

    let row = poll_thread_summary_by_marker(&pool, &marker, 15).await;
    assert_eq!(
        row.parent_thread_id,
        Some(parent_uuid),
        "parent_thread_id from request must reach projection"
    );
    assert_eq!(
        row.initiator, "system",
        "mode=agent must mark the thread system-initiated"
    );
}

/// mode=human keeps threads user-initiated (regression check for the default path).
#[tokio::test]
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

    let row = poll_thread_summary_by_marker(&pool, &marker, 15).await;
    assert_eq!(
        row.parent_thread_id, None,
        "human-mode requests have NULL parent_thread_id in projection"
    );
    assert_eq!(row.initiator, "user", "mode=human produces initiator=user");
}

#[tokio::test]
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

/// A thread is locked to its (mode, repo) at first message — switching
/// either mid-thread caused multi-repo session mixing where the executor
/// card and the commands menu disagreed (e.g. menu showed Lucidos skills
/// while the session ran on User Acquisition). The chat handler must reject
/// follow-ups that try to flip mode or repo.
#[tokio::test]
async fn chat_stream_rejects_mode_switch_on_existing_thread() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e db");

    let thread_id = Uuid::new_v4();
    sqlx::query("INSERT INTO thread_summaries (thread_id, source) VALUES ($1, 'chat')")
        .bind(thread_id)
        .execute(&pool)
        .await
        .expect("seed chat thread");

    let resp = client
        .post(format!("{}/api/chat/stream", base_url()))
        .json(&serde_json::json!({
            "message": "switch me to CC",
            "mode": "human",
            "thread_id": thread_id.to_string(),
            "use_claude_code": true,
            "repo_id": Uuid::new_v4().to_string(),
        }))
        .send()
        .await
        .expect("send follow-up");

    assert_eq!(
        resp.status(),
        409,
        "chat\u{2192}CC switch on existing thread must 409, got {}",
        resp.status(),
    );

    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .ok();
}

/// Companion to the mode-switch test: a CC thread bound to repo A must
/// reject a follow-up requesting repo B (the menu would otherwise collapse
/// to whichever repo's `changes` row is most recent and disagree with the
/// executor card).
#[tokio::test]
async fn chat_stream_rejects_repo_switch_on_existing_cc_thread() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e db");

    let repo_a = Uuid::new_v4().to_string();
    let repo_b = Uuid::new_v4().to_string();
    let thread_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, cc_repo_id) \
         VALUES ($1, 'claude_code', $2)",
    )
    .bind(thread_id)
    .bind(&repo_a)
    .execute(&pool)
    .await
    .expect("seed CC thread on repo A");

    let resp = client
        .post(format!("{}/api/chat/stream", base_url()))
        .json(&serde_json::json!({
            "message": "switch repo on me",
            "mode": "human",
            "thread_id": thread_id.to_string(),
            "use_claude_code": true,
            "repo_id": repo_b,
        }))
        .send()
        .await
        .expect("send follow-up");

    assert_eq!(
        resp.status(),
        409,
        "CC repo switch on existing thread must 409, got {}",
        resp.status(),
    );

    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .ok();
}

/// Regression: mobile screenshots in base64 routinely exceed axum's 2 MiB
/// default body limit, surfacing as "Failed to send message: Failed to buffer
/// the request body" toast. /api/chat/stream must accept large image payloads.
#[tokio::test]
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
