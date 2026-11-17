//! Coverage for `/api/v1/permissions`, permission batches and a thread's
//! `/automation-pause` (ADR 0664).
//!
//! A batch needs a thread-bound origin token, which only a subprocess the
//! engine spawned holds. So this suite covers the list, the refusals, a pause
//! and unpause from a device, and the CLI loop. The engine's `permission` and
//! `automation_pause` tests cover a grant and a batch.

use crate::support::{
    base_url, db_url, seed_cc_thread_summary, seed_chat_thread_summary, unattested_user_client,
    user_client,
};

#[tokio::test]
async fn permissions_list_answers_a_list() {
    let client = user_client().await;
    for query in ["", "?all=true"] {
        let resp = client
            .get(format!("{}/api/v1/permissions{query}", base_url()))
            .send()
            .await
            .expect("permissions list sends");
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.expect("list is JSON");
        assert!(body["permissions"].is_array(), "got {body}");
    }
}

/// Revoking an unknown permission is a 404, and a malformed id a 400.
#[tokio::test]
async fn revoke_names_what_it_could_not_find() {
    let client = user_client().await;
    let unknown = uuid::Uuid::new_v4();
    let resp = client
        .delete(format!("{}/api/v1/permissions/{unknown}", base_url()))
        .send()
        .await
        .expect("revoke sends");
    assert_eq!(resp.status(), 404);

    let resp = client
        .delete(format!("{}/api/v1/permissions/not-a-uuid", base_url()))
        .send()
        .await
        .expect("revoke sends");
    assert_eq!(resp.status(), 400);
}

/// A batch rides its raiser's thread, so a device, which has none, is refused.
#[tokio::test]
async fn a_batch_needs_a_thread_to_raise_it_on() {
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/permissions/batches", base_url()))
        .json(&serde_json::json!({ "items": [] }))
        .send()
        .await
        .expect("batch sends");
    assert_eq!(resp.status(), 403);
    let body: serde_json::Value = resp.json().await.expect("standard error body");
    assert!(body["error"]
        .as_str()
        .expect("error is a string")
        .contains("thread's agent"));
}

fn pause_url(thread_id: uuid::Uuid) -> String {
    format!("{}/api/v1/threads/{thread_id}/automation-pause", base_url())
}

/// The owner pauses a coding-agent thread and unpauses it. A repeat changes
/// nothing, and an unattested caller cannot unpause.
#[tokio::test]
async fn the_owner_pauses_and_unpauses_a_coding_agent_thread() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let thread_id = uuid::Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "idle").await;
    let client = user_client().await;

    for changed in [true, false] {
        let resp = client
            .post(pause_url(thread_id))
            .json(&serde_json::json!({ "reason": "checkpoint" }))
            .send()
            .await
            .expect("pause sends");
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.expect("pause is JSON");
        assert_eq!(
            body,
            serde_json::json!({ "paused": true, "changed": changed })
        );
    }

    let resp = unattested_user_client()
        .await
        .delete(pause_url(thread_id))
        .send()
        .await
        .expect("unpause sends");
    assert_eq!(resp.status(), 403, "only the attested owner unpauses");

    let resp = client
        .delete(pause_url(thread_id))
        .send()
        .await
        .expect("unpause sends");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("unpause is JSON");
    assert_eq!(
        body,
        serde_json::json!({ "paused": false, "changed": true })
    );
}

/// A pause names what it refused: an unknown thread is a 404, and a chat
/// thread or a body with no reason a 400.
#[tokio::test]
async fn pausing_names_what_it_refused() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let chat = uuid::Uuid::new_v4();
    seed_chat_thread_summary(&pool, chat, "idle").await;
    let client = user_client().await;
    let reason = serde_json::json!({ "reason": "checkpoint" });

    let resp = client
        .post(pause_url(uuid::Uuid::new_v4()))
        .json(&reason)
        .send()
        .await
        .expect("pause sends");
    assert_eq!(resp.status(), 404);

    let resp = client
        .post(pause_url(chat))
        .json(&reason)
        .send()
        .await
        .expect("pause sends");
    assert_eq!(resp.status(), 400);

    let resp = client
        .post(pause_url(chat))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("pause sends");
    assert_eq!(resp.status(), 400);
}

/// An unpause refuses what a pause refuses: an unknown thread is a 404, and a
/// chat thread a 400.
#[tokio::test]
async fn unpausing_names_what_it_refused() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let chat = uuid::Uuid::new_v4();
    seed_chat_thread_summary(&pool, chat, "idle").await;
    let client = user_client().await;

    let resp = client
        .delete(pause_url(uuid::Uuid::new_v4()))
        .send()
        .await
        .expect("unpause sends");
    assert_eq!(resp.status(), 404);

    let resp = client
        .delete(pause_url(chat))
        .send()
        .await
        .expect("unpause sends");
    assert_eq!(resp.status(), 400);
}

/// The CLI lists permissions and surfaces the batch route's refusal.
#[test]
fn cli_permissions_reach_the_routes() {
    let out = crate::lucidos_cli_test::lucidos_cmd()
        .args(["permissions", "list"])
        .output()
        .expect("lucidos permissions list runs");
    assert!(
        out.status.success(),
        "list succeeds, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = crate::lucidos_cli_test::lucidos_cmd()
        .args(["permissions", "batch", "--items", "[]"])
        .output()
        .expect("lucidos permissions batch runs");
    assert!(!out.status.success(), "a token-less batch is refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("403"),
        "the CLI must surface the engine's refusal, got: {stderr}"
    );
}
