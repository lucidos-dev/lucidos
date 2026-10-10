//! E2E coverage for `GET /api/v1/tailnet-status`, which Settings → Access reads
//! to print the address a user copies to another device.
//!
//! Shape only, deliberately. Both fields are properties of the HOST's tailnet.
//! Their values depend on whether the machine running the suite is on one, and
//! on whether `tailscale serve` is configured. What the endpoint owes every
//! caller regardless is that it answers, answers quickly, and answers with both
//! keys present as either a string or null.
//!
//! The bound matters as much as the shape. This runs on a settings pane, behind
//! a reverse lookup and a network round trip. An unbounded probe there leaves
//! the section loading with nothing to show and no reason why.

use crate::support::{base_url, http_client};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Generous next to the engine's own deadlines, which cap the reverse lookup at
/// 1.5s and the verification round trip at 3s. This fails on an UNBOUNDED
/// probe, not on a slow one, so it must not be tight enough to flake on a busy
/// CI host.
const ANSWER_WITHIN: Duration = Duration::from_secs(20);

/// A field that is present, and is either a string or null.
///
/// `Value::Null` is a real answer here: off a tailnet, or nothing verified. So
/// a missing key and a null one differ, and only the first is a broken
/// contract.
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

    // A published URL is always the full workspace address, never a bare
    // origin. The engine returns exactly the string it verified, and the page
    // prints it verbatim. So a root URL here is one handed to the user.
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
