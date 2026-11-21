//! E2E coverage for the refusal read route. It is what a cold page load is
//! told about a hook that throws away the deliveries it does get.
//!
//! The engine's unit tests judge a row. This is the route that turns a row plus
//! a declaration into the bar, over real HTTP. It is also the only place the
//! two halves meet: the declaration gates, and the live row describes.
//!
//! See `docs/adr/0235-a-refused-delivery-is-an-outage.md`.

use crate::support::{base_url, db_url, unique_marker, user_client};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// Read the route the Webhooks page and the refusal bar both call.
async fn read_refusals(client: &reqwest::Client, api: &str) -> Value {
    let resp = client
        .get(format!("{api}/api/v1/webhooks/refusals"))
        .send()
        .await
        .expect("refusals read failed");
    assert_eq!(resp.status(), 200, "refusals status");
    resp.json().await.expect("invalid JSON")
}

/// Create one hook and return its id.
async fn create_hook(client: &reqwest::Client, api: &str, name: &str) -> String {
    let body = json!({ "name": name, "event_type": "E2eRefusedDeliveryArrived" });
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

/// Put a hook into the state a long refusal run leaves behind.
///
/// Written straight to the row, because the engine's own writer needs real
/// refused deliveries and a clock this test cannot wind. The ages come from
/// `now()`, so they are the database's own, exactly as the route reads them.
async fn seed_run(pool: &PgPool, hook_id: &str, enabled: bool, age_secs: f64, count: i32) {
    sqlx::query(
        "UPDATE webhooks SET enabled = $2, \
         refusal_run_count = $3, \
         refusal_run_since = now() - make_interval(secs => $4), \
         refusal_run_cause = 'disabled', \
         refusal_run_reasons = jsonb_build_object('disabled', $3::bigint), \
         last_refused_at = now() - make_interval(secs => 60), \
         last_refusal_reason = 'the webhook is disabled' \
         WHERE id = $1::uuid",
    )
    .bind(hook_id)
    .bind(enabled)
    .bind(count)
    .bind(age_secs)
    .execute(pool)
    .await
    .expect("seed refusal run");
}

/// Append one declaration for this hook, aged by `age_secs`.
async fn seed_declaration(pool: &PgPool, hook_id: &str, name: &str, age_secs: f64) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate) \
         VALUES ($1, 'WebhookDeliveriesRefused', $2, now() - make_interval(secs => $3), \
                 $4, 'webhook')",
    )
    .bind(Uuid::new_v4())
    .bind(json!({
        "type": "WebhookDeliveriesRefused",
        "data": {
            "webhook_id": hook_id,
            "webhook_name": name,
            "enabled": false,
            "cause": "disabled",
            "refusals": 42,
            "reasons": { "disabled": 42 },
            "refusing_since": "2026-09-02T04:41:06Z",
            "refusing_secs": 1_555_200,
        }
    }))
    .bind(age_secs)
    .bind(hook_id)
    .execute(pool)
    .await
    .expect("seed declaration");
}

/// This hook's entry in the route's answer, or `None`.
fn entry_for<'a>(answer: &'a Value, hook_id: &str) -> Option<&'a Value> {
    answer["refusing"]
        .as_array()
        .expect("refusing is a list")
        .iter()
        .find(|entry| entry["webhook_id"] == hook_id)
}

/// One test function, because sibling tests create hooks in the same workspace
/// and the route answers over all of them. Every assertion is scoped to the
/// hook this test made.
#[tokio::test]
async fn the_refusals_route_reports_a_declared_hook_and_drops_it_when_the_row_clears() {
    let client = user_client().await;
    let api = base_url();
    let pool = PgPool::connect(&db_url()).await.expect("connect");

    let name = unique_marker("e2e-hook-refusal");
    let hook_id = create_hook(&client, &api, &name).await;

    // A fresh hook is refusing nothing, and the static route resolves ahead of
    // its `/webhooks/:id` sibling rather than parsing "refusals" as a uuid.
    let quiet = read_refusals(&client, &api).await;
    assert!(
        entry_for(&quiet, &hook_id).is_none(),
        "nothing declared yet"
    );

    // A run alone is not enough. The engine having SAID so is what raises the
    // bar, so a fault no cycle has declared draws nothing.
    let eighteen_days = 18.0 * 86_400.0;
    seed_run(&pool, &hook_id, false, eighteen_days, 42).await;
    let undeclared = read_refusals(&client, &api).await;
    assert!(
        entry_for(&undeclared, &hook_id).is_none(),
        "the route gates on the declaration, never on the row alone"
    );

    seed_declaration(&pool, &hook_id, &name, 3600.0).await;
    let standing = read_refusals(&client, &api).await;
    let entry = entry_for(&standing, &hook_id).expect("the declared hook");
    assert_eq!(entry["webhook_name"], name.as_str());
    assert_eq!(entry["cause"], "disabled");
    assert_eq!(entry["enabled"], false, "the one-click recovery");
    assert_eq!(entry["refusals"], 42);
    assert_eq!(entry["reasons"]["disabled"], 42);

    // The age is the RUN's, measured by the database, and it predates the
    // declaration by seventeen days (ADR 0053).
    let secs = entry["refusing_secs"].as_i64().expect("refusing_secs");
    assert!(
        (1_555_000..=1_556_000).contains(&secs),
        "refusing_secs was {secs}, expected the run's own age"
    );

    // Switching the hook back on ends the fault the declaration named. The
    // route says so at once, without waiting for the next cycle to retract.
    sqlx::query("UPDATE webhooks SET enabled = TRUE WHERE id = $1::uuid")
        .bind(&hook_id)
        .execute(&pool)
        .await
        .expect("re-enable");
    let re_enabled = read_refusals(&client, &api).await;
    assert!(
        entry_for(&re_enabled, &hook_id).is_none(),
        "a switched-on hook must not still read as switched off"
    );

    // An app may not read which hooks are broken, the same rule every other
    // webhook route carries.
    let as_app = client
        .get(format!("{api}/api/v1/webhooks/refusals"))
        .header("x-lucidos-app-id", "habit-tracker")
        .send()
        .await
        .expect("app-stamped read failed");
    assert_eq!(as_app.status(), 403, "an app reaches no webhook route");
}
