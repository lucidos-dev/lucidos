//! E2E coverage for a Slack hook over real HTTP: the v0 signature, and the
//! `url_verification` handshake Slack runs when you save a Request URL.
//!
//! The signature here is computed independently with the `hmac` crate, never
//! with the engine's own signer, so a bug in that signer cannot pass itself.

use crate::support::{base_url, db_url, unique_marker, user_client};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;

const SECRET: &str = "8f742231b10e8888abcd99yyyzzz85a5";

/// Create a hook signed the way Slack signs, with a provided signing secret.
async fn create_hook(client: &reqwest::Client, api: &str, marker: &str, hmac: Value) -> String {
    let resp = client
        .post(format!("{api}/api/v1/webhooks"))
        .json(&json!({
            "name": format!("slack-{marker}"),
            "event_type": format!("SlackEvent{marker}"),
            "hmac": hmac,
            "signing_secret": { "mode": "provided", "value": SECRET },
        }))
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200, "create webhook");
    let created: Value = resp.json().await.unwrap();
    created["id"].as_str().expect("an id").to_string()
}

fn slack_hmac(marker: &str) -> Value {
    json!({
        "credential": format!("slack-signing-{marker}"),
        "signature_header": "X-Slack-Signature",
        "prefix": "v0=",
        "timestamp_header": "X-Slack-Request-Timestamp",
        "template": "v0:{timestamp}:{body}",
    })
}

/// Hex HMAC-SHA256 of `message` under the shared secret.
fn hex_hmac(message: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(message.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Slack's v0 signature: `v0=` plus the HMAC over `v0:<ts>:<body>`.
fn slack_signature(timestamp: i64, body: &str) -> String {
    format!("v0={}", hex_hmac(&format!("v0:{timestamp}:{body}")))
}

async fn post(
    client: &reqwest::Client,
    api: &str,
    id: &str,
    body: &str,
    headers: &[(&str, String)],
) -> (reqwest::StatusCode, Value) {
    let mut req = client
        .post(format!("{api}/api/v1/webhooks/{id}/deliver"))
        .header("content-type", "application/json")
        .body(body.to_string());
    for (name, value) in headers {
        req = req.header(*name, value);
    }
    let resp = req.send().await.expect("deliver failed");
    let status = resp.status();
    (status, resp.json().await.unwrap_or(Value::Null))
}

fn signed(body: &str) -> Vec<(&'static str, String)> {
    let now = unix_now();
    vec![
        ("X-Slack-Request-Timestamp", now.to_string()),
        ("X-Slack-Signature", slack_signature(now, body)),
    ]
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

async fn events_of_type(event_type: &str) -> i64 {
    let pool = PgPool::connect(&db_url()).await.expect("connect");
    sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
        .bind(event_type)
        .fetch_one(&pool)
        .await
        .expect("count events")
}

fn url_check(challenge: &str) -> String {
    json!({ "token": "t", "challenge": challenge, "type": "url_verification" }).to_string()
}

/// A delivery signed exactly as Slack signs it authenticates and emits.
#[tokio::test]
async fn a_slack_v0_signed_delivery_emits_its_event() {
    let client = user_client().await;
    let api = base_url();
    let marker = unique_marker("slack");
    let id = create_hook(&client, &api, &marker, slack_hmac(&marker)).await;

    let body = json!({ "type": "event_callback", "event": { "type": "app_mention" } }).to_string();
    let (status, answer) = post(&client, &api, &id, &body, &signed(&body)).await;
    assert_eq!(status, 202, "{answer}");
    assert_eq!(events_of_type(&format!("SlackEvent{marker}")).await, 1);

    let (status, _) = post(
        &client,
        &api,
        &id,
        &body,
        &[("X-Slack-Request-Timestamp", unix_now().to_string())],
    )
    .await;
    assert_eq!(status, 401, "an unsigned delivery is refused");
}

/// Slack saving the Request URL gets its challenge back, and no event.
#[tokio::test]
async fn a_signed_url_check_echoes_the_challenge_and_emits_nothing() {
    let client = user_client().await;
    let api = base_url();
    let marker = unique_marker("slack");
    let id = create_hook(&client, &api, &marker, slack_hmac(&marker)).await;

    let body = url_check("3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P");
    let (status, answer) = post(&client, &api, &id, &body, &signed(&body)).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        answer,
        json!({ "challenge": "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P" })
    );
    assert_eq!(events_of_type(&format!("SlackEvent{marker}")).await, 0);
}

/// A forged challenge never echoes: verification runs first.
#[tokio::test]
async fn an_unsigned_url_check_is_refused() {
    let client = user_client().await;
    let api = base_url();
    let marker = unique_marker("slack");
    let id = create_hook(&client, &api, &marker, slack_hmac(&marker)).await;

    let body = url_check("forged");
    let mut headers = signed(&body);
    headers[1].1 = slack_signature(unix_now(), "a different body");
    let (status, answer) = post(&client, &api, &id, &body, &headers).await;
    assert_eq!(status, 401, "{answer}");
    assert_ne!(answer["challenge"], "forged");

    let (status, _) = post(&client, &api, &id, &body, &[]).await;
    assert_eq!(status, 401);
    assert_eq!(events_of_type(&format!("SlackEvent{marker}")).await, 0);
}

/// Only a Slack hook answers the handshake. Any other signed hook treats the
/// same body as an ordinary delivery.
#[tokio::test]
async fn a_non_slack_hook_emits_a_url_check_like_any_body() {
    let client = user_client().await;
    let api = base_url();
    let marker = unique_marker("slack");
    let id = create_hook(
        &client,
        &api,
        &marker,
        json!({
            "credential": format!("other-signing-{marker}"),
            "signature_header": "X-Hub-Signature-256",
            "prefix": "sha256=",
            "template": "{body}",
        }),
    )
    .await;

    let body = url_check("not-for-me");
    let (status, answer) = post(
        &client,
        &api,
        &id,
        &body,
        &[("X-Hub-Signature-256", format!("sha256={}", hex_hmac(&body)))],
    )
    .await;
    assert_eq!(status, 202, "{answer}");
    assert!(answer["event_id"].is_string(), "{answer}");
    assert_eq!(events_of_type(&format!("SlackEvent{marker}")).await, 1);
}
