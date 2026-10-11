//! E2E for the two routes an *owner approval* (ADR 0387) adds behaviour to.
//!
//! An external client cannot mint a thread-bound origin token, so the token
//! paths (the request itself, the spend) are tested in the engine. What a real
//! engine must show from outside: a caller with no thread token cannot ask,
//! and only the owner's registered device answers an approval card.

use crate::support::{
    base_url, count_events_of_type, db_url, local_process_client, seed_cc_thread_summary,
    user_client,
};
use uuid::Uuid;

/// An owner approval card as the ask route stores it: the engine's question,
/// the parser's option ids, and the act it proposes. The labels pin the wire
/// contract: the engine's spend lookup matches "Allow once" by its text.
async fn insert_owner_approval_card(pool: &sqlx::PgPool, thread_id: Uuid, tool_use_id: &str) {
    seed_cc_thread_summary(pool, thread_id, "waiting_for_user_answer").await;
    let payload = serde_json::json!({
        "tool_use_id": tool_use_id,
        "cc_session_id": "sess_e2e",
        "question": "**Let this thread act outside its own subtree, once?**",
        "options": [
            { "id": "opt-0", "label": "Allow once" },
            { "id": "opt-1", "label": "Don't allow" },
        ],
        "owner_approval": { "verb": "create-top-thread" },
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
    .expect("failed to insert the approval card");
}

fn answer_question_url(thread_id: Uuid) -> String {
    format!(
        "{}/api/v1/threads/{}/answer-question",
        base_url(),
        thread_id
    )
}

/// Asking is a coding agent's act inside its own thread. The owner's own
/// device presents no thread token, so it has no thread to ask on.
#[tokio::test]
async fn an_owner_approval_request_needs_a_thread_token() {
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/owner-approvals", base_url()))
        .json(&serde_json::json!({ "verb": "create-top-thread", "reason": "why" }))
        .send()
        .await
        .expect("request failed");
    assert_eq!(resp.status().as_u16(), 403);
    let body = resp.text().await.unwrap_or_default();
    assert!(body.contains("ask-owner-approval"), "{body}");
}

/// The machine's own token is a credential, but not a person: it may not
/// answer a card whose Allow is spendable authority. The owner's device may.
#[tokio::test]
async fn only_the_owners_device_answers_an_owner_approval_card() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let thread_id = Uuid::new_v4();
    let tool_use_id = format!(
        "tu-approval-{}",
        &Uuid::new_v4().as_simple().to_string()[..8]
    );
    insert_owner_approval_card(&pool, thread_id, &tool_use_id).await;
    // Canceled keeps the test hermetic: a cancel spawns no coding agent.
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
