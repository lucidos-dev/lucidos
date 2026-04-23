use crate::support::{base_url, http_client, poll_threads_until_history, unique_marker};

#[tokio::test]
#[ignore]
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
    assert!(body["pinned"].is_array(), "pinned should be array");
    assert!(body["history"].is_array(), "history should be array");
    assert!(body["active"].is_array(), "active should be array");
    assert!(
        body["active_threads"].is_array(),
        "active_threads should be array"
    );
}

#[tokio::test]
#[ignore]
async fn thread_appears_after_sending_message() {
    let client = http_client();
    let marker = unique_marker("api-threads-list");

    // Send a message to create a thread
    let chat_url = format!("{}/api/chat/stream", base_url());
    let chat_body = serde_json::json!({
        "message": format!("Say exactly: \"listed {marker}\""),
        "sender": "user",
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
#[ignore]
async fn thread_messages_endpoint_returns_events() {
    let client = http_client();
    let marker = unique_marker("api-thread-msgs");

    // Create a thread
    let chat_url = format!("{}/api/chat/stream", base_url());
    let chat_body = serde_json::json!({
        "message": format!("Say exactly: \"messages {marker}\""),
        "sender": "user",
    });
    client
        .post(&chat_url)
        .json(&chat_body)
        .send()
        .await
        .expect("Chat request failed");

    // Poll until the thread appears in history
    let threads = poll_threads_until_history(&client, 30).await;
    let history = threads["history"].as_array().unwrap();
    let thread_id = history[0]["thread_id"].as_str().unwrap();

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
#[ignore]
async fn thread_pin_and_unpin() {
    let client = http_client();
    let marker = unique_marker("api-pin");

    // Create a thread
    let chat_url = format!("{}/api/chat/stream", base_url());
    let chat_body = serde_json::json!({
        "message": format!("Say exactly: \"pin-api {marker}\""),
        "sender": "user",
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

    // Pin the thread
    let pin_url = format!("{}/api/threads/pin", base_url());
    let pin_body = serde_json::json!({ "thread_id": thread_id });
    let pin_resp = client
        .post(&pin_url)
        .json(&pin_body)
        .send()
        .await
        .expect("Pin request failed");
    assert_eq!(pin_resp.status(), 200);

    // Verify it appears in pinned
    let threads_url = format!("{}/api/threads", base_url());
    let threads2: serde_json::Value = client
        .get(&threads_url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pinned = threads2["pinned"].as_array().unwrap();
    assert!(
        pinned
            .iter()
            .any(|t| t["thread_id"].as_str() == Some(thread_id)),
        "Thread should appear in pinned list"
    );

    // Unpin the thread
    let unpin_url = format!("{}/api/threads/unpin", base_url());
    let unpin_body = serde_json::json!({ "thread_id": thread_id });
    let unpin_resp = client
        .post(&unpin_url)
        .json(&unpin_body)
        .send()
        .await
        .expect("Unpin request failed");
    assert_eq!(unpin_resp.status(), 200);
}
