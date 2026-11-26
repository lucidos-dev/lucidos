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
    assert_eq!(
        body["release"],
        include_str!("../../../../RELEASE").trim_ascii_end()
    );
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

    for field in &[
        "status",
        "workspace",
        "workspace_path",
        "started_at",
        "engine_version",
        "release",
        "release_dirty",
        "packaged",
        "database_reachable",
        "database_pool_exhausted",
    ] {
        assert!(
            body.get(field).is_some() && !body[field].is_null(),
            "Missing field: {field}"
        );
    }
    assert_eq!(
        body["packaged"], false,
        "e2e engine is not a packaged build"
    );
    assert_eq!(
        body["database_reachable"], true,
        "a booted e2e engine can reach its database"
    );
    let sandbox = &body["agent_sandbox_unavailable"];
    assert!(
        body.get("agent_sandbox_unavailable").is_some()
            && (sandbox.is_null() || sandbox.is_string()),
        "agent_sandbox_unavailable must be null or a reason, got {sandbox}"
    );
    let unguarded = &body["agent_sandbox_truncation_unguarded"];
    assert!(
        unguarded.is_boolean(),
        "agent_sandbox_truncation_unguarded must be a bool, got {unguarded}"
    );
    let metadata = &body["agent_sandbox_metadata_unguarded"];
    assert!(
        metadata.is_boolean(),
        "agent_sandbox_metadata_unguarded must be a bool, got {metadata}"
    );
}
