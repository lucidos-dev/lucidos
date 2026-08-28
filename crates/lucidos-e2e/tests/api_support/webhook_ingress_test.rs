//! E2E coverage for the ingress read route, which is what a cold page load is
//! told about the public delivery path.
//!
//! The scheduler's unit tests judge a probe. This is the route that turns the
//! newest declaration into the outage bar, over real HTTP.

use crate::support::{base_url, db_url, http_client, unique_marker};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// Read the route the Webhooks page and the outage bar both call.
async fn read_ingress(client: &reqwest::Client, api: &str) -> Value {
    let resp = client
        .get(format!("{api}/api/v1/webhooks/ingress"))
        .send()
        .await
        .expect("ingress read failed");
    assert_eq!(resp.status(), 200, "ingress status");
    resp.json().await.expect("invalid JSON")
}

/// Create one enabled hook and return its id.
async fn create_hook(client: &reqwest::Client, api: &str, name: &str) -> String {
    let body = json!({ "name": name, "event_type": "E2eIngressProbeArrived" });
    let resp = client
        .post(format!("{api}/api/v1/webhooks"))
        .json(&body)
        .send()
        .await
        .expect("create failed");
    assert_eq!(resp.status(), 200, "create webhook");
    let created: Value = resp.json().await.unwrap();
    created["id"].as_str().expect("an id").to_string()
}

/// Append one `WebhookIngress*` event, aged by `age_secs`, and return its id.
///
/// The engine writes a `SystemEvent` as `{"type": ..., "data": {...}}`, and the
/// read route parses `payload->'data'`. So the seed carries both halves.
async fn seed_event(
    pool: &PgPool,
    event_type: &str,
    hook_id: &str,
    data: Value,
    age_secs: f64,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate) \
         VALUES ($1, $2, $3, now() - make_interval(secs => $4), $5, 'webhook')",
    )
    .bind(id)
    .bind(event_type)
    .bind(json!({ "type": event_type, "data": data }))
    .bind(age_secs)
    .bind(hook_id)
    .execute(pool)
    .await
    .expect("seed ingress event");
    id
}

/// One test function, because every read answers from the single newest
/// `WebhookIngress*` event in the workspace. Two of these at once would each be
/// reading the other's seed.
#[tokio::test]
async fn the_ingress_route_reports_a_standing_outage_and_drops_it_on_recovery() {
    let client = http_client();
    let api = base_url();
    let pool = PgPool::connect(&db_url()).await.expect("connect");

    // A run against a kept workspace (`--no-reset`) inherits the last one's seeds.
    sqlx::query("DELETE FROM events WHERE event_type LIKE 'WebhookIngress%'")
        .execute(&pool)
        .await
        .expect("clear ingress events");

    let healthy = read_ingress(&client, &api).await;
    assert_eq!(
        healthy["degraded"],
        Value::Null,
        "no declaration, no outage"
    );

    let name = unique_marker("e2e-hook-ingress");
    let hook_id = create_hook(&client, &api, &name).await;
    let host = "hooks.example.ts.net";
    let two_hours = 7200.0;

    let declared = seed_event(
        &pool,
        "WebhookIngressDegraded",
        &hook_id,
        json!({
            "webhook_id": hook_id,
            "webhook_name": name,
            "host": host,
            "port": 8443,
            "degraded_families": ["ipv4"],
        }),
        two_hours,
    )
    .await;

    let standing = read_ingress(&client, &api).await;
    let outage = &standing["degraded"];
    assert_eq!(outage["host"], host, "the funnel hostname");
    assert_eq!(outage["port"], 8443, "the public port");
    assert_eq!(outage["families"], json!(["ipv4"]), "the dead family");

    // The age is measured by the database, never by subtracting a browser
    // clock from a server one (ADR 0053).
    let down_secs = outage["down_secs"].as_i64().expect("down_secs");
    assert!(
        (7195..=7260).contains(&down_secs),
        "down_secs was {down_secs}"
    );

    // Postgres reading it back proves the route emitted real RFC 3339, and
    // that the instant is the declaring event's own.
    let same_instant: bool =
        sqlx::query_scalar("SELECT $1::timestamptz = (SELECT created FROM events WHERE id = $2)")
            .bind(outage["down_since"].as_str().expect("down_since"))
            .bind(declared)
            .fetch_one(&pool)
            .await
            .expect("compare down_since");
    assert!(same_instant, "down_since names the declaring event");

    // A newer recovery retracts it. The "no enabled webhook left" branch is not
    // asserted here, because sibling tests create enabled hooks concurrently.
    seed_event(
        &pool,
        "WebhookIngressRecovered",
        &hook_id,
        json!({
            "webhook_id": hook_id,
            "webhook_name": name,
            "host": host,
            "port": 8443,
            "recovered_families": ["ipv4"],
        }),
        0.0,
    )
    .await;

    let recovered = read_ingress(&client, &api).await;
    assert_eq!(
        recovered["degraded"],
        Value::Null,
        "a recovery retracts the outage"
    );
}
