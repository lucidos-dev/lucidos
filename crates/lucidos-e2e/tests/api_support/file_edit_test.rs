use crate::support::{base_url, unique_marker, user_client, workspace_path};

/// Write a file directly to the workspace data/artifacts/ directory (no git commit).
/// This avoids git commit races with other test modules that also touch the repo.
fn write_file_to_disk(path: &str, content: &str) {
    let full_path = workspace_path().join("data/artifacts").join(path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).expect("Failed to create parent dirs");
    }
    std::fs::write(&full_path, content).expect("Failed to write test file");
}

/// Read a file via GET /api/v1/data/artifacts/..., return body text
async fn read_artifact(client: &reqwest::Client, path: &str) -> String {
    let url = format!("{}/api/v1/data/artifacts/{}", base_url(), path);
    client
        .get(&url)
        .send()
        .await
        .expect("Read request failed")
        .text()
        .await
        .expect("Failed to read body")
}

/// POST /api/v1/data/edit with a JSON body
async fn post_file_edit(client: &reqwest::Client, body: serde_json::Value) -> reqwest::Response {
    let url = format!("{}/api/v1/data/edit", base_url());
    client
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("Edit request failed")
}

/// Test both JSON and text edit modes sequentially to avoid git commit races.
#[tokio::test]
async fn edit_json_and_text_modes() {
    let client = user_client().await;
    // Creates and then edits files in the shared working tree, both of which
    // show up in a command checkpoint's diff if they land mid-snapshot; see
    // `workspace_tree_lock`. Held for the whole test, which is short.
    let _tree = crate::support::workspace_tree_lock().read().await;

    // --- JSON mode ---
    let json_marker = unique_marker("edit-json");
    let json_path = format!("{}.json", json_marker);

    write_file_to_disk(&json_path, &format!(r#"{{"title": "{}"}}"#, json_marker));

    let resp = post_file_edit(
        &client,
        serde_json::json!({
            "path": format!("artifacts/{}", json_path),
            "operations": [{ "json_path": "title", "json_value": "updated" }]
        }),
    )
    .await;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(status, 200, "JSON mode failed: {:?}", body);
    assert_eq!(body["success"], true);

    let content = read_artifact(&client, &json_path).await;
    let doc: serde_json::Value = serde_json::from_str(&content).expect("Not valid JSON");
    assert_eq!(doc["title"], "updated");

    // --- Text mode ---
    let text_marker = unique_marker("edit-text");
    let text_path = format!("{}.md", text_marker);

    write_file_to_disk(&text_path, &format!("hello {}", text_marker));

    let resp = post_file_edit(
        &client,
        serde_json::json!({
            "path": format!("artifacts/{}", text_path),
            "operations": [{ "find": "hello", "replace": "goodbye" }]
        }),
    )
    .await;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(status, 200, "Text mode failed: {:?}", body);
    assert_eq!(body["success"], true);

    let content = read_artifact(&client, &text_path).await;
    assert!(
        content.contains("goodbye"),
        "Expected 'goodbye' in: {}",
        content
    );
    assert!(
        !content.contains("hello"),
        "Expected no 'hello' in: {}",
        content
    );
}

#[tokio::test]
async fn edit_empty_path_returns_400() {
    let client = user_client().await;
    let resp = post_file_edit(
        &client,
        serde_json::json!({
            "path": "",
            "operations": [{ "json_path": "title", "json_value": "x" }]
        }),
    )
    .await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn edit_path_traversal_returns_400() {
    let client = user_client().await;
    let resp = post_file_edit(
        &client,
        serde_json::json!({
            "path": "../etc/passwd",
            "operations": [{ "find": "root", "replace": "hacked" }]
        }),
    )
    .await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn edit_file_not_found_returns_400() {
    let client = user_client().await;
    let marker = unique_marker("edit-notfound");
    let resp = post_file_edit(
        &client,
        serde_json::json!({
            "path": format!("artifacts/{}-nonexistent.json", marker),
            "operations": [{ "json_path": "title", "json_value": "x" }]
        }),
    )
    .await;
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert!(body["error"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn edit_old_string_not_found_returns_400() {
    let client = user_client().await;
    let marker = unique_marker("edit-nomatch");
    let path = format!("{}.md", marker);

    // A new file in the shared working tree; see `workspace_tree_lock`.
    let _tree = crate::support::workspace_tree_lock().read().await;
    write_file_to_disk(&path, "some content");

    let resp = post_file_edit(
        &client,
        serde_json::json!({
            "path": format!("artifacts/{}", path),
            "operations": [{ "find": "nonexistent string", "replace": "replacement" }]
        }),
    )
    .await;
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert!(body["error"].as_str().unwrap().contains("not found"));
}
