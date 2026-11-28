use crate::support::{base_url, http_client, local_process_client};

fn meta_content(html: &str, name: &str) -> Option<String> {
    let start = html.find(&format!("<meta name=\"{name}\" content=\""))?;
    let rest = &html[start..];
    let value = rest.split_once("content=\"")?.1.split_once('"')?.0;
    Some(value.to_string())
}

async fn listed_name(gateway_port: &str, workspace_id: &str, engine_port: u16) -> String {
    let origin = lucidos_local_token::client::origin(gateway_port);
    let listing: serde_json::Value = local_process_client()
        .get(format!("{origin}/~/api/v1/control/workspaces"))
        .send()
        .await
        .unwrap_or_else(|e| panic!("gateway on :{gateway_port} unreachable: {e}"))
        .json()
        .await
        .expect("listing is JSON");
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
    row["name"]
        .as_str()
        .expect("row carries a name")
        .to_string()
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
