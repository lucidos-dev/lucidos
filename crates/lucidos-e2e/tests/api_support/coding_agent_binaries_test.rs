//! E2E coverage for `GET /api/v1/coding-agents/binaries` (Settings, Coding
//! Agents, Binaries).
//!
//! The endpoint EXECUTES each resolved binary with `--version`, so the point of
//! this test is that a real host cannot break it: whatever `claude` / `codex`
//! are (installed, missing, wedged, printing something we don't recognize), the
//! response must still be a 200 carrying both agents' resolution.
//!
//! Everything asserted here is host-independent on purpose. Whether a version
//! comes back depends on what is installed on the machine running the suite, so
//! the test pins the CONTRACT (`version` is absent or a clean single-line
//! token, never null, never empty) rather than a value.

use crate::support::{base_url, http_client};
use serde_json::Value;

/// The engine omits `version` when it doesn't know one, so a present field is
/// always a real, renderable token. The frontend prefixes it with `v` and shows
/// it verbatim, so a newline or empty string would land in the settings row.
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
        // A resolution that found nothing has no path; anything else does.
        assert_eq!(
            status.get("path").is_some_and(|p| p.is_string()),
            source != "not-found",
            "{agent}: path presence must follow the source ({source})"
        );
        assert_version_contract(agent, status);
        // Nothing may be executed for a resolution that isn't valid, so such a
        // status can never carry a version.
        if status.get("valid").and_then(Value::as_bool) == Some(false) {
            assert!(
                status.get("version").is_none(),
                "{agent}: an unusable binary must not be probed for a version"
            );
        }
    }
}
