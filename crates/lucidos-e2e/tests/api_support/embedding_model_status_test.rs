use crate::support::{base_url, http_client};

const KNOWN_KINDS: &[&str] = &["downloading", "loading", "ready", "waiting", "failed"];

#[tokio::test]
async fn embedding_model_status_reports_a_known_state() {
    let client = http_client();
    let url = format!("{}/api/v1/memory/embedding-model-status", base_url());

    let resp = client
        .get(&url)
        .send()
        .await
        .expect("embedding-model status request failed");
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");

    let model_id = body["model_id"]
        .as_str()
        .expect("model_id must be a string");
    assert!(!model_id.is_empty(), "model_id must name the model");

    let kind = body["load_state"]["kind"]
        .as_str()
        .expect("load_state.kind must be a string");
    assert!(
        KNOWN_KINDS.contains(&kind),
        "unknown load state '{kind}'; the frontend union only handles {KNOWN_KINDS:?}"
    );

    if kind == "downloading" {
        assert!(
            body["load_state"]["downloaded_bytes"].is_u64(),
            "a downloading state must report downloaded_bytes: {body}"
        );
        assert!(
            body["load_state"]["total_bytes"].is_u64(),
            "a downloading state must report total_bytes: {body}"
        );
    }
}
