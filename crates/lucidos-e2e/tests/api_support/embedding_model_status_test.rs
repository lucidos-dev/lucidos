//! The embedding-model status snapshot.
//!
//! The live signal is the transient `EmbeddingModelStatusChanged` SSE frame, but
//! those are never replayed, so a client that connects mid-download has missed
//! all of them. On a fresh workspace that IS the normal case: the ~465 MB fetch
//! starts at engine boot, seconds before the app document exists. This endpoint
//! is how such a client catches up, which makes its availability and its shape
//! the contract worth pinning here.

use crate::support::{base_url, http_client};

/// Every `kind` the loader can report. The frontend switches on this exact set
/// (`EmbeddingModelLoadState` in `api/types.ts`), so an unrecognised value would
/// leave the badge and the status toast showing nothing.
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

    // Names WHICH model the reading is about, so it stays unambiguous across a
    // configuration change.
    let model_id = body["model_id"]
        .as_str()
        .expect("model_id must be a string");
    assert!(!model_id.is_empty(), "model_id must name the model");

    // The discriminator the frontend switches on, alongside the tag on the same
    // object (not nested), which is what lets the progress bar read the byte
    // counts straight off it.
    let kind = body["load_state"]["kind"]
        .as_str()
        .expect("load_state.kind must be a string");
    assert!(
        KNOWN_KINDS.contains(&kind),
        "unknown load state '{kind}'; the frontend union only handles {KNOWN_KINDS:?}"
    );

    // A download in flight must carry both counts, or there is nothing to draw.
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
