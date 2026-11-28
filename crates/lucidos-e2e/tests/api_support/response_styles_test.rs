use crate::support::{base_url, user_client};
use lucidos_engine::core::prefs;
use serde_json::json;

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

async fn put_document(client: &reqwest::Client, api: &str, document: &str) -> serde_json::Value {
    let resp = client
        .put(format!(
            "{}/api/v1/preferences?key={}",
            api,
            prefs::RESPONSE_STYLES.key()
        ))
        .json(&json!({ "value": document }))
        .send()
        .await
        .expect("preference write failed");
    assert_eq!(resp.status(), 200);
    resp.json().await.expect("an ApiResult body")
}

async fn put_ok(client: &reqwest::Client, api: &str, document: &str) {
    let result = put_document(client, api, document).await;
    assert_eq!(result["success"], json!(true), "refused: {result}");
}

#[tokio::test]
async fn the_style_library_round_trips_over_http() {
    let client = user_client().await;
    let api = base_url();

    put_ok(&client, &api, "[]").await;
    let styles = library(&client, &api).await;
    let ids: Vec<&str> = styles.iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["standard", "concise", "minimal", "learning"]);

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

    let shipped_description = row(&styles, "minimal")["description"].clone();
    let edited = row(&library(&client, &api).await, "minimal");
    assert_eq!(edited["label"], json!("Terse"));
    assert_eq!(edited["source"], json!("overridden"));
    assert_eq!(edited["description"], shipped_description);
    assert_eq!(
        edited["instruction"],
        json!("- One sentence, and only one.")
    );

    put_ok(&client, &api, "[]").await;
    let reset = row(&library(&client, &api).await, "minimal");
    assert_eq!(reset["label"], json!("Minimal"));
    assert_eq!(reset["source"], json!("builtin"));
    assert_eq!(reset["instruction"], json!(shipped_minimal));

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
    assert_eq!(styles.len(), 5);
    let mine = row(&styles, "board-report");
    assert_eq!(mine["source"], json!("user"));
    assert_eq!(mine["editable"], json!(true));
    assert_eq!(mine["description"], json!("Three bullets, no more."));

    let rejected = [
        "{not json".to_string(),
        json!([{ "id": "standard", "label": "Loud", "instruction": "- Twice." }]).to_string(),
        json!([{ "id": "Board Report", "label": "X", "instruction": "- y" }]).to_string(),
        json!([{ "id": "long", "label": "Long", "instruction": "x".repeat(1_001) }]).to_string(),
    ];
    for document in rejected {
        let result = put_document(&client, &api, &document).await;
        assert_eq!(
            result["success"],
            json!(false),
            "the engine accepted a document it should refuse: {document}"
        );
        assert!(
            result["error"].as_str().is_some_and(|e| !e.is_empty()),
            "a refusal with no reason: {result}"
        );
    }

    let styles = library(&client, &api).await;
    assert_eq!(row(&styles, "board-report")["label"], json!("Board report"));
    assert_eq!(row(&styles, "standard")["instruction"], json!(""));

    put_ok(&client, &api, "[]").await;
}
