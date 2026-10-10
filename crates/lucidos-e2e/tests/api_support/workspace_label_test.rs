//! `GET /api/v1/workspace-label`: the display name a direct-port page asks for.
//!
//! The harness gives the e2e engine a gateway identity only when a live gateway
//! lists this workspace at the engine's port. Otherwise the engine has none
//! (ADR 0385, `scripts/lib/e2e_gateway.sh`). A local run usually finds the dev
//! gateway; a GitHub API shard has none. So the expected label is read from the identity
//! the engine actually holds, never assumed.
//!
//! The engine stamps that identity into the shell it serves, as the
//! `lucidos-gateway-port` and `lucidos-workspace-id` metas. With both, the label
//! must be the name the gateway lists for our own slug at our own port. With
//! neither, it must be null: 200 with a null label, not 404 and not a 500.
//!
//! The projection's edge cases (another workspace's row, a slug at the wrong
//! port) live in the Rust unit tests beside the handler, `api/workspace_label.rs`.

use crate::support::{base_url, http_client, local_process_client};

/// The `content` of `<meta name="{name}" content="…">` in `html`, or `None`.
fn meta_content(html: &str, name: &str) -> Option<String> {
    let start = html.find(&format!("<meta name=\"{name}\" content=\""))?;
    let rest = &html[start..];
    let value = rest.split_once("content=\"")?.1.split_once('"')?.0;
    Some(value.to_string())
}

/// The name the gateway on `gateway_port` lists for `workspace_id`. Panics if
/// the gateway does not list it at `engine_port`, since the harness attaches an
/// identity only when it does.
async fn listed_name(gateway_port: &str, workspace_id: &str, engine_port: u16) -> String {
    let client = local_process_client();
    for scheme in ["https", "http"] {
        let url = format!("{scheme}://127.0.0.1:{gateway_port}/~/api/v1/control/workspaces");
        let Ok(resp) = client.get(&url).send().await else {
            continue;
        };
        let listing: serde_json::Value = resp.json().await.expect("listing is JSON");
        let row = listing["workspaces"]
            .as_array()
            .expect("listing carries a workspaces array")
            .iter()
            .find(|w| w["id"] == workspace_id)
            .unwrap_or_else(|| panic!("gateway does not list '{workspace_id}': {listing}"));
        assert_eq!(
            row["port"], engine_port,
            "the harness attached this gateway, so it lists us at our port: {row}"
        );
        return row["name"]
            .as_str()
            .expect("row carries a name")
            .to_string();
    }
    panic!("gateway on :{gateway_port} unreachable on either scheme");
}

#[tokio::test]
async fn workspace_label_is_the_name_the_engines_own_gateway_lists() {
    let client = http_client();
    let base = base_url();
    let engine_port = reqwest::Url::parse(&base)
        .expect("base url parses")
        .port()
        .expect("base url carries the engine port");

    let shell = client
        .get(format!("{base}/"))
        .send()
        .await
        .expect("Shell request failed");
    // A shell with no metas reads as "no gateway", so a missing one must fail.
    assert_eq!(shell.status(), 200, "the engine serves its shell");
    let shell = shell.text().await.expect("Shell body");
    let expected = match (
        meta_content(&shell, "lucidos-gateway-port"),
        meta_content(&shell, "lucidos-workspace-id"),
    ) {
        (Some(gateway_port), Some(id)) => Some(listed_name(&gateway_port, &id, engine_port).await),
        _ => None,
    };

    let resp = client
        .get(format!("{base}/api/v1/workspace-label"))
        .send()
        .await
        .expect("Workspace label request failed");
    assert_eq!(
        resp.status(),
        200,
        "the label route answers with or without a gateway"
    );

    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    // The key must be PRESENT, even when null: the client reads `body.label`,
    // and an engine that answered `{}` would look identical to one that
    // answered a label of `undefined`.
    assert!(
        body.get("label").is_some(),
        "response carries a `label` key: {body}"
    );
    assert_eq!(
        body["label"].as_str(),
        expected.as_deref(),
        "the label is our own gateway's name for us, or null with no gateway: {body}"
    );
}

#[tokio::test]
async fn workspace_label_never_leaks_the_workspace_listing() {
    // The engine resolves the label from the gateway's control listing, which
    // names every workspace on the machine. Only this workspace's own name may
    // cross the boundary, so the response is one key and nothing else.
    let client = http_client();
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/workspace-label", base_url()))
        .send()
        .await
        .expect("Workspace label request failed")
        .json()
        .await
        .expect("Invalid JSON");

    let obj = body.as_object().expect("an object response");
    assert_eq!(
        obj.keys().collect::<Vec<_>>(),
        vec!["label"],
        "exactly one key, so no listing row can ride along: {body}"
    );
}
