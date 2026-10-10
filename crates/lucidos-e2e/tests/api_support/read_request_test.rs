//! E2E for the *read request* routes (ADR 0409): a coding agent asks for its
//! reply to be read through `lucidos request-read`, and the drawer reports
//! that the user saw it.
//!
//! What it pins, against a real engine:
//!
//! * `POST .../read-request` sets `read_requested` and records
//!   `ThreadReadRequested`, and refuses an unknown thread with a 404
//! * `POST .../read-request/seen` clears it once, records one
//!   `ThreadReplySeen`, and answers `seen: false` when nothing is pending

use crate::support::{base_url, db_url, poll_thread_summary_by_marker, unique_marker, user_client};
use uuid::Uuid;

async fn read_requested(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT read_requested FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("DB query failed")
}

async fn count_events(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
           AND event_type = $2",
    )
    .bind(thread_id.to_string())
    .bind(event_type)
    .fetch_one(pool)
    .await
    .expect("DB query failed")
}

async fn post(url: String) -> reqwest::Response {
    user_client()
        .await
        .post(url)
        .send()
        .await
        .expect("request failed")
}

/// Report a sighting as the drawer does: with the summary version it saw.
async fn see(pool: &sqlx::PgPool, thread_id: Uuid, url: &str) -> reqwest::Response {
    let summary_version: i64 =
        sqlx::query_scalar("SELECT summary_version FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(pool)
            .await
            .expect("DB query failed");
    user_client()
        .await
        .post(url)
        .json(&serde_json::json!({ "summary_version": summary_version }))
        .send()
        .await
        .expect("request failed")
}

#[tokio::test]
async fn a_read_request_is_set_over_http_and_a_sighting_clears_it_once() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");

    let marker = unique_marker("api-read-request");
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

    let request_url = format!("{}/api/v1/threads/{}/read-request", base_url(), thread_id);
    let seen_url = format!("{request_url}/seen");

    let resp = see(&pool, thread_id, &seen_url).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["seen"], false, "nothing was pending yet");

    let resp = post(request_url).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["status"], "requested");
    assert!(read_requested(&pool, thread_id).await);
    assert_eq!(
        count_events(&pool, thread_id, "ThreadReadRequested").await,
        1
    );

    let resp = see(&pool, thread_id, &seen_url).await;
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["seen"], true);
    assert!(!read_requested(&pool, thread_id).await);

    let resp = see(&pool, thread_id, &seen_url).await;
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["seen"], false, "a second device adds nothing");
    assert_eq!(count_events(&pool, thread_id, "ThreadReplySeen").await, 1);

    let resp = post(format!(
        "{}/api/v1/threads/{}/read-request",
        base_url(),
        Uuid::new_v4()
    ))
    .await;
    assert_eq!(resp.status(), 404, "an unknown thread asks for nothing");
}
