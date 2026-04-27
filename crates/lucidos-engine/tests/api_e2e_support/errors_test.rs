use crate::support::{base_url, http_client};

#[tokio::test]
#[ignore]
async fn unknown_route_returns_non_json() {
    let client = http_client();
    // Unknown /api routes fall through to the Vite SPA proxy, returning HTML.
    // Verify the response is NOT valid JSON (it's the SPA fallback, not an API response).
    let url = format!("{}/api/nonexistent-endpoint", base_url());

    let resp = client.get(&url).send().await.expect("Request failed");
    let body = resp.text().await.expect("Failed to read body");
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&body);
    assert!(
        parsed.is_err(),
        "Unknown API route should not return valid JSON"
    );
}

#[tokio::test]
#[ignore]
async fn malformed_chat_body_returns_error() {
    let client = http_client();
    let url = format!("{}/api/chat/stream", base_url());

    // Send invalid JSON
    let resp = client
        .post(&url)
        .header("content-type", "application/json")
        .body("{invalid json}")
        .send()
        .await
        .expect("Request failed");

    let status = resp.status().as_u16();
    // Should be 4xx (bad request), not 5xx (server error)
    assert!(
        (400..500).contains(&status),
        "Malformed JSON should return 4xx, got {status}"
    );
}

#[tokio::test]
#[ignore]
async fn missing_content_type_for_chat_returns_error() {
    let client = http_client();
    let url = format!("{}/api/chat/stream", base_url());

    // Send without content-type header
    let resp = client
        .post(&url)
        .body("not json")
        .send()
        .await
        .expect("Request failed");

    let status = resp.status().as_u16();
    assert!(
        (400..500).contains(&status),
        "Missing content-type should return 4xx, got {status}"
    );
}

#[tokio::test]
#[ignore]
async fn get_on_post_only_endpoint_returns_405() {
    let client = http_client();
    let url = format!("{}/api/chat/stream", base_url());

    let resp = client.get(&url).send().await.expect("Request failed");
    let status = resp.status().as_u16();
    // Should be 405 Method Not Allowed or 404
    assert!(
        status == 405 || status == 404,
        "GET on POST-only endpoint should return 405 or 404, got {status}"
    );
}

#[tokio::test]
#[ignore]
async fn thread_messages_for_nonexistent_thread() {
    let client = http_client();
    let url = format!(
        "{}/api/threads/00000000-0000-0000-0000-000000000000/messages",
        base_url()
    );

    let resp = client.get(&url).send().await.expect("Request failed");
    let status = resp.status().as_u16();
    // Should return 200 with empty array or 404, not 500
    assert!(
        status == 200 || status == 404,
        "Nonexistent thread should return 200 (empty) or 404, got {status}"
    );
}
