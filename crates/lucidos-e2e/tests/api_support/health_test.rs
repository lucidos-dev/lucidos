use crate::support::{base_url, http_client};

#[tokio::test]
async fn health_returns_ok() {
    let client = http_client();
    let url = format!("{}/api/v1/health", base_url());

    let resp = client
        .get(&url)
        .send()
        .await
        .expect("Health request failed");
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(body["status"], "ok");
    assert!(body["workspace"].is_string());
    assert!(body["workspace_path"].is_string());
    assert!(body["engine_version"].is_string());
    assert_eq!(body["release"], lucidos_engine::LUCIDOS_RELEASE);
}

#[tokio::test]
async fn health_has_expected_fields() {
    let client = http_client();
    let url = format!("{}/api/v1/health", base_url());

    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("Health request failed")
        .json()
        .await
        .expect("Invalid JSON");

    // All required fields must be present and non-null
    for field in &[
        "status",
        "workspace",
        "workspace_path",
        "started_at",
        "engine_version",
        "release",
        "release_dirty",
        // packaged-mode signal — drives the frontend's Restart routing.
        "packaged",
    ] {
        assert!(
            body.get(field).is_some() && !body[field].is_null(),
            "Missing field: {field}"
        );
    }
    // The e2e engine is built from the source checkout (it serves the built
    // `dist/` via LUCIDOS_STATIC_DIR, but that is no longer the packaged signal —
    // ADR 0014 makes dev engines set it too). `is_packaged()` keys off the
    // presence of a source checkout, so a source-built engine is `false`.
    assert_eq!(body["packaged"], false, "e2e engine is not a packaged build");
}
