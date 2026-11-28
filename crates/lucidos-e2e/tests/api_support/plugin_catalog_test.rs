use crate::support::{base_url, unique_marker, user_client};
use serde_json::json;
use std::time::{Duration, Instant};

fn unreachable_source() -> String {
    format!("https://example.invalid/{}.git", unique_marker("e2e-mkt"))
}

const NO_CLONE_BUDGET: Duration = Duration::from_secs(3);

async fn get_catalog() -> (Duration, serde_json::Value) {
    let client = user_client().await;
    let started = Instant::now();
    let resp = client
        .get(format!("{}/api/v1/plugins/catalog", base_url()))
        .send()
        .await
        .expect("catalog request failed");
    assert_eq!(resp.status(), 200, "catalog must answer 200");
    let body = resp.json().await.expect("catalog body is not json");
    (started.elapsed(), body)
}

#[tokio::test]
async fn the_catalog_answers_without_cloning_anything() {
    let client = user_client().await;
    let api = base_url();
    let source = unreachable_source();

    let resp = client
        .post(format!("{api}/api/v1/plugins/marketplaces"))
        .json(&json!({ "source": source, "name": "E2E unreachable" }))
        .send()
        .await
        .expect("register failed");
    assert_eq!(resp.status(), 200, "registering a marketplace must succeed");
    let registered: serde_json::Value = resp.json().await.expect("register body is not json");
    let id = registered["marketplace"]["id"]
        .as_str()
        .expect("register response names the new id")
        .to_string();

    for attempt in 1..=2 {
        let (elapsed, body) = get_catalog().await;
        assert!(
            elapsed < NO_CLONE_BUDGET,
            "attempt {attempt}: catalog took {elapsed:?}, which means it cloned",
        );
        let sources: Vec<&str> = body["marketplaces"]
            .as_array()
            .expect("marketplaces array")
            .iter()
            .filter_map(|m| m["source"].as_str())
            .collect();
        assert!(
            sources.contains(&source.as_str()),
            "attempt {attempt}: the live registry must be in the response: {body}",
        );
    }

    let _ = client
        .delete(format!("{api}/api/v1/plugins/marketplaces/{id}"))
        .send()
        .await;
}

#[tokio::test]
async fn the_catalog_reports_its_own_freshness() {
    let (_, body) = get_catalog().await;
    let obj = body.as_object().expect("catalog body is an object");

    assert!(
        obj.get("scanning").and_then(|v| v.as_bool()).is_some(),
        "scanning must be a bool: {body}",
    );
    assert!(obj.contains_key("scanned_at"), "missing scanned_at: {body}");
    assert!(obj.contains_key("scan_error"), "missing scan_error: {body}");
}

#[tokio::test]
async fn a_manual_rescan_returns_immediately() {
    let client = user_client().await;
    let started = Instant::now();

    let resp = client
        .post(format!("{}/api/v1/plugins/catalog/rescan", base_url()))
        .json(&json!({}))
        .send()
        .await
        .expect("rescan failed");

    assert_eq!(resp.status(), 200);
    assert!(
        started.elapsed() < NO_CLONE_BUDGET,
        "rescan waited for the scan it queued",
    );
}

#[tokio::test]
async fn the_media_route_serves_nothing_unlisted_and_refuses_apps() {
    let client = user_client().await;
    let api = base_url();
    for path in [
        "/api/v1/plugins/media/installed/no-such-plugin/media/icon.svg",
        "/api/v1/plugins/media/catalog/no-mkt/no-such-plugin/1.0.0/media/icon.svg",
    ] {
        let resp = client
            .get(format!("{api}{path}"))
            .send()
            .await
            .expect("media request failed");
        assert_eq!(resp.status(), 404, "{path}");
        let headers = resp.headers();
        assert_eq!(headers["x-content-type-options"], "nosniff", "{path}");
        assert_eq!(
            headers["content-security-policy"], "sandbox; default-src 'none'",
            "{path}"
        );

        let from_app = client
            .get(format!("{api}{path}"))
            .header("x-lucidos-app-id", "habit-tracker")
            .send()
            .await
            .expect("media request failed");
        assert_eq!(from_app.status(), 403, "an app frame reached {path}");
    }
}
