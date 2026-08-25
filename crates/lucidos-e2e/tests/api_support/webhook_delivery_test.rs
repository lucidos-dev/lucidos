//! E2E coverage for the webhook delivery path: what a resend does, and what a
//! delivery becomes.
//!
//! The unit tests cover the ledger and the payload as separate pieces. This is
//! the wiring between them, over real HTTP. A claim taken in the wrong order,
//! or a header dropped on the floor, shows up here and nowhere else.

use crate::support::{base_url, db_url, http_client, unique_marker};
use serde_json::{json, Value};
use sqlx::PgPool;

/// Create a hook and return its id plus its one-time bearer token.
async fn create_hook(
    client: &reqwest::Client,
    api: &str,
    name: &str,
    event_type: &str,
    extra: Value,
) -> (String, String) {
    let mut body = json!({ "name": name, "event_type": event_type });
    for (key, value) in extra.as_object().expect("extra is an object") {
        body[key] = value.clone();
    }
    let resp = client
        .post(format!("{api}/api/v1/webhooks"))
        .json(&body)
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200, "create webhook");
    let created: Value = resp.json().await.unwrap();
    (
        created["id"].as_str().expect("an id").to_string(),
        created["token"]
            .as_str()
            .expect("an unsigned hook prints its token")
            .to_string(),
    )
}

/// POST one delivery, exactly as the hook socket forwards it.
async fn deliver(
    client: &reqwest::Client,
    api: &str,
    id: &str,
    token: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> (reqwest::StatusCode, Value) {
    let mut req = client
        .post(format!("{api}/api/v1/webhooks/{id}/deliver"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(body.to_string());
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let resp = req.send().await.expect("deliver failed");
    let status = resp.status();
    (status, resp.json().await.unwrap_or(Value::Null))
}

async fn events_of_type(event_type: &str) -> i64 {
    let pool = PgPool::connect(&db_url()).await.expect("connect");
    sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
        .bind(event_type)
        .fetch_one(&pool)
        .await
        .expect("count events")
}

/// The whole point: a sender resending one delivery fires the event once.
#[tokio::test]
async fn a_resend_inside_the_window_emits_nothing_and_reports_the_first_event() {
    let client = http_client();
    let api = base_url();
    let name = unique_marker("e2e-hook-dedupe");
    let event_type = "E2eDedupedDeliveryArrived";

    let (id, token) = create_hook(
        &client,
        &api,
        &name,
        event_type,
        json!({ "dedupe": { "header": "X-Delivery-Id", "window_secs": 3600 } }),
    )
    .await;

    let before = events_of_type(event_type).await;
    let body = r#"{"action":"opened","number":7}"#;
    let sent = [("x-delivery-id", "e2e-delivery-1")];

    let (status, first) = deliver(&client, &api, &id, &token, body, &sent).await;
    assert_eq!(status, 202, "the first delivery is accepted");
    let event_id = first["event_id"].as_str().expect("an event id").to_string();

    let (status, resend) = deliver(&client, &api, &id, &token, body, &sent).await;
    assert_eq!(
        status, 200,
        "a resend is 2xx, or the sender retries forever"
    );
    assert_eq!(resend["duplicate"], true);
    assert_eq!(
        resend["event_id"], event_id,
        "the resend is told what the first delivery emitted"
    );

    assert_eq!(
        events_of_type(event_type).await - before,
        1,
        "two deliveries, one event"
    );
}

/// A different delivery id is a different delivery, however alike the bodies.
#[tokio::test]
async fn a_new_delivery_id_still_emits_even_with_an_identical_body() {
    let client = http_client();
    let api = base_url();
    let name = unique_marker("e2e-hook-distinct");
    let event_type = "E2eDistinctDeliveryArrived";

    let (id, token) = create_hook(
        &client,
        &api,
        &name,
        event_type,
        json!({ "dedupe": { "header": "X-Delivery-Id" } }),
    )
    .await;

    let before = events_of_type(event_type).await;
    let body = r#"{"status":"ok"}"#;
    deliver(&client, &api, &id, &token, body, &[("x-delivery-id", "a")]).await;
    deliver(&client, &api, &id, &token, body, &[("x-delivery-id", "b")]).await;

    assert_eq!(events_of_type(event_type).await - before, 2);
}

/// The default. Nothing is deduped, so the log keeps every arrival, which is
/// what makes a sender's retry rate answerable.
#[tokio::test]
async fn a_hook_without_dedupe_emits_on_every_arrival() {
    let client = http_client();
    let api = base_url();
    let name = unique_marker("e2e-hook-plain");
    let event_type = "E2ePlainDeliveryArrived";

    let (id, token) = create_hook(&client, &api, &name, event_type, json!({})).await;

    let before = events_of_type(event_type).await;
    let body = r#"{"action":"opened"}"#;
    let sent = [("x-delivery-id", "same-id-both-times")];
    let (first, _) = deliver(&client, &api, &id, &token, body, &sent).await;
    let (second, payload) = deliver(&client, &api, &id, &token, body, &sent).await;

    assert_eq!(first, 202);
    assert_eq!(second, 202, "not a duplicate: this hook does not dedupe");
    assert!(payload["duplicate"].is_null());
    assert_eq!(events_of_type(event_type).await - before, 2);
}

/// What the trigger author actually addresses: the sender's fields under
/// `payload`, the allow-listed headers under `headers`, and no secret anywhere.
#[tokio::test]
async fn a_delivery_becomes_summary_headers_and_payload() {
    let client = http_client();
    let api = base_url();
    let name = unique_marker("e2e-hook-shape");
    let event_type = "E2eShapedDeliveryArrived";

    let (id, token) = create_hook(
        &client,
        &api,
        &name,
        event_type,
        json!({ "headers": ["X-GitHub-Event"] }),
    )
    .await;

    deliver(
        &client,
        &api,
        &id,
        &token,
        r#"{"action":"opened","number":7}"#,
        &[("x-github-event", "pull_request"), ("x-ignored", "nope")],
    )
    .await;

    let pool = PgPool::connect(&db_url()).await.expect("connect");
    let payload: Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE event_type = $1 ORDER BY created DESC LIMIT 1",
    )
    .bind(event_type)
    .fetch_one(&pool)
    .await
    .expect("the delivery's event");

    assert_eq!(payload["payload"]["action"], "opened");
    assert_eq!(payload["payload"]["number"], 7);
    assert_eq!(payload["headers"]["X-GitHub-Event"], "pull_request");
    assert!(
        payload["headers"]["X-Ignored"].is_null(),
        "a header off the allow-list is not carried"
    );
    assert!(
        !payload.to_string().contains(&token),
        "the bearer token never reaches the append-only log"
    );
    assert!(payload["summary"].is_string());
}

/// Deduping runs after verification, never before. A caller that cannot
/// authenticate learns nothing about which delivery ids this hook has seen.
#[tokio::test]
async fn an_unauthenticated_resend_is_refused_rather_than_called_a_duplicate() {
    let client = http_client();
    let api = base_url();
    let name = unique_marker("e2e-hook-authfirst");
    let event_type = "E2eAuthFirstDeliveryArrived";

    let (id, token) = create_hook(
        &client,
        &api,
        &name,
        event_type,
        json!({ "dedupe": { "header": "X-Delivery-Id" } }),
    )
    .await;

    let body = r#"{"action":"opened"}"#;
    let sent = [("x-delivery-id", "probe-1")];
    let (status, _) = deliver(&client, &api, &id, &token, body, &sent).await;
    assert_eq!(status, 202);

    let (status, refused) = deliver(&client, &api, &id, "wrong-token", body, &sent).await;
    assert_eq!(status, 401, "same delivery id, but it did not authenticate");
    assert_eq!(refused["error"], "unauthorized");
    assert!(refused["duplicate"].is_null());
}

/// A config that cannot do what it says is refused at create, not discovered
/// later from a sender's failed retries.
#[tokio::test]
async fn a_secret_bearing_header_is_refused_at_create() {
    let client = http_client();
    let api = base_url();

    for extra in [
        json!({ "headers": ["Authorization"] }),
        json!({ "dedupe": { "header": "Authorization" } }),
        json!({ "dedupe": { "window_secs": 604801 } }),
    ] {
        let mut body = json!({
            "name": unique_marker("e2e-hook-refused"),
            "event_type": "E2eNeverCreated",
        });
        for (key, value) in extra.as_object().unwrap() {
            body[key] = value.clone();
        }
        let resp = client
            .post(format!("{api}/api/v1/webhooks"))
            .json(&body)
            .send()
            .await
            .expect("create request failed");
        assert_eq!(resp.status(), 400, "should be refused: {extra}");
    }
}
