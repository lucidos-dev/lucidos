use crate::support::{base_url, http_client};

const PAYLOAD: &str = r#"{"category":"test","message":"e2e-routing","data":{"x":1}}"#;

/// Regression: theme-flash telemetry POSTs were silently 404ing because the
/// inline FOUC IIFE fetched `/api/v1/internal/client-log` while the route is
/// registered under `/api/internal/...` (the `api_routes` block, not
/// `api_v1_routes`). Pin both URLs so a future `/v1` move can't drift.
#[tokio::test]
async fn client_log_resolves_at_api_internal_not_api_v1() {
    let client = http_client();

    let ok = client
        .post(format!("{}/api/internal/client-log", base_url()))
        .header("content-type", "application/json")
        .body(PAYLOAD)
        .send()
        .await
        .expect("request to /api/internal/client-log failed");
    assert_eq!(
        ok.status(),
        204,
        "POST /api/internal/client-log must return 204"
    );

    let not_found = client
        .post(format!("{}/api/v1/internal/client-log", base_url()))
        .header("content-type", "application/json")
        .body(PAYLOAD)
        .send()
        .await
        .expect("request to /api/v1/internal/client-log failed");
    assert_eq!(
        not_found.status(),
        404,
        "POST /api/v1/internal/client-log must NOT resolve — the original telemetry typo"
    );
}
