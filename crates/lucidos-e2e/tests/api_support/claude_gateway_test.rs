use crate::support::{base_url, user_client};
use lucidos_engine::core::prefs;
use serde_json::json;

async fn put_url(client: &reqwest::Client, api: &str, url: &str) -> serde_json::Value {
    let resp = client
        .put(format!(
            "{}/api/v1/preferences?key={}",
            api,
            prefs::CLAUDE_GATEWAY_URL.key()
        ))
        .json(&json!({ "value": url }))
        .send()
        .await
        .expect("preference write failed");
    assert_eq!(resp.status(), 200);
    resp.json().await.expect("an ApiResult body")
}

async fn status(client: &reqwest::Client, api: &str) -> serde_json::Value {
    let resp = client
        .get(format!("{}/api/v1/providers/claude-gateway", api))
        .send()
        .await
        .expect("status request failed");
    assert_eq!(resp.status(), 200);
    resp.json().await.expect("a status body")
}

async fn post(
    client: &reqwest::Client,
    api: &str,
    action: &str,
    body: serde_json::Value,
) -> reqwest::Response {
    client
        .post(format!(
            "{}/api/v1/providers/claude-gateway/{}",
            api, action
        ))
        .json(&body)
        .send()
        .await
        .expect("request failed")
}

#[tokio::test]
async fn the_claude_gateway_routes_answer_without_a_gateway() {
    let client = user_client().await;
    let api = base_url();
    assert_eq!(put_url(&client, &api, "").await["success"], json!(true));

    let refused = put_url(&client, &api, "http://gateway.example").await;
    assert_eq!(refused["success"], json!(false), "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("https"),
        "{refused}"
    );

    let idle = status(&client, &api).await;
    assert_eq!(idle["url"], json!(null), "{idle}");
    assert_eq!(idle["account"], json!(null), "{idle}");
    assert_eq!(idle["sign_in_again"], json!(false), "{idle}");
    assert_eq!(
        idle["telemetry"],
        json!(false),
        "no sign-in, no telemetry: {idle}"
    );
    assert!(
        idle["suggested_url"].is_null() || idle["suggested_url"].is_string(),
        "{idle}"
    );
    assert_eq!(idle["sign_in"], json!({ "state": "idle" }), "{idle}");

    let resp = post(&client, &api, "sign-in", json!({})).await;
    assert_eq!(resp.status(), 400);
    let body = resp.text().await.unwrap();
    assert!(body.contains(prefs::CLAUDE_GATEWAY_URL.key()), "{body}");

    let resp = post(
        &client,
        &api,
        "sign-in",
        json!({ "url": "http://gateway.example" }),
    )
    .await;
    assert_eq!(resp.status(), 400);
    assert_eq!(status(&client, &api).await["url"], json!(null));

    let unreachable = "http://127.0.0.1:9";
    let resp = post(&client, &api, "sign-in", json!({ "url": unreachable })).await;
    assert_eq!(resp.status(), 502);
    let after = status(&client, &api).await;
    assert_eq!(after["url"], json!(unreachable), "{after}");
    assert_eq!(after["sign_in"], json!({ "state": "idle" }), "{after}");

    let out = post(&client, &api, "sign-out", json!({})).await;
    assert_eq!(out.status(), 200);
    let out: serde_json::Value = out.json().await.unwrap();
    assert_eq!(out, json!({ "signed_out": false }));

    assert_eq!(put_url(&client, &api, "").await["success"], json!(true));
}
