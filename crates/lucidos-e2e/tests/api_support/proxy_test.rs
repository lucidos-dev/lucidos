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
//! What we test here is just route wiring — that the engine actually
//! mounts `/api/v1/proxy/...` and runs the handler.

use crate::support::{base_url, http_client};

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

#[tokio::test]
async fn proxy_credentials_returns_404_when_name_not_configured() {
    let client = http_client();
    let url = format!(
        "{}/api/v1/proxy-credentials/this-name-is-not-in-apis-json",
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
