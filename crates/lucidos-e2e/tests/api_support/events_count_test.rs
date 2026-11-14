//! `GET /api/v1/events/count` honours `thread_id`.
//!
//! The grouped `events` tool offers `thread_id` on `count`, and the endpoint
//! once ignored it, so a thread-scoped count returned the workspace total.

use crate::support::{base_url, db_url, http_client};
use serde_json::Value;
use uuid::Uuid;

async fn seed(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str, n: i32) {
    for _ in 0..n {
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
             VALUES ($1, $2, '{}'::jsonb, NOW(), $3::text, 'thread', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(event_type)
        .bind(thread_id)
        .execute(pool)
        .await
        .expect("seed event");
    }
}

async fn count(params: &[(&str, String)]) -> (reqwest::StatusCode, String) {
    let res = http_client()
        .get(format!("{}/api/v1/events/count", base_url()))
        .query(params)
        .send()
        .await
        .expect("count request");
    (res.status(), res.text().await.expect("count body"))
}

#[tokio::test]
async fn a_thread_scoped_count_counts_only_that_thread() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to e2e DB");
    let (mine, other) = (Uuid::new_v4(), Uuid::new_v4());
    let event_type = format!("CountedFixture{}", mine.simple());
    seed(&pool, mine, &event_type, 2).await;
    seed(&pool, other, &event_type, 1).await;

    let (status, body) = count(&[
        ("type", event_type.clone()),
        ("thread_id", mine.to_string()),
    ])
    .await;
    assert!(status.is_success(), "scoped count: {status} {body}");
    let scoped: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(scoped["count"], 2);

    let (status, body) = count(&[("type", event_type)]).await;
    assert!(status.is_success(), "unscoped count: {status} {body}");
    let unscoped: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(unscoped["count"], 3, "no thread_id is every thread");

    let (status, body) = count(&[("thread_id", mine.to_string())]).await;
    assert!(status.is_success(), "scoped breakdown: {status} {body}");
    let breakdown: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(breakdown["total_count"], 2);
}

#[tokio::test]
async fn a_malformed_thread_id_is_refused_not_ignored() {
    for bad in ["not-a-uuid", "current"] {
        let (status, body) = count(&[("thread_id", bad.to_string())]).await;
        assert_eq!(status, 400, "{bad} must be a 400, got {status}: {body}");
    }
}
