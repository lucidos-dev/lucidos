//! The diagnostic log answers only this engine's agents (ADR 0667). A caller
//! without a verified origin token meets the refusal, never a line.
use crate::support::{base_url, http_client};

/// The refusal's `reason` slug. A wire contract.
const AGENT_ONLY: &str = "diagnostic_log_agent_only";

#[tokio::test]
async fn the_diagnostic_log_refuses_a_caller_without_an_origin_token() {
    let url = format!("{}/api/v1/engine/diagnostic-log", base_url());
    let unstamped = http_client().get(&url);
    let forged = http_client()
        .get(&url)
        .header("x-lucidos-agent-origin-token", "-.0.-.deadbeef");
    for request in [unstamped, forged] {
        let response = request.send().await.expect("diagnostic log request");
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
        let body: serde_json::Value = response.json().await.expect("a JSON refusal");
        assert_eq!(body["reason"], AGENT_ONLY, "{body}");
        assert!(body.get("lines").is_none(), "{body}");
    }
}
