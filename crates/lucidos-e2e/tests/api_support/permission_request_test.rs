use crate::support::{
    base_url, count_events_of_type, db_url, local_process_client, seed_cc_thread_summary,
    user_client,
};
use uuid::Uuid;

async fn insert_permission_request_card(pool: &sqlx::PgPool, thread_id: Uuid, tool_use_id: &str) {
    seed_cc_thread_summary(pool, thread_id, "waiting_for_user_answer").await;
    let payload = serde_json::json!({
        "tool_use_id": tool_use_id,
        "cc_session_id": "sess_e2e",
        "question": "**Let this thread act outside its own subtree?**",
        "options": [
            { "id": "opt-0", "label": "Allow once" },
            { "id": "opt-1", "label": "Don't allow" },
        ],
        "permission_request": {
            "action": "apply",
            "breadth": thread_id,
            "prepared": "permission-request:e2e",
            "choices": {
                "opt-0": {
                    "kind": "grant",
                    "lifetime": "once",
                    "holder": { "kind": "thread", "thread_id": thread_id },
                },
                "opt-1": { "kind": "refuse" },
            },
        },
        "channel": "claude_code",
    });
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, 'UserQuestionAsked', $3, NOW(), $2)",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id)
    .bind(payload)
    .execute(pool)
    .await
    .expect("failed to insert the permission request card");
}

fn answer_question_url(thread_id: Uuid) -> String {
    format!(
        "{}/api/v1/threads/{}/answer-question",
        base_url(),
        thread_id
    )
}

#[tokio::test]
async fn a_permission_request_needs_a_thread_token() {
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/permission-requests", base_url()))
        .json(&serde_json::json!({
            "action": "apply",
            "target_thread_id": Uuid::new_v4(),
            "reason": "why",
        }))
        .send()
        .await
        .expect("request failed");
    assert_eq!(resp.status().as_u16(), 403);
    let body = resp.text().await.unwrap_or_default();
    assert!(body.contains("ask-permission"), "{body}");
}

#[tokio::test]
async fn only_the_owners_device_answers_a_permission_request() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let thread_id = Uuid::new_v4();
    let tool_use_id = format!(
        "tu-permission-{}",
        &Uuid::new_v4().as_simple().to_string()[..8]
    );
    insert_permission_request_card(&pool, thread_id, &tool_use_id).await;
    let body = serde_json::json!({ "tool_use_id": tool_use_id, "answer": { "kind": "Canceled" } });

    let refused = local_process_client()
        .post(answer_question_url(thread_id))
        .json(&body)
        .send()
        .await
        .expect("request failed");
    assert_eq!(refused.status().as_u16(), 403);
    assert_eq!(
        count_events_of_type(&pool, thread_id, "UserQuestionAnswered").await,
        0,
        "a refused answer leaves the card live"
    );

    let accepted = user_client()
        .await
        .post(answer_question_url(thread_id))
        .json(&body)
        .send()
        .await
        .expect("request failed");
    assert_eq!(accepted.status().as_u16(), 200);
    assert_eq!(
        count_events_of_type(&pool, thread_id, "UserQuestionAnswered").await,
        1
    );
}
