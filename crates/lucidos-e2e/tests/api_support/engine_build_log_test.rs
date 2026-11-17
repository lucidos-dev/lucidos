//! Build diagnostics stay reachable through the workspace HTTP router.
use crate::support::{base_url, http_client, user_client};

#[tokio::test]
async fn engine_rebuild_rejects_unknown_mode_without_starting_a_build() {
    let response = user_client()
        .await
        .post(format!("{}/api/v1/engine/rebuild?mode=invalid", base_url()))
        .send()
        .await
        .expect("rebuild request");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn engine_build_log_returns_bounded_text_or_explicit_absence() {
    let response = http_client()
        .get(format!("{}/api/v1/engine/build-log", base_url()))
        .send()
        .await
        .expect("build log request");
    let status = response.status();
    assert!(
        status.is_success() || status == reqwest::StatusCode::NOT_FOUND,
        "unexpected status: {status}"
    );
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/plain"));
    if status.is_success() {
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let text = response.text().await.unwrap();
    // Wire contract: 64 KiB of source bytes; lossy UTF-8 can triple them.
    assert!(text.len() <= 3 * 64 * 1024);
    if status == reqwest::StatusCode::NOT_FOUND {
        assert_eq!(text, "Build output is not available yet.");
    }
}
