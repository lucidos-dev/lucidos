//! E2E coverage for the generic API proxy at `/api/v1/proxy/...`.
//!
//! Path-traversal and method-validation guards are exercised by the
//! in-module unit tests in `crates/lucidos-engine/src/api/proxy.rs::tests`.
//! Note that we can't reach those guards via reqwest in an e2e test —
//! `url::Url` (and most HTTP clients) normalize `..` segments before the
//! request leaves the process, so the bytes that hit the engine no longer
//! contain `..`. The guards are defense-in-depth against non-normalizing
//! clients and are unit-tested directly.
//!
//! Full proxy round-trip (auth injection, header stripping, upstream
//! forwarding) is also covered by the in-module unit tests, which spin up
//! a tiny axum upstream and don't need the full workspace.
//!
//! What we test here is route wiring (that the engine actually mounts
//! `/api/v1/proxy/...` and runs the handler), and the proxy timeout end to end:
//! a raised setting lets the real `lucidos proxy` binary outlast 30 seconds.

use crate::support::{apis_json_lock, base_url, http_client, user_client, workspace_tree_lock};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn proxy_returns_404_when_name_not_configured() {
    let client = http_client();
    let url = format!(
        "{}/api/v1/proxy/this-name-is-not-in-apis-json/some/path",
        base_url()
    );
    let resp = client.get(&url).send().await.expect("request failed");
    assert_eq!(resp.status().as_u16(), 404);
    let body = resp.text().await.unwrap_or_default();
    assert!(
        body.contains("not configured"),
        "expected 'not configured' in body, got: {}",
        body
    );
}

#[tokio::test]
async fn proxy_returns_404_for_root_path_when_name_not_configured() {
    let client = http_client();
    let url = format!("{}/api/v1/proxy/missing-proxy", base_url());
    let resp = client.get(&url).send().await.expect("request failed");
    assert_eq!(resp.status().as_u16(), 404);
}

/// An upstream on loopback that answers `slow-ok` after `delay`, however it is
/// asked. The engine reaches it over plain `http://`, which loopback allows.
async fn spawn_slow_upstream(delay: Duration) -> String {
    let app = axum::Router::new().fallback(move || async move {
        tokio::time::sleep(delay).await;
        "slow-ok"
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// Write one preference, and return the engine's verdict.
///
/// `PUT /preferences` answers a refusal with `200 {success: false, error}`, so
/// the verdict is in the body.
async fn put_preference(key: &str, value: &str) -> serde_json::Value {
    let resp = user_client()
        .await
        .put(format!("{}/api/v1/preferences?key={key}", base_url()))
        .json(&json!({ "value": value }))
        .send()
        .await
        .expect("preference write failed");
    assert_eq!(resp.status(), 200);
    resp.json().await.expect("an ApiResult body")
}

async fn delete_preference(key: &str) {
    let _ = user_client()
        .await
        .delete(format!("{}/api/v1/preferences?key={key}", base_url()))
        .send()
        .await;
}

/// The headline case. An upstream answers after 31 s, past the old fixed limit.
/// With `proxy_timeout_secs` at 45 the real `lucidos proxy` binary gets the
/// body. That proves both halves: the engine honours the setting, and the
/// CLI's own client no longer gives up at 30 s.
#[tokio::test]
async fn a_raised_proxy_timeout_lets_lucidos_proxy_outlast_thirty_seconds() {
    let _tree = workspace_tree_lock().read().await;
    let _apis = apis_json_lock().lock().await;
    let upstream = spawn_slow_upstream(Duration::from_secs(31)).await;
    let name = "e2e-slow-upstream";

    let config = json!({ name: { "base_url": upstream } }).to_string();
    let written = user_client()
        .await
        .put(format!("{}/api/v1/data/config/apis.json", base_url()))
        .header("Content-Type", "text/plain")
        .body(config)
        .send()
        .await
        .expect("apis.json write failed")
        .status()
        .as_u16();
    let raised = put_preference("proxy_timeout_secs", "45").await;

    // Blocking, so it runs off this test's runtime, which also drives the
    // upstream. The CLI's stdout is the upstream's body on success.
    let output = tokio::task::spawn_blocking(move || {
        super::lucidos_cli_test::lucidos_cmd()
            .args(["proxy", name, "/v1/slow", "--fail"])
            .output()
            .expect("lucidos proxy should run")
    })
    .await
    .expect("the CLI task");

    // Restore before asserting, so a failure leaves nothing behind.
    delete_preference("proxy_timeout_secs").await;
    let _ = user_client()
        .await
        .delete(format!("{}/api/v1/data/config/apis.json", base_url()))
        .send()
        .await;

    assert_eq!(written, 200, "writing apis.json must work");
    assert_eq!(
        raised["success"],
        json!(true),
        "the raise was refused: {raised}"
    );
    assert!(
        output.status.success(),
        "lucidos proxy failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "slow-ok");
}

/// The maximum is 600 s. The HTTP write refuses a larger value, a zero and a
/// word. Each refusal names the key and the range, and nothing is stored.
#[tokio::test]
async fn a_proxy_timeout_outside_the_range_is_refused_over_http() {
    for bad in ["601", "0", "soon"] {
        let result = put_preference("proxy_timeout_secs", bad).await;
        assert_eq!(
            result["success"],
            json!(false),
            "'{bad}' was stored: {result}"
        );
        let error = result["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("proxy_timeout_secs"),
            "the refusal must name the key: {error}"
        );
    }
    let result = put_preference("proxy_timeout_secs", "601").await;
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("600"),
        "the refusal must name the maximum: {error}"
    );

    let stored: serde_json::Value = http_client()
        .get(format!("{}/api/v1/preferences", base_url()))
        .send()
        .await
        .expect("preferences read")
        .json()
        .await
        .expect("preferences JSON");
    // The raise test may hold its own value at this moment, so check for the
    // refused values rather than for absence.
    let value = stored["preferences"]["proxy_timeout_secs"].as_str();
    assert!(
        !matches!(value, Some("601" | "0" | "soon")),
        "a refused value was stored: {stored}"
    );
}
