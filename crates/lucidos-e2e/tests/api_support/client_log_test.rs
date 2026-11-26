use crate::support::{base_url, http_client};

const PAYLOAD: &str =
    r#"{"category":"error","code":"uncaught","message":"e2e-routing","data":{"x":1}}"#;

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

#[tokio::test]
async fn client_log_batch_refuses_oversized_codes() {
    let response = http_client()
        .post(format!("{}/api/v1/internal/client-logs", base_url()))
        .json(&serde_json::json!([
            {"category": "error", "code": "uncaught", "message": "private message"},
            {"category": "error", "code": "x".repeat(257), "message": "private message"}
        ]))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
}
