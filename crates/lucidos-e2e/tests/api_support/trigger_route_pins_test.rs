//! E2E coverage for clearing a trigger's model, reasoning effort and provider
//! pins over `PUT /api/v1/triggers`.
//!
//! Both null spellings must clear: JSON null, and the string `"null"`. The
//! string is the exact body `lucidos triggers update --model null` sends, since
//! a CLI flag can only carry a string.

use crate::support::{base_url, unique_marker, user_client};
use serde_json::json;

/// A schedule that never fires inside a test run, so the intent never runs.
const NEVER_SOON_CRON: &str = "0 0 3 1 1 *";

const PINS: [&str; 3] = ["model", "reasoning_effort", "provider"];

async fn find_trigger(client: &reqwest::Client, name: &str) -> serde_json::Value {
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/triggers", base_url()))
        .send()
        .await
        .expect("GET /triggers failed")
        .json()
        .await
        .expect("Invalid JSON");
    body["triggers"]
        .as_array()
        .expect("triggers array")
        .iter()
        .find(|t| t["name"] == name)
        .cloned()
        .expect("trigger is listed")
}

async fn update(client: &reqwest::Client, id: &str, body: serde_json::Value) -> serde_json::Value {
    client
        .put(format!("{}/api/v1/triggers?id={}", base_url(), id))
        .json(&body)
        .send()
        .await
        .expect("PUT /triggers failed")
        .json()
        .await
        .expect("Invalid JSON")
}

async fn assert_unpinned(client: &reqwest::Client, name: &str, how: &str) {
    let trigger = find_trigger(client, name).await;
    for pin in PINS {
        assert!(
            trigger.get(pin).is_none(),
            "{how} must clear {pin}: {trigger}"
        );
    }
}

#[tokio::test]
async fn both_null_spellings_clear_every_pin() {
    let client = user_client().await;
    let name = unique_marker("e2e-route-pins");
    let created: serde_json::Value = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&json!({
            "name": name,
            "run": { "type": "intent", "intent": "never executed: route pin test" },
            "cron_expressions": [NEVER_SOON_CRON],
            "model": "claude-sonnet-5",
            "reasoning_effort": "low",
        }))
        .send()
        .await
        .expect("POST /triggers failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert_eq!(created["success"], true, "create failed: {created}");
    let id = find_trigger(&client, &name).await["id"]
        .as_str()
        .expect("trigger id")
        .to_string();

    // Absent: a rename-only update keeps the pins.
    let renamed = format!("{name} renamed");
    let resp = update(&client, &id, json!({ "name": renamed })).await;
    assert_eq!(resp["success"], true, "rename failed: {resp}");
    let trigger = find_trigger(&client, &renamed).await;
    assert_eq!(trigger["model"], "claude-sonnet-5", "{trigger}");
    assert_eq!(trigger["reasoning_effort"], "low", "{trigger}");

    // The string "null", as the CLI sends it.
    let resp = update(
        &client,
        &id,
        json!({ "model": "null", "reasoning_effort": "null", "provider": "null" }),
    )
    .await;
    assert_eq!(resp["success"], true, "string null refused: {resp}");
    assert_unpinned(&client, &renamed, "the string \"null\"").await;

    // JSON null.
    let resp = update(
        &client,
        &id,
        json!({ "model": "claude-sonnet-5", "reasoning_effort": "high" }),
    )
    .await;
    assert_eq!(resp["success"], true, "re-pin failed: {resp}");
    let resp = update(
        &client,
        &id,
        json!({ "model": null, "reasoning_effort": null, "provider": null }),
    )
    .await;
    assert_eq!(resp["success"], true, "JSON null refused: {resp}");
    assert_unpinned(&client, &renamed, "JSON null").await;

    let _ = client
        .delete(format!("{}/api/v1/triggers?id={}", base_url(), id))
        .send()
        .await;
}
