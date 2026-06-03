use crate::support::{base_url, http_client};

const PAYLOAD: &str = r#"{"category":"test","message":"e2e-routing","data":{"x":1}}"#;

/// Regression: every HTTP route now lives under `/api/v1/` (see the
/// "API URL Conventions" section of `.claude/rules/rust.md`). The
/// theme-flash telemetry IIFE in `index.html` and `liveness.ts` must POST
/// to `/api/v1/internal/client-log`, and the legacy unversioned
/// `/api/internal/client-log` must NOT resolve so a regression that drops
/// the `/v1/` is caught loudly instead of silently 404-ing in production.
#[tokio::test]
async fn client_log_resolves_at_api_v1_internal_not_api_internal() {
    let client = http_client();

    let ok = client
        .post(format!("{}/api/v1/internal/client-log", base_url()))
        .header("content-type", "application/json")
        .body(PAYLOAD)
        .send()
        .await
        .expect("request to /api/v1/internal/client-log failed");
    assert_eq!(
        ok.status(),
        204,
        "POST /api/v1/internal/client-log must return 204"
    );

    let not_found = client
        .post(format!("{}/api/internal/client-log", base_url()))
        .header("content-type", "application/json")
        .body(PAYLOAD)
        .send()
        .await
        .expect("request to /api/internal/client-log failed");
    assert_eq!(
        not_found.status(),
        404,
        "POST /api/internal/client-log must NOT resolve — every route now lives under /api/v1/"
    );
}
