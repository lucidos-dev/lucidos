//! Snapshot route boundaries exercise the actual router without opening a key
//! store or capturing data. Capture integrity uses disposable service fixtures.
use crate::support::{base_url, http_client, local_process_client};
use serde_json::{json, Value};

#[tokio::test]
async fn workspace_snapshots_require_identity_for_every_surface() {
    let root = format!("{}/api/v1/snapshots", base_url());
    let handle = "snap_00000000000000000000000000";
    for (method, path) in [
        (reqwest::Method::GET, root.clone()),
        (reqwest::Method::POST, root.clone()),
        (reqwest::Method::GET, format!("{root}/{handle}")),
        (reqwest::Method::PATCH, format!("{root}/{handle}")),
        (reqwest::Method::POST, format!("{root}/{handle}/check")),
        (reqwest::Method::DELETE, format!("{root}/{handle}")),
        (
            reqwest::Method::GET,
            format!("{root}/jobs/{}", uuid::Uuid::new_v4()),
        ),
    ] {
        let response = http_client()
            .request(method, &path)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 401, "{path}");
    }
}

#[tokio::test]
async fn workspace_snapshots_status_needs_no_snapshot_key_or_archive_slot() {
    let response = local_process_client()
        .get(format!(
            "{}/api/v1/snapshots/jobs/{}",
            base_url(),
            uuid::Uuid::new_v4()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], "snapshot_job_unavailable");
}

#[tokio::test]
async fn workspace_snapshots_reject_invalid_handles_before_store_access() {
    let response = local_process_client()
        .post(format!(
            "{}/api/v1/snapshots/not-a-handle/check",
            base_url()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
}
