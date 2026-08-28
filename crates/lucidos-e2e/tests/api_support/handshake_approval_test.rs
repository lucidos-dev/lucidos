//! The write-then-execute chain ADR 0144 closes, walked end to end over real
//! HTTP against a running workspace.
//!
//! Every step here is what an app UI can do from inside its iframe, using
//! nothing but documented routes. The point of the test is that the last step
//! refuses, and that nothing ran.

use crate::support::{base_url, http_client, workspace_path};

/// A file the script would create if it ever executed. Its absence is the
/// assertion that matters: a 502 with the script having already run would be
/// the same status code and a completely different outcome.
fn marker_path(name: &str) -> std::path::PathBuf {
    workspace_path().join(".lucidos/tmp").join(name)
}

fn planted_script(marker: &std::path::Path) -> String {
    format!(
        "import json, pathlib\n\
         pathlib.Path({:?}).parent.mkdir(parents=True, exist_ok=True)\n\
         pathlib.Path({:?}).write_text('pwned')\n\
         print(json.dumps({{\"headers\": {{}}, \"expires_in\": 60}}))\n",
        marker.display().to_string(),
        marker.display().to_string()
    )
}

async fn put_data(path: &str, body: String) -> u16 {
    let url = format!("{}/api/v1/data/{}", base_url(), path);
    http_client()
        .put(&url)
        .header("Content-Type", "text/plain")
        .body(body)
        .send()
        .await
        .expect("data write failed")
        .status()
        .as_u16()
}

async fn delete_data(path: &str) {
    let url = format!("{}/api/v1/data/{}", base_url(), path);
    let _ = http_client().delete(&url).send().await;
}

/// The chain, and the edit that must not re-bless a script, in one test.
///
/// Both halves write `data/config/apis.json`, which is one file per workspace.
/// As separate `#[tokio::test]`s they ran concurrently and deleted each other's
/// provider entry, so they are one sequence with one config write.
#[tokio::test]
async fn the_write_then_execute_chain_is_refused() {
    let planted = "e2e-handshake-guard";
    let edited = "e2e-handshake-edit";
    let marker = marker_path("e2e-handshake-pwned.txt");
    let _ = std::fs::remove_file(&marker);
    let planted_rel = format!("scripts/auth/{}.py", planted);
    let edited_rel = format!("scripts/auth/{}.py", edited);
    let benign = "import json\nprint(json.dumps({\"headers\": {}, \"expires_in\": 60}))\n";

    // Step 1: land the Python files. Allowed, deliberately: ADR 0144 guards
    // what runs, not what is written.
    assert_eq!(
        put_data(&planted_rel, planted_script(&marker)).await,
        200,
        "writing under scripts/ must keep working"
    );
    assert_eq!(put_data(&edited_rel, benign.to_string()).await, 200);

    // Step 2: point providers at them. Also allowed.
    let config = format!(
        r#"{{
          "{planted}": {{"base_url": "https://upstream.invalid", "auth": {{"pipeline": [
            {{"type": "script_handshake", "script": "{planted_rel}"}}
          ]}}}},
          "{edited}": {{"base_url": "https://upstream.invalid", "auth": {{"pipeline": [
            {{"type": "script_handshake", "script": "{edited_rel}"}}
          ]}}}}
        }}"#
    );
    assert_eq!(
        put_data("config/apis.json", config).await,
        200,
        "writing config/ must keep working"
    );

    // Step 3: the call that used to run the file as the engine user.
    let url = format!("{}/api/v1/proxy/{}/anything", base_url(), planted);
    let resp = http_client().get(&url).send().await.expect("proxy call");
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();

    assert_eq!(status, 502, "the chain must not complete; body: {body}");
    assert!(
        body.contains("not approved"),
        "the refusal must say why; got: {body}"
    );
    assert!(
        body.contains("lucidos handshake approve"),
        "the refusal must name the fix; got: {body}"
    );
    assert!(
        !marker.exists(),
        "the planted script executed: {}",
        marker.display()
    );

    // Now the trap that would have voided the whole guard.
    // `POST /api/v1/data/edit` runs the same `edit_file_at_path` the agent's
    // own `edit_file` tool runs. Had recording hung off that shared function,
    // an app editing a script over HTTP would have approved its own change.
    //
    // The e2e client is not a browser, so it may approve. That is the CLI's
    // path, standing in for the user having blessed this script.
    let approve = format!("{}/api/v1/handshake-scripts/approve", base_url());
    let resp = http_client()
        .post(&approve)
        .json(&serde_json::json!({ "path": edited_rel }))
        .send()
        .await
        .expect("approve call");
    assert_eq!(resp.status().as_u16(), 200, "approving must succeed");
    assert_eq!(
        approved_state(&edited_rel).await,
        Some(true),
        "the approval must land first, or the assertion below proves nothing"
    );

    let edit = format!("{}/api/v1/data/edit", base_url());
    let resp = http_client()
        .post(&edit)
        .json(&serde_json::json!({
            "path": edited_rel,
            "operations": [{ "find": "\"expires_in\": 60", "replace": "\"expires_in\": 61" }],
        }))
        .send()
        .await
        .expect("edit call");
    assert_eq!(resp.status().as_u16(), 200, "editing must keep working");
    assert_eq!(
        approved_state(&edited_rel).await,
        Some(false),
        "an API edit must not carry the old approval"
    );

    // Leave the workspace as we found it, so a rerun starts clean and no
    // later test inherits a provider pointing at an invalid host.
    delete_data("config/apis.json").await;
    delete_data(&planted_rel).await;
    delete_data(&edited_rel).await;
    let _ = std::fs::remove_file(&marker);
}

/// Whether the state route reports this script as approved, or `None` when it
/// does not list it at all.
async fn approved_state(script_rel: &str) -> Option<bool> {
    let url = format!("{}/api/v1/handshake-scripts", base_url());
    let body: serde_json::Value = http_client()
        .get(&url)
        .send()
        .await
        .expect("state call")
        .json()
        .await
        .expect("json");
    body["scripts"]
        .as_array()?
        .iter()
        .find(|r| r["path"] == format!("data/{}", script_rel))
        .and_then(|r| r["approved"].as_bool())
}

/// The read side of the approval record is open to any caller, including a
/// browser: the Files panel needs it to warn at edit time.
#[tokio::test]
async fn handshake_script_state_is_readable() {
    let url = format!("{}/api/v1/handshake-scripts", base_url());
    let resp = http_client().get(&url).send().await.expect("list failed");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert!(
        body.get("scripts").and_then(|s| s.as_array()).is_some(),
        "expected a scripts array, got: {body}"
    );
}

/// Approving is the act that lets a file run, so a browser-shaped caller is
/// refused. An app UI cannot suppress `Sec-Fetch-*`, so it cannot get past it.
#[tokio::test]
async fn approving_is_refused_to_a_browser() {
    let url = format!("{}/api/v1/handshake-scripts/approve", base_url());
    let resp = http_client()
        .post(&url)
        .header("sec-fetch-site", "same-origin")
        .header("sec-fetch-mode", "cors")
        .json(&serde_json::json!({ "path": "scripts/auth/anything.py" }))
        .send()
        .await
        .expect("approve call");
    assert_eq!(resp.status().as_u16(), 403);
    let body = resp.text().await.unwrap_or_default();
    assert!(
        body.contains("lucidos handshake approve"),
        "the refusal must name the route that works; got: {body}"
    );
}
