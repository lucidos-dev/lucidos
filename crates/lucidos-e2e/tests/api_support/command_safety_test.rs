//! E2E coverage for the **Lucidos Agent command safety** HTTP surface (ADR
//! 0002). Two contracts are exercised over real HTTP against the booted e2e
//! workspace:
//!
//! 1. The trigger **side-effect grant** (Phase 5) round-trips through
//!    `POST/PUT/GET /api/v1/triggers` as a typed `SideEffectCategory` set, and an
//!    unknown category is rejected at the serde boundary.
//! 2. The command-checkpoint **undo** endpoint (Phase 4) fails closed with an
//!    `[ERROR]`-prefixed body when handed an unknown checkpoint id.
//!
//! Neither path needs the `command_guard` preference on: the grant field is
//! stored on the trigger regardless of the guard, and the undo endpoint resolves
//! the checkpoint from the event store directly.

use crate::support::{base_url, http_client, unique_marker};
use serde_json::json;

/// Ensure a timezone preference exists so `create_trigger` doesn't have to fall
/// back to its UTC default. The `set_preference` handler reads the key from the
/// `?key=` query param (not the body), so pass it there with `{value}` in the
/// body — the body-only `{key, value}` form is silently a no-op.
async fn set_timezone_utc(client: &reqwest::Client) {
    let _ = client
        .put(format!("{}/api/v1/preferences?key=timezone", base_url()))
        .json(&json!({ "value": "UTC" }))
        .send()
        .await;
}

/// Create a schedule trigger carrying `grant` and assert the API accepted it.
async fn create_trigger_with_grant(client: &reqwest::Client, name: &str, grant: &[&str]) {
    let body = json!({
        "name": name,
        "run": { "type": "intent", "intent": "noop" },
        "cron_expressions": ["0 0 8 * * *"],
        "side_effect_grant": grant,
    });
    let resp = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&body)
        .send()
        .await
        .expect("POST /triggers failed");
    let res: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(
        res["success"], true,
        "Trigger create must succeed: {:?}",
        res
    );
}

/// Replace the side-effect grant of an existing trigger.
async fn update_trigger_grant(client: &reqwest::Client, trigger_id: &str, grant: &[&str]) {
    let resp = client
        .put(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
        .json(&json!({ "side_effect_grant": grant }))
        .send()
        .await
        .expect("PUT /triggers failed");
    let res: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(
        res["success"], true,
        "Trigger update must succeed: {:?}",
        res
    );
}

async fn delete_trigger(client: &reqwest::Client, trigger_id: &str) {
    let _ = client
        .delete(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
        .send()
        .await;
}

/// Read the `side_effect_grant` of one trigger as a sorted Vec. The TriggerInfo
/// wire shape omits the field when empty (`skip_serializing_if = "Vec::is_empty"`),
/// so an absent array reads back as the empty grant.
fn grant_of(trigger: &serde_json::Value) -> Vec<String> {
    let mut v: Vec<String> = trigger["side_effect_grant"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Poll `GET /api/v1/triggers` until the trigger named `name` has the expected
/// (sorted) grant. POST/PUT return once the event is persisted, but the
/// in-memory scheduler projection the list endpoint reads updates
/// asynchronously — under the suite's parallel load a single GET races the
/// subscriber, so poll until it catches up. Returns the trigger's id.
async fn poll_trigger_grant(
    client: &reqwest::Client,
    name: &str,
    expected_sorted: &[&str],
) -> String {
    let mut want: Vec<String> = expected_sorted.iter().map(|s| s.to_string()).collect();
    want.sort();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let listed: serde_json::Value = client
            .get(format!("{}/api/v1/triggers", base_url()))
            .send()
            .await
            .expect("GET /triggers failed")
            .json()
            .await
            .expect("Invalid JSON");
        let found = listed["triggers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"].as_str() == Some(name))
            .cloned();
        if let Some(ref t) = found {
            if grant_of(t) == want {
                return t["id"].as_str().unwrap().to_string();
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Trigger '{}' grant did not become {:?} within 5s (last saw {:?})",
            name,
            want,
            found.as_ref().map(grant_of),
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Create → GET → update (replace) → update (clear) round-trip of the typed
/// side-effect grant set.
#[tokio::test]
async fn trigger_side_effect_grant_round_trips() {
    let client = http_client();
    set_timezone_utc(&client).await;
    let name = unique_marker("e2e-grant");

    // Create with two categories; GET must round-trip them.
    create_trigger_with_grant(&client, &name, &["email", "external_api"]).await;
    let trigger_id = poll_trigger_grant(&client, &name, &["email", "external_api"]).await;

    // Replace with a different single category.
    update_trigger_grant(&client, &trigger_id, &["cloud_cli"]).await;
    poll_trigger_grant(&client, &name, &["cloud_cli"]).await;

    // Clear it: an empty array drops the grant entirely.
    update_trigger_grant(&client, &trigger_id, &[]).await;
    poll_trigger_grant(&client, &name, &[]).await;

    delete_trigger(&client, &trigger_id).await;
}

/// `side_effect_grant` is a typed serde enum, so an unknown category fails to
/// deserialize at the `Json` extractor before the handler runs. Note: axum
/// 0.7's `Json` rejects a valid-JSON-but-wrong-shape body (a `JsonDataError`)
/// with **422 Unprocessable Entity** — 400 is reserved for malformed JSON
/// *syntax* (`JsonSyntaxError`). Either way the unknown category never reaches
/// the scheduler.
#[tokio::test]
async fn trigger_create_rejects_unknown_side_effect_category() {
    let client = http_client();
    set_timezone_utc(&client).await;

    let body = json!({
        "name": unique_marker("e2e-bad-grant"),
        "run": { "type": "intent", "intent": "noop" },
        "cron_expressions": ["0 0 8 * * *"],
        "side_effect_grant": ["yolo"],
    });
    let resp = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&body)
        .send()
        .await
        .expect("POST /triggers failed");
    assert_eq!(
        resp.status().as_u16(),
        422,
        "Unknown side-effect category must be rejected at the serde boundary (422)"
    );
}

/// The same rejection applies to PUT: an unknown category can't sneak in via an
/// update either.
#[tokio::test]
async fn trigger_update_rejects_unknown_side_effect_category() {
    let client = http_client();
    set_timezone_utc(&client).await;
    let name = unique_marker("e2e-bad-update");

    // A real trigger to target.
    create_trigger_with_grant(&client, &name, &["email"]).await;
    let trigger_id = poll_trigger_grant(&client, &name, &["email"]).await;

    let resp = client
        .put(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
        .json(&json!({ "side_effect_grant": ["not_a_category"] }))
        .send()
        .await
        .expect("PUT /triggers failed");
    assert_eq!(
        resp.status().as_u16(),
        422,
        "Unknown side-effect category on update must be rejected (422)"
    );

    // The bad update must not have mutated the stored grant.
    poll_trigger_grant(&client, &name, &["email"]).await;
    delete_trigger(&client, &trigger_id).await;
}

/// `POST /api/v1/command-checkpoint/undo` with an unknown id fails closed: the
/// engine can't find a matching `CommandCheckpointed` event, so the handler
/// returns 400 with an `[ERROR]`-prefixed body (mirrors the change
/// discard/revert endpoints).
#[tokio::test]
async fn command_checkpoint_undo_unknown_id_returns_error() {
    let client = http_client();
    let unknown_id = uuid::Uuid::new_v4().to_string();

    let resp = client
        .post(format!("{}/api/v1/command-checkpoint/undo", base_url()))
        .json(&json!({ "checkpoint_id": unknown_id }))
        .send()
        .await
        .expect("POST /command-checkpoint/undo failed");

    assert_eq!(
        resp.status().as_u16(),
        400,
        "Unknown checkpoint id must return 400 Bad Request"
    );
    let body = resp.text().await.expect("Failed to read body");
    assert!(
        body.starts_with("[ERROR]"),
        "Error body must use the [ERROR] prefix, got: {body:?}"
    );
    assert!(
        body.contains("no command checkpoint"),
        "Error body should name the missing checkpoint, got: {body:?}"
    );
}

/// A malformed body (missing the required `checkpoint_id`) is rejected before
/// the handler runs — a guard against the endpoint silently treating an empty
/// id as a valid no-op.
#[tokio::test]
async fn command_checkpoint_undo_missing_id_is_rejected() {
    let client = http_client();
    let resp = client
        .post(format!("{}/api/v1/command-checkpoint/undo", base_url()))
        .json(&json!({}))
        .send()
        .await
        .expect("POST /command-checkpoint/undo failed");
    let status = resp.status().as_u16();
    assert!(
        (400..500).contains(&status),
        "Missing checkpoint_id should return 4xx, got {status}"
    );
}
