//! Every thread summary the API serves carries the row's `summary_version`,
//! and the number grows whenever the row changes. The client refuses any
//! summary older than the one it holds. A read path that dropped the field or
//! served a stale number would let an old status back on screen.
//!
//! See ADR 0329.

use crate::support::{base_url, db_url, http_client, seed_chat_thread_summary};
use serde_json::Value;
use uuid::Uuid;

async fn pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to e2e DB")
}

async fn get_json(path: String) -> Value {
    let res = http_client()
        .get(format!("{}{}", base_url(), path))
        .send()
        .await
        .expect("request");
    assert!(res.status().is_success(), "{path}: status {}", res.status());
    res.json().await.expect("json")
}

async fn row_version(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT summary_version FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("read summary_version")
}

/// The version as the by-id read and the events read each report it.
async fn served_versions(thread_id: Uuid) -> (i64, i64) {
    let summary = get_json(format!("/api/v1/threads/{thread_id}")).await;
    let snapshot = get_json(format!("/api/v1/threads/{thread_id}/events")).await;
    let by_id = summary["summary_version"]
        .as_i64()
        .unwrap_or_else(|| panic!("GET /threads/:id carries no summary_version: {summary}"));
    let aggregate = snapshot["currentAggregate"]["summaryVersion"]
        .as_i64()
        .unwrap_or_else(|| panic!("currentAggregate carries no summaryVersion: {snapshot}"));
    (by_id, aggregate)
}

#[tokio::test]
async fn both_read_paths_serve_the_row_version() {
    let pool = pool().await;
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "idle").await;

    let row = row_version(&pool, thread_id).await;
    assert_eq!(served_versions(thread_id).await, (row, row));
}

#[tokio::test]
async fn the_served_version_grows_when_the_row_changes() {
    let pool = pool().await;
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "idle").await;
    let (before, _) = served_versions(thread_id).await;

    sqlx::query("UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .expect("change the row");

    let (by_id, aggregate) = served_versions(thread_id).await;
    assert!(
        by_id > before,
        "by-id read did not advance: {before} -> {by_id}"
    );
    assert_eq!(aggregate, by_id, "the two read paths disagree");
}
