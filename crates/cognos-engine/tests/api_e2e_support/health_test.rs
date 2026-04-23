use crate::support::{base_url, http_client};

#[tokio::test]
#[ignore]
async fn health_returns_ok() {
    let client = http_client();
    let url = format!("{}/api/health", base_url());

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
    assert_eq!(body["release"], cognos_engine::LUCIDOS_RELEASE);
}

#[tokio::test]
#[ignore]
async fn health_has_expected_fields() {
    let client = http_client();
    let url = format!("{}/api/health", base_url());

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
    ] {
        assert!(
            body.get(field).is_some() && !body[field].is_null(),
            "Missing field: {field}"
        );
    }
}
