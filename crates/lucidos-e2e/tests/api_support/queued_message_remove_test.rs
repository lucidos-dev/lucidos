//! `POST /api/v1/chat/queued-message/remove` on a Lucidos Agent thread is a
//! *consumption claim* (ADR 0706). A 200 means the message never reaches the
//! agent. A message the agent already has answers 409 `already_read`.

use crate::support::{base_url, db_url, seed_chat_thread_summary, user_client};
use uuid::Uuid;

async fn pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database")
}

async fn remove(client: &reqwest::Client, thread_id: Uuid, message: Uuid) -> reqwest::Response {
    client
        .post(format!("{}/api/v1/chat/queued-message/remove", base_url()))
        .json(&serde_json::json!({
            "thread_id": thread_id.to_string(),
            "message_id": message.to_string(),
        }))
        .send()
        .await
        .expect("removal request sends")
}

async fn insert_event(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    id: Uuid,
    event_type: &str,
    payload: serde_json::Value,
) {
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, $3, $4, NOW(), $2)",
    )
    .bind(id)
    .bind(thread_id)
    .bind(event_type)
    .bind(payload)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("failed to insert {event_type}: {e}"));
}

async fn tombstones(pool: &sqlx::PgPool, thread_id: Uuid, message: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 \
           AND event_type = 'QueuedMessageRemoved' \
           AND payload->>'removed_message_id' = $2::text",
    )
    .bind(thread_id)
    .bind(message)
    .fetch_one(pool)
    .await
    .expect("events query failed")
}

#[tokio::test]
async fn removing_a_message_the_agent_has_not_read_is_accepted_once() {
    let pool = pool().await;
    let client = user_client().await;
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "running").await;
    let message = Uuid::new_v4();

    assert_eq!(remove(&client, thread_id, message).await.status(), 200);
    assert_eq!(
        remove(&client, thread_id, message).await.status(),
        200,
        "a repeated removal stays idempotent"
    );
    assert_eq!(tombstones(&pool, thread_id, message).await, 1);
}

#[tokio::test]
async fn repeated_removal_reports_compose_when_stop_won() {
    let pool = pool().await;
    let client = user_client().await;
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "running").await;
    let message = Uuid::new_v4();
    let compose = serde_json::json!({
        "summary_version": 8, "compose_epoch": 3,
        "text": "saved draft\n\nrestored input", "image_hashes": [],
    });
    insert_event(
        &pool,
        thread_id,
        Uuid::new_v4(),
        "QueuedMessageRemoved",
        serde_json::json!({
            "removed_message_id": message, "restore_to_compose": true, "compose": compose,
        }),
    )
    .await;
    for _ in 0..2 {
        let response = remove(&client, thread_id, message).await;
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.expect("removal response");
        assert_eq!(
            body["compose"], compose,
            "Edit must not append Stop's content again"
        );
    }
    assert_eq!(tombstones(&pool, thread_id, message).await, 1);
}

#[tokio::test]
async fn removing_a_message_the_agent_already_has_answers_already_read() {
    let pool = pool().await;
    let client = user_client().await;
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "running").await;
    let message = Uuid::new_v4();
    insert_event(
        &pool,
        thread_id,
        message,
        "MessageReceived",
        serde_json::json!({
            "text": "follow-up",
            "mode": "human",
            "request_event_id": Uuid::new_v4().to_string(),
        }),
    )
    .await;
    insert_event(
        &pool,
        thread_id,
        Uuid::new_v4(),
        "PromptInjected",
        serde_json::json!({
            "text": "follow-up",
            "mode": "human",
            "injected_message_id": message.to_string(),
        }),
    )
    .await;

    let response = remove(&client, thread_id, message).await;
    assert_eq!(response.status(), 409);
    let body: serde_json::Value = response.json().await.expect("an error body");
    assert_eq!(body["reason"], "already_read");
    assert_eq!(tombstones(&pool, thread_id, message).await, 0);
}

/// Fixtures represent durable acceptance without scheduling a real model call.
/// Stop must work for an input that has no live handle and no message row yet.
#[tokio::test]
async fn cancel_takes_back_unstarted_inputs_and_returns_content_without_a_live_turn() {
    let pool = pool().await;
    let client = user_client().await;
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "running").await;
    let unread = Uuid::new_v4();
    let consumed = Uuid::new_v4();
    for message in [unread, consumed] {
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created) \
             VALUES ($1, 'system', 'system', 'OrdinarySubmissionAccepted', $2, NOW())",
        )
        .bind(Uuid::new_v4())
        .bind(
            serde_json::json!({ "type": "OrdinarySubmissionAccepted", "data": {
                "thread_id": thread_id, "message_event_id": message,
                "entry_id": Uuid::new_v4(),
                "request": {
                    "type": "agent-chat", "thread_id": thread_id,
                    "event_id": message, "mode": "human",
                    "message": "unstarted input", "image_hashes": ["test-image"]
                }
            }}),
        )
        .execute(&pool)
        .await
        .expect("seed durable acceptance");
    }
    insert_event(
        &pool,
        thread_id,
        consumed,
        "MessageReceived",
        serde_json::json!({ "text": "already read", "mode": "human" }),
    )
    .await;
    insert_event(
        &pool,
        thread_id,
        Uuid::new_v4(),
        "PromptInjected",
        serde_json::json!({ "text": "already read", "mode": "human",
            "injected_message_id": consumed }),
    )
    .await;

    let response = client
        .post(format!(
            "{}/api/v1/chat/cancel?thread_id={thread_id}",
            base_url()
        ))
        .send()
        .await
        .expect("cancel sends");
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("cancel JSON");
    let taken = body["taken_back_messages"].as_array().unwrap();
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0]["thread_id"], thread_id.to_string());
    assert_eq!(taken[0]["id"], unread.to_string());
    assert_eq!(taken[0]["text"], "unstarted input");
    assert_eq!(taken[0]["image_hashes"], serde_json::json!(["test-image"]));
    let report = &taken[0]["compose"];
    assert_eq!(report["text"], "unstarted input");
    assert_eq!(report["restorations"][0]["text"], "unstarted input");
    let saved: (String, serde_json::Value) = sqlx::query_as(
        "SELECT compose_text,compose_restorations FROM thread_summaries WHERE thread_id=$1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(saved.0, "unstarted input");
    assert_eq!(saved.1, report["restorations"]);
    assert_eq!(body["failed_take_back_ids"], serde_json::json!([]));
    assert_eq!(tombstones(&pool, thread_id, unread).await, 1);
    assert_eq!(tombstones(&pool, thread_id, consumed).await, 0);
}

#[tokio::test]
async fn a_coding_message_held_by_a_permission_card_can_be_taken_back() {
    let pool = pool().await;
    let client = user_client().await;
    let thread_id = Uuid::new_v4();
    crate::support::seed_cc_thread_summary(&pool, thread_id, "waiting_for_user_answer").await;
    insert_event(
        &pool,
        thread_id,
        Uuid::new_v4(),
        "CodingAgentPermissionRequest",
        serde_json::json!({"request_id": "permission-test", "channel": "claude_code"}),
    )
    .await;

    let response = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&serde_json::json!({
            "thread_id": thread_id,
            "message": "Keep this in the queue",
            "mode": "human",
            "use_coding_agent": true,
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, 200, "send response: {body}");
    let receipt: serde_json::Value = serde_json::from_str(&body).unwrap();
    let message = Uuid::parse_str(receipt["event_id"].as_str().unwrap()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let recorded: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE id=$1 AND payload->>'queued_for_permission'='true')")
                .bind(message).fetch_one(&pool).await.unwrap();
            if recorded {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the queued message becomes visible while the card is open");
    let response = remove(&client, thread_id, message).await;
    let status = response.status();
    assert_eq!(
        status,
        200,
        "take-back response: {}",
        response.text().await.unwrap()
    );
    assert_eq!(tombstones(&pool, thread_id, message).await, 1);
    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id=$1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "waiting_for_user_answer");
}
