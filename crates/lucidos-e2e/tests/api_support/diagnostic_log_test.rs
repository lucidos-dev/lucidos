//! The diagnostic log answers only this engine's agents (ADR 0667). A caller
//! without a verified origin token meets the refusal, never a line.
use crate::support::{base_url, http_client};

/// The refusal's `reason` slug. A wire contract.
const AGENT_ONLY: &str = "diagnostic_log_agent_only";

#[tokio::test]
async fn the_diagnostic_log_refuses_a_caller_without_an_origin_token() {
    let url = format!("{}/api/v1/engine/diagnostic-log", base_url());
    for source in [None, Some("all"), Some("gateway"), Some("client")] {
        let mut unstamped = http_client().get(&url);
        if let Some(source) = source {
            unstamped = unstamped.query(&[("source", source)]);
        }
        let forged = unstamped
            .try_clone()
            .unwrap()
            .header("x-lucidos-agent-origin-token", "-.0.-.deadbeef");
        for request in [unstamped, forged] {
            let response = request.send().await.expect("diagnostic log request");
            assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
            let body: serde_json::Value = response.json().await.expect("a JSON refusal");
            assert_eq!(body["reason"], AGENT_ONLY, "{body}");
            assert!(body.get("lines").is_none(), "{body}");
        }
    }
}

#[tokio::test]
async fn diagnostic_snapshot_is_local_and_contains_only_bounded_evidence() {
    let url = format!("{}/api/v1/engine/diagnostic-snapshot", base_url());
    let token = lucidos_local_token::read().expect("the engine mints a local token at boot");
    let response = http_client()
        .get(&url)
        .header(lucidos_local_token::HEADER_LOCAL_TOKEN, token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let snapshot: serde_json::Value = response.json().await.unwrap();
    // Pin the bounded wire contract across the process boundary.
    assert!(snapshot["events"].as_array().unwrap().len() <= 2000);
    assert!(snapshot["faults"].as_array().unwrap().len() <= 32);
    assert!(snapshot["versions"]["engine"].is_string());
    assert!(snapshot["versions"]["engine-build"].is_string());
    assert!(snapshot["configured"]["llm"].is_boolean());
    for value in snapshot["configured"].as_object().unwrap().values() {
        assert!(value.is_boolean());
    }
    assert!(snapshot.get("workspace_path").is_none());
    assert!(snapshot.get("database_url").is_none());
    let unauthenticated = http_client().get(&url).send().await.unwrap();
    assert!(matches!(unauthenticated.status().as_u16(), 401 | 403));
}
