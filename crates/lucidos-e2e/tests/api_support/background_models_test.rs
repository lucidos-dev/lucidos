//! E2E coverage for `GET /api/v1/models/background`, which Settings reads to
//! show the model each background task runs on.

use crate::support::{base_url, user_client};

/// The e2e engine runs on the mock, and so do its background calls: every row
/// resolves to it as the chat model, reachable, at a tier, with a recommended
/// list for the picker.
#[tokio::test]
async fn every_background_row_resolves_on_the_mock_engine() {
    let client = user_client().await;
    let resp = client
        .get(format!("{}/api/v1/models/background", base_url()))
        .send()
        .await
        .expect("request failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json body");
    let rows = body.as_object().expect("rows keyed by preference");
    assert!(!rows.is_empty(), "no background rows: {body}");
    for (key, row) in rows {
        assert_eq!(row["model"], "mock", "{key}: {row}");
        assert_eq!(row["source"], "chat-model", "{key}: {row}");
        assert_eq!(row["reachable"], true, "{key}: {row}");
        // Nothing has answered not-found on a fresh engine (ADR 0403). Pins
        // the wire field the Settings caveat reads.
        assert_eq!(row["not_served"], serde_json::json!([]), "{key}: {row}");
        assert!(
            row["effort"].as_str().is_some_and(|e| !e.is_empty()),
            "{key}: {row}"
        );
        assert!(
            row["recommended"].as_array().is_some_and(|r| !r.is_empty()),
            "{key} recommends nothing: {row}"
        );
    }
}
