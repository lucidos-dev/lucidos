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
async fn a_pick_while_a_turn_runs_is_saved_for_the_next_turn() {
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
        assert_eq!(code, 200, "{status}: {body}");
        assert_eq!(recorded_picks(&pool, thread_id).await, 1, "{status}");
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

#[tokio::test]
async fn private_path_settings_require_a_device() {
    let url = format!("{}/api/v1/private-paths", base_url());
    for client in [
        crate::support::local_process_client(),
        crate::support::unattested_user_client().await,
    ] {
        let response = client
            .put(&url)
            .json(&json!({"scope": "workspace", "paths": ["/example/private"], "previous": []}))
            .send()
            .await
            .unwrap();
        assert!(matches!(response.status().as_u16(), 401 | 403));
    }
    let client = user_client().await;
    let response = client
        .put(&url)
        .json(&json!({"scope": "workspace", "paths": ["relative/path"], "previous": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let response = client.get(url).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.unwrap();
    assert!(body["machine"].is_array());
    assert!(body["workspace"].is_array());
    assert!(body["enforcement"].is_string());
}

#[tokio::test]
async fn private_path_settings_round_trip_and_reject_stale_edits() {
    let url = format!("{}/api/v1/private-paths", base_url());
    let client = user_client().await;
    let initial: Value = client
        .get(&url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let previous = initial["workspace"].clone();
    let blocked = client
        .put(&url)
        .json(&json!({
            "scope": "workspace",
            "paths": [crate::support::workspace_path().join("data/artifacts/private-test-path")],
            "previous": previous,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status().as_u16(), 400);
    let path = format!("/example/private-test-{}", Uuid::new_v4());
    let mut changed = previous.as_array().unwrap().clone();
    changed.push(json!(path));
    let response = client
        .put(&url)
        .json(&json!({
            "scope": "workspace", "paths": changed, "previous": previous,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let updated: Value = response.json().await.unwrap();
    assert!(updated["workspace"]
        .as_array()
        .unwrap()
        .contains(&json!(path)));
    assert_eq!(updated["machine"], initial["machine"]);
    let response = client
        .put(&url)
        .json(&json!({
            "scope": "workspace", "paths": previous, "previous": previous,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 409);
    client
        .put(&url)
        .json(&json!({
            "scope": "workspace", "paths": previous, "previous": updated["workspace"],
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
}
