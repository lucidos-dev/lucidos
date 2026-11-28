use crate::support::{base_url, db_url, unique_marker, user_client};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

async fn read_ingress(client: &reqwest::Client, api: &str) -> Value {
    let resp = client
        .get(format!("{api}/api/v1/webhooks/ingress"))
        .send()
        .await
        .expect("ingress read failed");
    assert_eq!(resp.status(), 200, "ingress status");
    resp.json().await.expect("invalid JSON")
}

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

#[tokio::test]
async fn the_ingress_route_reports_a_standing_outage_and_drops_it_on_recovery() {
    let client = user_client().await;
    let api = base_url();
    let pool = PgPool::connect(&db_url()).await.expect("connect");

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

    assert_eq!(healthy["unverified"], false);

    for request in [
        json!({ "value": "http://hooks.example.com" }),
        json!({ "value": "https://hooks.example.com", "device_id": "e2e-device" }),
    ] {
        let result: Value = client
            .put(format!("{api}/api/v1/preferences?key=webhook_public_url"))
            .json(&request)
            .send()
            .await
            .expect("write preference")
            .json()
            .await
            .expect("preference result");
        assert_eq!(result["success"], false, "{result}");
    }

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

    let down_secs = outage["down_secs"].as_i64().expect("down_secs");
    assert!(
        (7195..=7260).contains(&down_secs),
        "down_secs was {down_secs}"
    );

    let same_instant: bool =
        sqlx::query_scalar("SELECT $1::timestamptz = (SELECT created FROM events WHERE id = $2)")
            .bind(outage["down_since"].as_str().expect("down_since"))
            .bind(declared)
            .fetch_one(&pool)
            .await
            .expect("compare down_since");
    assert!(same_instant, "down_since names the declaring event");

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

    let data = json!({ "webhook_id": hook_id, "webhook_name": name });
    seed_event(
        &pool,
        "WebhookIngressUnverified",
        &hook_id,
        data.clone(),
        0.0,
    )
    .await;
    let unverified = read_ingress(&client, &api).await;
    assert_eq!(unverified["unverified"], true);
    assert_eq!(unverified["degraded"], Value::Null);

    seed_event(&pool, "WebhookIngressVerified", &hook_id, data.clone(), 0.0).await;
    assert_eq!(read_ingress(&client, &api).await["unverified"], false);

    seed_event(&pool, "WebhookIngressUnverified", &hook_id, data, 0.0).await;
    let deleted = client
        .delete(format!("{api}/api/v1/webhooks/{hook_id}"))
        .send()
        .await
        .expect("delete ingress hook");
    assert!(deleted.status().is_success());
    sqlx::query("DELETE FROM events WHERE aggregate_id = $1 AND event_type LIKE 'WebhookIngress%'")
        .bind(&hook_id)
        .execute(&pool)
        .await
        .expect("clear seeded ingress events");
}
