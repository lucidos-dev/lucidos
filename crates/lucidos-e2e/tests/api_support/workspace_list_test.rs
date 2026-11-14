//! `GET /api/v1/workspaces`: this machine's workspaces, as the engine's own
//! gateway lists them.
//!
//! The request carries no local token, which is the sandboxed agent's position
//! (ADR 0645). The harness attaches a gateway identity only when a live gateway
//! lists this workspace (`workspace_label_test.rs` has the detail). So the
//! expected answer is read from the identity the engine holds, never assumed.
//!
//! The projection's edge cases live in the unit tests beside the handler,
//! `api/workspace_list.rs`.

use crate::support::{base_url, http_client};

/// The `content` of `<meta name="{name}" content="…">` in `html`, or `None`.
fn meta_content(html: &str, name: &str) -> Option<String> {
    let start = html.find(&format!("<meta name=\"{name}\" content=\""))?;
    let value = html[start..].split_once("content=\"")?.1.split_once('"')?.0;
    Some(value.to_string())
}

#[tokio::test]
async fn workspace_list_needs_no_token_and_carries_only_the_projection() {
    let client = http_client();
    let base = base_url();
    let shell = client
        .get(format!("{base}/"))
        .send()
        .await
        .expect("Shell request failed")
        .text()
        .await
        .expect("Shell body");
    let our_id = meta_content(&shell, "lucidos-workspace-id");
    let has_gateway = meta_content(&shell, "lucidos-gateway-port").is_some() && our_id.is_some();

    let resp = client
        .get(format!("{base}/api/v1/workspaces"))
        .send()
        .await
        .expect("Workspace list request failed");
    if !has_gateway {
        // An engine with no gateway has no list to project, and says so
        // rather than answering with an empty machine.
        assert_eq!(resp.status(), 503, "no gateway answers 503");
        return;
    }
    assert_eq!(resp.status(), 200, "the list answers with no local token");
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    let rows = body["workspaces"]
        .as_array()
        .unwrap_or_else(|| panic!("a workspaces array: {body}"));
    assert!(
        rows.iter().any(|w| w["id"].as_str() == our_id.as_deref()),
        "the list names this workspace: {body}"
    );
    for row in rows {
        let mut keys: Vec<_> = row.as_object().expect("an object row").keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "autostart",
                "health",
                "id",
                "machine_made",
                "name",
                "origin",
                "path",
                "port"
            ],
            "only the projected fields cross the boundary: {row}"
        );
    }
}
