//! E2E for the *wake decision* route: a coding-agent child decides through
//! `lucidos wake-parent yes|no` whether its report wakes its parent.
//!
//! What it pins, against a real engine:
//!
//! * `POST .../wake-decision` refuses a body without `wake` with a 400
//! * `{"wake": false}` records `ParentWakeNotRequested` and sets the child's
//!   `parent_wake_requested` FALSE, and `{"wake": true}` sets it TRUE
//! * a thread with no parent gets a 409, and an unknown thread a 404
//!
//! Rows are seeded directly, as `detach_test.rs` does, so no LLM round-trip is
//! needed to build a family.

use crate::support::{base_url, count_events_of_type, db_url, user_client};
use uuid::Uuid;

async fn seed(pool: &sqlx::PgPool, thread_id: Uuid, parent: Option<Uuid>, depth: i32) {
    sqlx::query(
        "INSERT INTO thread_summaries (\
             thread_id, parent_thread_id, depth, source, is_coding_agent, created_at, \
             last_activity, message_count, status, archive_state, has_response, state \
         ) VALUES ($1, $2, $3, 'chat', FALSE, NOW(), NOW(), 0, 'running', 'inbox', TRUE, 'active')",
    )
    .bind(thread_id)
    .bind(parent)
    .bind(depth)
    .execute(pool)
    .await
    .expect("seed thread_summaries row");
}

async fn cleanup(pool: &sqlx::PgPool, ids: &[Uuid]) {
    let _ = sqlx::query("DELETE FROM events WHERE thread_id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM thread_summaries WHERE thread_id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await;
}

async fn decide(thread_id: Uuid, body: serde_json::Value) -> reqwest::Response {
    user_client()
        .await
        .post(format!(
            "{}/api/v1/threads/{thread_id}/wake-decision",
            base_url()
        ))
        .json(&body)
        .send()
        .await
        .expect("request failed")
}

async fn parent_wake_requested(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<bool> {
    sqlx::query_scalar("SELECT parent_wake_requested FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("DB query failed")
}

#[tokio::test]
async fn a_child_records_its_wake_decision_over_http() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let (parent, child) = (Uuid::new_v4(), Uuid::new_v4());
    seed(&pool, parent, None, 0).await;
    seed(&pool, child, Some(parent), 1).await;

    let resp = decide(child, serde_json::json!({})).await;
    assert_eq!(resp.status(), 400, "a decision has no default");

    let resp = decide(child, serde_json::json!({ "wake": false })).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["status"], "file");
    assert_eq!(parent_wake_requested(&pool, child).await, Some(false));
    assert_eq!(
        count_events_of_type(&pool, child, "ParentWakeNotRequested").await,
        1
    );

    let resp = decide(child, serde_json::json!({ "wake": true })).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["status"], "wake");
    assert_eq!(parent_wake_requested(&pool, child).await, Some(true));

    cleanup(&pool, &[parent, child]).await;
}

#[tokio::test]
async fn a_thread_without_a_parent_has_nothing_to_wake() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let top = Uuid::new_v4();
    seed(&pool, top, None, 0).await;

    let resp = decide(top, serde_json::json!({ "wake": false })).await;
    assert_eq!(resp.status(), 409);
    assert_eq!(
        count_events_of_type(&pool, top, "ParentWakeNotRequested").await,
        0
    );

    let resp = decide(Uuid::new_v4(), serde_json::json!({ "wake": true })).await;
    assert_eq!(resp.status(), 404);

    cleanup(&pool, &[top]).await;
}

/// A child whose spawn declared a wake mode records nothing: the answer is
/// `declared`, and no `ParentWake*` event lands (ADR 0451).
#[tokio::test]
async fn a_declared_child_records_no_wake_decision() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");
    let (parent, child) = (Uuid::new_v4(), Uuid::new_v4());
    seed(&pool, parent, None, 0).await;
    seed(&pool, child, Some(parent), 1).await;
    sqlx::query("UPDATE thread_summaries SET wake_mode = 'no-wake' WHERE thread_id = $1")
        .bind(child)
        .execute(&pool)
        .await
        .expect("declare the mode");

    let resp = decide(child, serde_json::json!({ "wake": true })).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["status"], "declared");
    assert!(body["message"].as_str().unwrap().contains("`no-wake`"));
    assert_eq!(parent_wake_requested(&pool, child).await, None);
    assert_eq!(
        count_events_of_type(&pool, child, "ParentWakeRequested").await,
        0
    );

    cleanup(&pool, &[parent, child]).await;
}
