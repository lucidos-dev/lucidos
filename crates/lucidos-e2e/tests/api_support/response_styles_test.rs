//! E2E coverage for the *style library*: `GET /api/v1/response-styles` and the
//! `PUT /api/v1/preferences` write that edits it.
//!
//! The read route exists so the shipped instructions have ONE home, in Rust.
//! The Settings editor renders what this returns, and resets a style by
//! dropping its override rather than by re-sending remembered text. So the
//! contract worth pinning over HTTP is the round trip: edit, read back the
//! override, drop it, read back the shipped text.
//!
//! **One test, walking the whole lifecycle.** The library is a single
//! workspace-global preference, and `cargo test` runs its cases in parallel, so
//! four tests editing it would clobber each other's documents. That is the
//! shared-seeded-row hazard in `.claude/rules/testing.md`, reached through a
//! preference rather than a table.

use crate::support::{base_url, user_client};
use serde_json::json;

/// The merged library as the Settings editor sees it.
async fn library(client: &reqwest::Client, api: &str) -> Vec<serde_json::Value> {
    let resp = client
        .get(format!("{}/api/v1/response-styles", api))
        .send()
        .await
        .expect("list response styles failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    body["styles"]
        .as_array()
        .expect("styles array")
        .clone()
        .to_vec()
}

fn row(styles: &[serde_json::Value], id: &str) -> serde_json::Value {
    styles
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("{id} must be in the library"))
        .clone()
}

/// Write the whole document, and answer with the engine's verdict.
///
/// The only way the library is edited: the read route is read-only on purpose.
///
/// The verdict is in the BODY. `PUT /preferences` answers a refusal with
/// `200 {success: false, error}` rather than a 4xx, so a test reading the
/// status alone would call every refusal a save.
async fn put_document(client: &reqwest::Client, api: &str, document: &str) -> serde_json::Value {
    let resp = client
        .put(format!("{}/api/v1/preferences?key=response_styles", api))
        .json(&json!({ "value": document }))
        .send()
        .await
        .expect("preference write failed");
    assert_eq!(resp.status(), 200);
    resp.json().await.expect("an ApiResult body")
}

/// Write a document the engine must accept.
async fn put_ok(client: &reqwest::Client, api: &str, document: &str) {
    let result = put_document(client, api, document).await;
    assert_eq!(result["success"], json!(true), "refused: {result}");
}

#[tokio::test]
async fn the_style_library_round_trips_over_http() {
    let client = user_client().await;
    let api = base_url();

    // ---- Shipped state ----
    put_ok(&client, &api, "[]").await;
    let styles = library(&client, &api).await;
    let ids: Vec<&str> = styles.iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["standard", "concise", "minimal"]);

    // Standard is the off switch: no instruction, and no editor.
    let standard = row(&styles, "standard");
    assert_eq!(standard["editable"], json!(false));
    assert_eq!(standard["instruction"], json!(""));
    assert_eq!(standard["source"], json!("builtin"));

    let shipped_minimal = row(&styles, "minimal")["instruction"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(row(&styles, "concise")["instruction"]
        .as_str()
        .unwrap()
        .contains("Lead with the answer"));

    // ---- Editing a shipped style writes an override ----
    put_ok(
        &client,
        &api,
        &json!([{
            "id": "minimal",
            "label": "Terse",
            "instruction": "- One sentence, and only one.",
        }])
        .to_string(),
    )
    .await;

    let edited = row(&library(&client, &api).await, "minimal");
    assert_eq!(edited["label"], json!("Terse"));
    assert_eq!(edited["source"], json!("overridden"));
    assert_eq!(
        edited["instruction"],
        json!("- One sentence, and only one.")
    );

    // ---- Reset drops the override, and the engine answers with its own text ----
    // A deletion, never a re-send: the client never held the shipped wording.
    put_ok(&client, &api, "[]").await;
    let reset = row(&library(&client, &api).await, "minimal");
    assert_eq!(reset["label"], json!("Minimal"));
    assert_eq!(reset["source"], json!("builtin"));
    assert_eq!(reset["instruction"], json!(shipped_minimal));

    // ---- A style of the user's own, with a derived description ----
    put_ok(
        &client,
        &api,
        &json!([{
            "id": "board-report",
            "label": "Board report",
            "instruction": "- Three bullets, no more.\n- No hedging.",
        }])
        .to_string(),
    )
    .await;

    let styles = library(&client, &api).await;
    assert_eq!(styles.len(), 4);
    let mine = row(&styles, "board-report");
    assert_eq!(mine["source"], json!("user"));
    assert_eq!(mine["editable"], json!(true));
    assert_eq!(mine["description"], json!("Three bullets, no more."));

    // ---- A refused edit changes nothing ----
    // The half only HTTP can show: the validator runs BEFORE the store write,
    // so a rejected document cannot leave a half-saved library behind.
    let rejected = [
        // Not JSON at all.
        "{not json".to_string(),
        // The off switch is not editable.
        json!([{ "id": "standard", "label": "Loud", "instruction": "- Twice." }]).to_string(),
        // An id that is not kebab-case.
        json!([{ "id": "Board Report", "label": "X", "instruction": "- y" }]).to_string(),
        // An instruction past its 1,000-character bound.
        json!([{ "id": "long", "label": "Long", "instruction": "x".repeat(1_001) }]).to_string(),
    ];
    for document in rejected {
        let result = put_document(&client, &api, &document).await;
        assert_eq!(
            result["success"],
            json!(false),
            "the engine accepted a document it should refuse: {document}"
        );
        // A refusal has to say what was wrong, or the editor can only report
        // that something was.
        assert!(
            result["error"].as_str().is_some_and(|e| !e.is_empty()),
            "a refusal with no reason: {result}"
        );
    }

    let styles = library(&client, &api).await;
    assert_eq!(row(&styles, "board-report")["label"], json!("Board report"));
    assert_eq!(row(&styles, "standard")["instruction"], json!(""));

    // Leave the workspace on the shipped library.
    put_ok(&client, &api, "[]").await;
}
