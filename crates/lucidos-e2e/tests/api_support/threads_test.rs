use crate::support::{
    base_url, db_url, poll_thread_summary_by_marker, poll_threads_until_archive, unique_marker,
    user_client,
};

#[tokio::test]
async fn threads_list_returns_expected_shape() {
    let client = user_client().await;
    let url = format!("{}/api/v1/threads", base_url());

    let resp = client
        .get(&url)
        .send()
        .await
        .expect("Threads request failed");
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");

    // Must have expected top-level fields
    assert!(body["saved"].is_array(), "saved should be array");
    assert!(body["archive"].is_array(), "archive should be array");
    assert!(body["active"].is_array(), "active should be array");
    assert!(
        body["active_threads"].is_array(),
        "active_threads should be array"
    );
    // Total archived-pile size for the collapsed Archive badge — distinct from
    // the loaded `archive` window.
    assert!(
        body["archive_count"].is_u64(),
        "archive_count should be a non-negative integer"
    );
}

#[tokio::test]
async fn thread_appears_after_sending_message() {
    let client = user_client().await;
    let marker = unique_marker("api-threads-list");

    // Send a message to create a thread
    let chat_url = format!("{}/api/v1/chat/stream", base_url());
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

    // Poll until the thread appears in archive (requires has_response = TRUE,
    // i.e., the LLM has finished generating a response)
    let body = poll_threads_until_archive(&client, 30).await;
    let archive = body["archive"].as_array().unwrap();

    // Verify thread has expected fields
    let thread = &archive[0];
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
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let marker = unique_marker("api-thread-msgs");

    // Create a thread
    let chat_url = format!("{}/api/v1/chat/stream", base_url());
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

    // Find OUR thread by marker — archive[0] races with parallel tests that
    // also create threads, and may resolve to a CC thread without any
    // SessionMessage-producing events.
    let thread_id = poll_thread_summary_by_marker(&pool, &marker, 30)
        .await
        .thread_id
        .to_string();

    // Poll messages until at least one appears — there can be a small window
    // between the thread appearing in archive (thread_summaries projection) and
    // the events being queryable via the messages endpoint.
    let messages_url = format!("{}/api/v1/threads/{}/messages", base_url(), thread_id);
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
    let client = user_client().await;
    let url = format!("{}/api/v1/disk-usage/worktrees", base_url());

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
    let client = user_client().await;
    let marker = unique_marker("api-save");

    // Create a thread
    let chat_url = format!("{}/api/v1/chat/stream", base_url());
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

    // Poll until the thread appears in archive
    let threads = poll_threads_until_archive(&client, 30).await;
    let thread_id = threads["archive"].as_array().unwrap()[0]["thread_id"]
        .as_str()
        .unwrap();

    // Save the thread
    let save_url = format!("{}/api/v1/threads/save", base_url());
    let save_body = serde_json::json!({ "thread_id": thread_id });
    let save_resp = client
        .post(&save_url)
        .json(&save_body)
        .send()
        .await
        .expect("Save request failed");
    assert_eq!(save_resp.status(), 200);

    // Idempotent: a duplicate save (e.g. an iOS PWA double-submit) must be a
    // 200 no-op, not a 409. Before the fix the second request hit the action
    // guard after the first flipped is_saved=TRUE and 409'd, whose client
    // handler then reverted the pin icon + toasted a spurious error.
    let save_again = client
        .post(&save_url)
        .json(&save_body)
        .send()
        .await
        .expect("Duplicate save request failed");
    assert_eq!(
        save_again.status(),
        200,
        "Saving an already-saved thread must be an idempotent 200, not 409"
    );

    // Verify it appears in saved
    let threads_url = format!("{}/api/v1/threads", base_url());
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
    let unsave_url = format!("{}/api/v1/threads/unsave", base_url());
    let unsave_body = serde_json::json!({ "thread_id": thread_id });
    let unsave_resp = client
        .post(&unsave_url)
        .json(&unsave_body)
        .send()
        .await
        .expect("Unsave request failed");
    assert_eq!(unsave_resp.status(), 200);

    // Idempotent the other way too: a duplicate unsave on an already-unsaved
    // thread is a 200 no-op, not a 409.
    let unsave_again = client
        .post(&unsave_url)
        .json(&unsave_body)
        .send()
        .await
        .expect("Duplicate unsave request failed");
    assert_eq!(
        unsave_again.status(),
        200,
        "Unsaving an already-unsaved thread must be an idempotent 200, not 409"
    );
}
