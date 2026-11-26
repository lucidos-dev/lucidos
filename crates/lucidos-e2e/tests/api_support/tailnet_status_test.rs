use crate::support::{base_url, http_client};
use serde_json::Value;
use std::time::{Duration, Instant};

const ANSWER_WITHIN: Duration = Duration::from_secs(20);

fn string_or_null(body: &Value, key: &str) -> bool {
    matches!(body.get(key), Some(Value::Null) | Some(Value::String(_)))
}

#[tokio::test]
async fn tailnet_status_answers_with_both_fields_and_a_bound() {
    let client = http_client();
    let api = base_url();

    let started = Instant::now();
    let resp = client
        .get(format!("{}/api/v1/tailnet-status", api))
        .send()
        .await
        .expect("get failed");
    let elapsed = started.elapsed();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(
        string_or_null(&body, "magic_dns_name"),
        "magic_dns_name must be a string or null: {body}"
    );
    assert!(
        string_or_null(&body, "workspace_serve_url"),
        "workspace_serve_url must be a string or null: {body}"
    );
    assert!(
        elapsed < ANSWER_WITHIN,
        "took {elapsed:?}, so a probe on this path is unbounded"
    );

    if let Some(url) = body.get("workspace_serve_url").and_then(Value::as_str) {
        assert!(url.starts_with("https://"), "not HTTPS: {url}");
        assert!(url.ends_with('/'), "not a workspace path: {url}");
        let authority_and_path = url.trim_start_matches("https://");
        assert!(
            authority_and_path.contains('/') && !authority_and_path.ends_with("//"),
            "reaches the gateway root rather than a workspace: {url}"
        );
    }
}
