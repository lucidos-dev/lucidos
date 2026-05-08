use crate::support::{
    base_url, db_url, http_client, poll_thread_summary_by_marker, poll_threads_until_history,
    unique_marker,
};

#[tokio::test]
async fn threads_list_returns_expected_shape() {
    let client = http_client();
    let url = format!("{}/api/threads", base_url());

    let resp = client
        .get(&url)
        .send()
        .await
        .expect("Threads request failed");
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");

    // Must have expected top-level fields
    assert!(body["saved"].is_array(), "saved should be array");
    assert!(body["history"].is_array(), "history should be array");
    assert!(body["active"].is_array(), "active should be array");
    assert!(
        body["active_threads"].is_array(),
        "active_threads should be array"
    );
}

#[tokio::test]
async fn thread_appears_after_sending_message() {
    let client = http_client();
    let marker = unique_marker("api-threads-list");

    // Send a message to create a thread
    let chat_url = format!("{}/api/chat/stream", base_url());
    let chat_body = serde_json::json!({
        "message": format!("Say exactly: \"listed {marker}\""),
        "mode": "human",
    });
    client
        .post(&chat_url)
        .json(&chat_body)
        .send()
        .await
        .expect("Chat request failed");

    // Poll until the thread appears in history (requires has_response = TRUE,
    // i.e., the LLM has finished generating a response)
    let body = poll_threads_until_history(&client, 30).await;
    let history = body["history"].as_array().unwrap();

    // Verify thread has expected fields
    let thread = &history[0];
    assert!(
        thread["thread_id"].is_string(),
        "thread_id should be string"
    );
    assert!(thread["title"].is_string(), "title should be string");
    assert!(
        thread["last_activity"].is_string(),
        "last_activity should be string"
    );
}

#[tokio::test]
async fn thread_messages_endpoint_returns_events() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let marker = unique_marker("api-thread-msgs");

    // Create a thread
    let chat_url = format!("{}/api/chat/stream", base_url());
    let chat_body = serde_json::json!({
        "message": format!("Say exactly: \"messages {marker}\""),
        "mode": "human",
    });
    client
        .post(&chat_url)
        .json(&chat_body)
        .send()
        .await
        .expect("Chat request failed");

    // Find OUR thread by marker — history[0] races with parallel tests that
    // also create threads, and may resolve to a CC thread without any
    // SessionMessage-producing events.
    let thread_id = poll_thread_summary_by_marker(&pool, &marker, 30)
        .await
        .thread_id
        .to_string();

    // Poll messages until at least one appears — there can be a small window
    // between the thread appearing in history (thread_summaries projection) and
    // the events being queryable via the messages endpoint.
    let messages_url = format!("{}/api/threads/{}/messages", base_url(), thread_id);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let resp = client
            .get(&messages_url)
            .send()
            .await
            .expect("Messages request failed");
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
        assert!(body["messages"].is_array(), "messages should be an array");
        let msgs = body["messages"].as_array().unwrap();
        if !msgs.is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Thread should have at least one message within 10s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

#[tokio::test]
async fn disk_usage_worktrees_returns_inventory_shape() {
    let client = http_client();
    let url = format!("{}/api/disk-usage/worktrees", base_url());

    let resp = client
        .get(&url)
        .send()
        .await
        .expect("Disk usage request failed");
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    let arr = body["worktrees"]
        .as_array()
        .expect("worktrees should be an array");
    // Each row (if any) must carry the documented fields. We don't assume
    // any particular row count — fresh test workspaces may have zero CC
    // worktrees on disk.
    for row in arr {
        assert!(row["thread_id"].is_string(), "thread_id is string");
        assert!(row["worktree_path"].is_string(), "worktree_path is string");
        assert!(row["size_bytes"].is_u64(), "size_bytes is integer");
        assert!(row["is_dirty"].is_boolean(), "is_dirty is boolean");
        assert!(row["is_saved"].is_boolean(), "is_saved is boolean");
    }
}

#[tokio::test]
async fn thread_save_and_unsave() {
    let client = http_client();
    let marker = unique_marker("api-save");

    // Create a thread
    let chat_url = format!("{}/api/chat/stream", base_url());
    let chat_body = serde_json::json!({
        "message": format!("Say exactly: \"save-api {marker}\""),
        "mode": "human",
    });
    client
        .post(&chat_url)
        .json(&chat_body)
        .send()
        .await
        .expect("Chat request failed");

    // Poll until the thread appears in history
    let threads = poll_threads_until_history(&client, 30).await;
    let thread_id = threads["history"].as_array().unwrap()[0]["thread_id"]
        .as_str()
        .unwrap();

    // Save the thread
    let save_url = format!("{}/api/threads/save", base_url());
    let save_body = serde_json::json!({ "thread_id": thread_id });
    let save_resp = client
        .post(&save_url)
        .json(&save_body)
        .send()
        .await
        .expect("Save request failed");
    assert_eq!(save_resp.status(), 200);

    // Verify it appears in saved
    let threads_url = format!("{}/api/threads", base_url());
    let threads2: serde_json::Value = client
        .get(&threads_url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let saved = threads2["saved"].as_array().unwrap();
    assert!(
        saved
            .iter()
            .any(|t| t["thread_id"].as_str() == Some(thread_id)),
        "Thread should appear in saved list"
    );

    // Unsave the thread
    let unsave_url = format!("{}/api/threads/unsave", base_url());
    let unsave_body = serde_json::json!({ "thread_id": thread_id });
    let unsave_resp = client
        .post(&unsave_url)
        .json(&unsave_body)
        .send()
        .await
        .expect("Unsave request failed");
    assert_eq!(unsave_resp.status(), 200);
}
