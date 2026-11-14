//! `POST /api/v1/chat/settings` records a rung pick on a Lucidos Agent thread.
//! While a turn runs, the rung is locked: the pick is refused with 409 and
//! `reason: "turn-running"`, and nothing is recorded.

use crate::support::{base_url, db_url, seed_chat_thread_summary, user_client};
use serde_json::{json, Value};
use uuid::Uuid;

async fn pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to e2e DB")
}

async fn pick(body: Value) -> (u16, Value) {
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/chat/settings", base_url()))
        .json(&body)
        .send()
        .await
        .expect("settings request failed");
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

async fn recorded_picks(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events \
         WHERE thread_id = $1 AND event_type = 'ChatSettingsChanged'",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .expect("count picks")
}

#[tokio::test]
async fn a_pick_while_a_turn_runs_is_refused_as_turn_running() {
    let pool = pool().await;
    for status in ["running", "waiting_for_user_answer", "paused"] {
        let thread_id = Uuid::new_v4();
        seed_chat_thread_summary(&pool, thread_id, status).await;
        let (code, body) = pick(json!({
            "thread_id": thread_id,
            "model": "claude-opus-5-5",
            "reasoning_effort": "high",
        }))
        .await;
        assert_eq!(code, 409, "{status}: {body}");
        assert_eq!(body["reason"], "turn-running", "{status}: {body}");
        assert_eq!(recorded_picks(&pool, thread_id).await, 0, "{status}");
    }
}

#[tokio::test]
async fn a_pick_naming_no_level_is_refused() {
    let pool = pool().await;
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "idle").await;
    let (code, body) = pick(json!({
        "thread_id": thread_id,
        "model": "claude-opus-5-5",
        "reasoning_effort": "turbo",
    }))
    .await;
    assert_eq!(code, 400, "{body}");
    assert_eq!(recorded_picks(&pool, thread_id).await, 0);
}

#[tokio::test]
async fn a_pick_on_an_unknown_thread_is_not_found() {
    let (code, body) = pick(json!({
        "thread_id": Uuid::new_v4(),
        "model": "claude-opus-5-5",
    }))
    .await;
    assert_eq!(code, 404, "{body}");
}
