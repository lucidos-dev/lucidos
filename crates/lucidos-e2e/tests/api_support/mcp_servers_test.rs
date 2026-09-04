//! E2E coverage for the `/api/v1/mcp` control surface.
//!
//! Why over HTTP rather than in-engine: the point of these routes is the status
//! code. A Backstage server with an unusable id locked a workspace out of chat.
//! The only removal path was the chat tool that server had killed. A DELETE
//! answering 200 for an id it never found would leave the page unable to say
//! anything useful. The engine-side unit tests cover the mapping; these check
//! the bytes actually sent.
//!
//! Registering a server is deliberately not reachable over HTTP, since it needs
//! a command and args and spawns a process. So these tests exercise the
//! unknown-id lane plus the read surfaces.

use crate::support::{base_url, unique_marker, user_client};
use serde_json::json;

/// An id no server holds. Every verb below has to say "not found" rather than
/// report success against nothing.
fn missing_id() -> String {
    unique_marker("e2e-absent-mcp")
}

#[tokio::test]
async fn delete_on_an_unknown_server_is_404() {
    let client = user_client().await;
    let api = base_url();
    let id = missing_id();

    let resp = client
        .delete(format!("{}/api/v1/mcp/servers/{}", api, id))
        .send()
        .await
        .expect("delete failed");

    // The regression this guards: `remove_server` used to return
    // `Ok("... not found")`, which behind a route is a 200 that did nothing.
    assert_eq!(
        resp.status(),
        404,
        "removing a server that does not exist must not report success"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap_or_default().contains(&id),
        "the error should name the id: {body}"
    );
}

#[tokio::test]
async fn start_and_stop_on_an_unknown_server_are_404() {
    let client = user_client().await;
    let api = base_url();
    let id = missing_id();

    for verb in ["start", "stop"] {
        let resp = client
            .post(format!("{}/api/v1/mcp/servers/{}/{}", api, id, verb))
            .json(&json!({}))
            .send()
            .await
            .unwrap_or_else(|e| panic!("{verb} failed: {e}"));
        assert_eq!(resp.status(), 404, "{verb} on an unknown id");
    }
}

#[tokio::test]
async fn setting_disabled_tools_on_an_unknown_server_is_404() {
    let client = user_client().await;
    let api = base_url();
    let id = missing_id();

    let resp = client
        .put(format!("{}/api/v1/mcp/servers/{}/disabled-tools", api, id))
        .json(&json!({ "disabled_tools": ["mcp__x__y"] }))
        .send()
        .await
        .expect("put failed");
    assert_eq!(resp.status(), 404);
}

/// The page reads cost off this response, so the shape is the contract: every
/// figure is engine-computed, and the model's window comes with it.
#[tokio::test]
async fn listing_servers_carries_totals_and_the_resolved_window() {
    let client = user_client().await;
    let api = base_url();

    let resp = client
        .get(format!("{}/api/v1/mcp/servers", api))
        .send()
        .await
        .expect("list failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();

    assert!(body["servers"].is_array(), "servers array: {body}");
    let totals = &body["totals"];
    for key in [
        "servers",
        "running_servers",
        "tools",
        "chars",
        "tokens",
        "stopped_tools",
        "stopped_chars",
        "stopped_tokens",
        "disabled_tools",
        "disabled_chars",
        "disabled_tokens",
    ] {
        assert!(
            totals[key].is_u64(),
            "totals.{key} should be a number: {totals}"
        );
    }

    // The model name can legitimately be empty, since a workspace with no
    // provider configured resolves to none. The window cannot: it is what the
    // request packer would size against either way, and a share of zero is
    // meaningless.
    assert!(body["model"].is_string(), "model: {body}");
    assert!(
        body["context_window"].as_u64().unwrap_or(0) > 0,
        "context_window: {body}"
    );

    // The counts have to agree with the array, or the header contradicts the
    // rows it sits above.
    let servers = body["servers"].as_array().unwrap();
    assert_eq!(totals["servers"].as_u64().unwrap(), servers.len() as u64);

    for server in servers {
        assert!(
            ["live", "cache", "never-observed"]
                .contains(&server["tools_source"].as_str().unwrap_or_default()),
            "unexpected tools_source: {server}"
        );
        assert!(server["dispatchable"].is_boolean());
    }
}

/// The MCP allowlist editor, the pair `cc-allowed-tools` already had. Without a
/// route the file was editable only by hand on the host.
#[tokio::test]
async fn mcp_allowed_tools_round_trips_over_http() {
    let client = user_client().await;
    let api = base_url();
    let url = format!("{}/api/v1/mcp-allowed-tools", api);

    let original: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("get failed")
        .json()
        .await
        .unwrap();
    let before = original["contents"].as_str().expect("contents").to_string();

    let pattern = format!("Mcp({}:*)", unique_marker("e2e-mcp"));
    let next = format!("{before}{pattern}\n");
    let resp = client
        .put(&url)
        .json(&json!({ "contents": next }))
        .send()
        .await
        .expect("put failed");
    assert_eq!(resp.status(), 204);

    let after: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("get failed")
        .json()
        .await
        .unwrap();
    assert!(
        after["contents"].as_str().unwrap().contains(&pattern),
        "the written pattern should read back"
    );

    // Put it back, so a rerun starts from the same file.
    client
        .put(&url)
        .json(&json!({ "contents": before }))
        .send()
        .await
        .expect("restore failed");
}
