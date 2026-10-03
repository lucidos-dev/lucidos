//! End-to-end API tests for Archive all and its Undo (ADR 0349).
//!
//! The decisions are unit-tested in the engine (`engine::thread_triage`). What
//! is tested here is the WIRING: real HTTP through the owner gate into the
//! Archive button's own cascade, and the unarchive that Undo calls.
//!
//! Setup mirrors `delete_thread_test.rs`: seed `thread_summaries` rows
//! directly. The workspace may hold other threads, so every assertion looks
//! only at the ids it seeded.

use crate::support::{base_url, db_url, http_client, user_client};
use serde_json::json;
use uuid::Uuid;

/// Seed one idle inbox thread. `set` adjusts the row after the insert.
async fn seed(pool: &sqlx::PgPool, id: Uuid, parent: Option<Uuid>, set: &str) {
    sqlx::query(
        "INSERT INTO thread_summaries (\
             thread_id, parent_thread_id, source, is_coding_agent, title, \
             created_at, last_activity, message_count, status, archive_state, \
             coding_agent_proposed, coding_agent_is_external_repo, \
             has_response, state \
         ) VALUES ($1, $2, 'chat', FALSE, 'archive all e2e', NOW(), NOW(), 2, 'idle', \
                   'inbox', FALSE, FALSE, TRUE, 'active')",
    )
    .bind(id)
    .bind(parent)
    .execute(pool)
    .await
    .expect("failed to seed thread_summaries row");
    if !set.is_empty() {
        sqlx::query(&format!(
            "UPDATE thread_summaries SET {set} WHERE thread_id = $1"
        ))
        .bind(id)
        .execute(pool)
        .await
        .expect("failed to adjust the seeded row");
    }
}

async fn section(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("read the section")
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

fn ids_in(body: &serde_json::Value, key: &str) -> Vec<String> {
    body[key]
        .as_array()
        .unwrap_or_else(|| panic!("response missing `{key}`: {body:?}"))
        .iter()
        .map(|v| {
            v.as_str()
                .or_else(|| v["thread_id"].as_str())
                .expect("an id")
                .to_string()
        })
        .collect()
}

/// The safety net end to end. The confirm lists the safe root and not the one
/// holding a draft. The press archives the root, leaves its pinned sub-thread
/// open, and keeps the drafted root. Undo puts the batch back.
#[tokio::test]
async fn archive_all_takes_only_what_is_safe_and_undo_restores_it() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let (root, child, pinned_child, drafted) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    seed(&pool, root, None, "").await;
    seed(&pool, child, Some(root), "").await;
    seed(&pool, pinned_child, Some(root), "is_saved = TRUE").await;
    seed(&pool, drafted, None, "compose_text = 'half a thought'").await;
    let all = [root, child, pinned_child, drafted];

    let preflight: serde_json::Value = client
        .get(format!(
            "{}/api/v1/threads/archive-all-preflight",
            base_url()
        ))
        .send()
        .await
        .expect("preflight request failed")
        .json()
        .await
        .expect("Invalid JSON");
    let safe: Vec<String> = preflight["safe"]
        .as_array()
        .unwrap_or_else(|| panic!("preflight missing `safe`: {preflight:?}"))
        .iter()
        .map(|t| t["thread_id"].as_str().unwrap().to_string())
        .collect();
    assert!(safe.contains(&root.to_string()), "{preflight:?}");
    assert!(
        !safe.contains(&drafted.to_string()),
        "a draft needs the user"
    );
    assert!(
        !safe.contains(&child.to_string()),
        "a sub-thread goes with its root"
    );

    let resp = client
        .post(format!("{}/api/v1/threads/archive-all", base_url()))
        .json(&json!({ "thread_ids": [root.to_string(), drafted.to_string()] }))
        .send()
        .await
        .expect("archive-all request failed");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(status, 200, "{body:?}");
    let archived = ids_in(&body, "archived");
    assert!(archived.contains(&root.to_string()) && archived.contains(&child.to_string()));
    assert!(!archived.contains(&pinned_child.to_string()));
    assert!(ids_in(&body, "kept").contains(&drafted.to_string()));
    assert_eq!(
        section(&pool, pinned_child).await,
        "inbox",
        "the pin is the user's"
    );
    assert_eq!(section(&pool, drafted).await, "inbox");

    let undo: serde_json::Value = client
        .post(format!("{}/api/v1/threads/unarchive", base_url()))
        .json(&json!({ "thread_ids": archived }))
        .send()
        .await
        .expect("unarchive request failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert_eq!(ids_in(&undo, "unarchived").len(), 2, "{undo:?}");
    for id in all {
        assert_eq!(section(&pool, id).await, "inbox", "{id} after Undo");
    }

    cleanup(&pool, &all).await;
    pool.close().await;
}

/// All three routes are the owner's buttons. A caller with no credential is
/// refused at the boundary and the thread stays where it was.
#[tokio::test]
async fn archive_all_refuses_an_unattributed_caller() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let id = Uuid::new_v4();
    seed(&pool, id, None, "").await;

    let preflight = http_client()
        .get(format!(
            "{}/api/v1/threads/archive-all-preflight",
            base_url()
        ))
        .send()
        .await
        .expect("preflight request failed")
        .status()
        .as_u16();
    for path in ["archive-all", "unarchive"] {
        let status = http_client()
            .post(format!("{}/api/v1/threads/{path}", base_url()))
            .json(&json!({ "thread_ids": [id.to_string()] }))
            .send()
            .await
            .expect("request failed")
            .status()
            .as_u16();
        assert!(status == 401 || status == 403, "{path} answered {status}");
    }
    assert!(
        preflight == 401 || preflight == 403,
        "preflight answered {preflight}"
    );
    assert_eq!(section(&pool, id).await, "inbox");

    cleanup(&pool, &[id]).await;
    pool.close().await;
}
