//! E2E for the *expected wake* route (ADR 0603): a coding agent states it
//! through `lucidos expect-wake <duration> --reason <text>`.
//!
//! What it pins, against a real engine:
//!
//! * `POST .../expected-wake` refuses a body without `in_secs` with a 400
//! * it refuses a span past 24 hours, and an empty reason, with a 400
//! * a valid body records one `WakeExpected` with `source: agent`
//! * an unknown thread gets a 404

use crate::support::{base_url, db_url, poll_thread_summary_by_marker, unique_marker, user_client};
use uuid::Uuid;

async fn post(url: &str, body: serde_json::Value) -> reqwest::Response {
    user_client()
        .await
        .post(url)
        .json(&body)
        .send()
        .await
        .expect("request failed")
}

#[tokio::test]
async fn an_expected_wake_is_recorded_over_http() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");

    let marker = unique_marker("api-expected-wake");
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({ "message": marker, "mode": "human" }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200);
    let thread_id = poll_thread_summary_by_marker(&pool, &marker, 20)
        .await
        .thread_id;
    let url = format!("{}/api/v1/threads/{}/expected-wake", base_url(), thread_id);

    let resp = post(&url, serde_json::json!({ "reason": "the build" })).await;
    assert_eq!(resp.status(), 400, "a span has no default");
    let resp = post(
        &url,
        serde_json::json!({ "in_secs": 90_000, "reason": "the build" }),
    )
    .await;
    assert_eq!(resp.status(), 400, "past every wait's timeout");
    let resp = post(&url, serde_json::json!({ "in_secs": 600, "reason": " " })).await;
    assert_eq!(resp.status(), 400, "a reason is required");

    let resp = post(
        &url,
        serde_json::json!({ "in_secs": 2700, "reason": "the build" }),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["status"], "recorded");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("about 45 min"),
        "{body}"
    );
    let recorded: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
           AND event_type = 'WakeExpected'",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .expect("DB query failed");
    assert_eq!(recorded.len(), 1, "the refusals recorded nothing");
    assert_eq!(recorded[0]["source"], "agent");
    assert_eq!(recorded[0]["reason"], "the build");

    let unknown = format!(
        "{}/api/v1/threads/{}/expected-wake",
        base_url(),
        Uuid::new_v4()
    );
    let resp = post(
        &unknown,
        serde_json::json!({ "in_secs": 60, "reason": "x" }),
    )
    .await;
    assert_eq!(resp.status(), 404);
}
