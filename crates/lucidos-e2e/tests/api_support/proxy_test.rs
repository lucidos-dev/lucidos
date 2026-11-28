use crate::support::{apis_json_lock, base_url, http_client, user_client, workspace_tree_lock};
use lucidos_engine::core::prefs;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn gateway_builtin_without_sign_in_returns_an_actionable_error() {
    let _apis = apis_json_lock().lock().await;
    let response = user_client()
        .await
        .post(format!(
            "{}/api/v1/proxy/claude-gateway/messages",
            base_url()
        ))
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "claude-haiku-5-5", "max_tokens": 8,
            "messages": [{"role":"user", "content":"Reply with OK."}]
        }))
        .send()
        .await
        .expect("gateway proxy route responds");
    assert_eq!(response.status().as_u16(), 404);
    let body = response.text().await.unwrap();
    assert!(body.contains("builtin model provider"), "{body}");
    assert!(body.contains("Settings"), "{body}");
}

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
    let raised = put_preference(prefs::PROXY_TIMEOUT_SECS.key(), "45").await;

    let output = tokio::task::spawn_blocking(move || {
        super::lucidos_cli_test::lucidos_cmd()
            .args(["proxy", name, "/v1/slow", "--fail"])
            .output()
            .expect("lucidos proxy should run")
    })
    .await
    .expect("the CLI task");

    delete_preference(prefs::PROXY_TIMEOUT_SECS.key()).await;
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

#[tokio::test]
async fn a_proxy_timeout_outside_the_range_is_refused_over_http() {
    for bad in ["601", "0", "soon"] {
        let result = put_preference(prefs::PROXY_TIMEOUT_SECS.key(), bad).await;
        assert_eq!(
            result["success"],
            json!(false),
            "'{bad}' was stored: {result}"
        );
        let error = result["error"].as_str().unwrap_or_default();
        assert!(
            error.contains(prefs::PROXY_TIMEOUT_SECS.key()),
            "the refusal must name the key: {error}"
        );
    }
    let result = put_preference(prefs::PROXY_TIMEOUT_SECS.key(), "601").await;
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
    let value = stored["preferences"][prefs::PROXY_TIMEOUT_SECS.key()].as_str();
    assert!(
        !matches!(value, Some("601" | "0" | "soon")),
        "a refused value was stored: {stored}"
    );
}

async fn spawn_json_upstream(body: serde_json::Value) -> String {
    let app = axum::Router::new().fallback(move || {
        let body = body.clone();
        async move { axum::Json(body) }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn a_model_call_through_the_proxy_records_its_cost() {
    let model = format!("e2e-proxied-{}", uuid::Uuid::new_v4());
    let served_model = format!("{model}-served");
    let reply = json!({
        "model": served_model,
        "choices": [{"message": {"role": "assistant", "content": "hi"}}],
        "usage": {"prompt_tokens": 321, "completion_tokens": 12},
    });
    let upstream = spawn_json_upstream(reply.clone()).await;
    let pointed = put_preference(prefs::LOCAL_BASE_URL.key(), &upstream).await;

    let resp = http_client()
        .post(format!(
            "{}/api/v1/proxy/local/chat/completions",
            base_url()
        ))
        .json(&json!({"model": model, "messages": []}))
        .send()
        .await
        .expect("proxy request failed");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();

    let mut row = None;
    for _ in 0..50 {
        let rows: Vec<serde_json::Value> = user_client()
            .await
            .get(format!(
                "{}/api/v1/events/query?event_type=ContextCaptured&limit=200",
                base_url()
            ))
            .send()
            .await
            .expect("events query failed")
            .json()
            .await
            .expect("an event list");
        row = rows.into_iter().find(|r| r["payload"]["model"] == model);
        if row.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    delete_preference(prefs::LOCAL_BASE_URL.key()).await;

    assert_eq!(
        pointed["success"],
        json!(true),
        "the base URL was refused: {pointed}"
    );
    assert_eq!(status, 200);
    assert_eq!(body, reply, "the caller gets the reply unchanged");
    let row = row.expect("the proxied call recorded a row");
    assert_eq!(row["payload"]["purpose"], "proxy");
    assert_eq!(row["payload"]["served_model"], json!(served_model));
    assert_eq!(row["payload"]["usage"]["input_tokens"], 321);
    assert_eq!(row["payload"]["usage"]["output_tokens"], 12);
}
