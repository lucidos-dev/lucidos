use crate::support::{base_url, http_client};
use serde_json::Value;

fn assert_version_contract(agent: &str, status: &Value) {
    let Some(version) = status.get("version") else {
        return;
    };
    let version = version
        .as_str()
        .unwrap_or_else(|| panic!("{agent}: version must be a string when present, got {version}"));
    assert!(
        !version.trim().is_empty(),
        "{agent}: an unknown version must be OMITTED, never an empty string"
    );
    assert!(
        !version.contains('\n'),
        "{agent}: version must be a single token, got {version:?}"
    );
    assert!(
        version.starts_with(|c: char| c.is_ascii_digit()),
        "{agent}: version must be the bare token with no leading 'v' or product name, got {version:?}"
    );
}

#[tokio::test]
async fn coding_agent_binaries_reports_both_agents_with_a_sane_version() {
    let resp = http_client()
        .get(format!("{}/api/v1/coding-agents/binaries", base_url()))
        .send()
        .await
        .expect("get coding-agents/binaries failed");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.expect("invalid JSON");

    for agent in ["claude_code", "codex"] {
        let status = body
            .get(agent)
            .unwrap_or_else(|| panic!("{agent} missing from {body}"));
        let source = status
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{agent}: source missing from {status}"));
        assert!(
            matches!(source, "override" | "detected" | "path" | "not-found"),
            "{agent}: unexpected source {source}"
        );
        assert!(
            status.get("valid").and_then(Value::as_bool).is_some(),
            "{agent}: valid missing from {status}"
        );
        assert_eq!(
            status.get("path").is_some_and(|p| p.is_string()),
            source != "not-found",
            "{agent}: path presence must follow the source ({source})"
        );
        assert_version_contract(agent, status);
        if status.get("valid").and_then(Value::as_bool) == Some(false) {
            assert!(
                status.get("version").is_none(),
                "{agent}: an unusable binary must not be probed for a version"
            );
        }
    }
}
