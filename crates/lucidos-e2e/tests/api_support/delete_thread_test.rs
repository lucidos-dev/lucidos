//! End-to-end API tests for deleting a thread.
//!
//! The transaction itself is covered in
//! `crates/lucidos-engine/src/api/threads/delete_tests.rs`, and the caller gate
//! in `api/actor_tests.rs`, where an origin token can be minted. What is tested
//! here is the WIRING: real HTTP into a booted workspace, through the owner
//! gate, the shared cascade classifier and the one destructive transaction.
//!
//! Two properties an external client is uniquely placed to prove. A caller with
//! no credential really is refused at the boundary rather than somewhere
//! deeper. And the cascade reaches a grandchild over real HTTP, not only in a
//! hand-built family snapshot.
//!
//! Setup mirrors `cascade_archive_test.rs`: seed `thread_summaries` rows
//! directly, so each scenario is fast and deterministic.

use crate::support::{base_url, db_url, http_client, user_client};
use serde_json::json;
use uuid::Uuid;

/// Seed one family member, covering every column the cascade gate consults.
async fn seed_thread(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    parent_thread_id: Option<Uuid>,
    status: &str,
) {
    sqlx::query(
        "INSERT INTO thread_summaries (\
             thread_id, parent_thread_id, source, is_coding_agent, title, \
             created_at, last_activity, message_count, status, archive_state, \
             coding_agent_proposed, coding_agent_is_external_repo, \
             has_response, state \
         ) VALUES ($1, $2, 'chat', FALSE, 'delete e2e', NOW(), NOW(), 0, $3, 'inbox', \
                   FALSE, FALSE, TRUE, 'active')",
    )
    .bind(thread_id)
    .bind(parent_thread_id)
    .bind(status)
    .execute(pool)
    .await
    .expect("failed to seed thread_summaries row");

    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, thread_id, aggregate, aggregate_id) \
         VALUES ($1, 'MessageReceived', $2, NOW(), $3, 'thread', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(json!({ "text": "something to delete" }))
    .bind(thread_id)
    .bind(thread_id.to_string())
    .execute(pool)
    .await
    .expect("failed to seed MessageReceived event");
}

async fn rows_left(pool: &sqlx::PgPool, table: &str, ids: &[Uuid]) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!(
        "SELECT COUNT(*) FROM {table} WHERE thread_id = ANY($1)"
    ))
    .bind(ids)
    .fetch_one(pool)
    .await
    .expect("count failed")
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

/// Three generations, deleted whole. Stop at the children and the grandchildren
/// point at a parent that is gone. The design plan rejected that shape outright.
#[tokio::test]
async fn delete_cascades_over_three_generations() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let grandparent = Uuid::new_v4();
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    seed_thread(&pool, grandparent, None, "idle").await;
    seed_thread(&pool, parent, Some(grandparent), "idle").await;
    seed_thread(&pool, child, Some(parent), "idle").await;
    let family = [grandparent, parent, child];

    let resp = client
        .post(format!("{}/api/v1/threads/delete", base_url()))
        .json(&json!({ "thread_id": grandparent.to_string() }))
        .send()
        .await
        .expect("delete request failed");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(
        status, 200,
        "deleting an idle family must succeed: {body:?}"
    );

    let deleted: Vec<String> = body["deleted"]
        .as_array()
        .unwrap_or_else(|| panic!("response missing `deleted`: {body:?}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    for id in family {
        assert!(
            deleted.contains(&id.to_string()),
            "{id} missing from the deleted list {deleted:?}"
        );
    }

    assert_eq!(rows_left(&pool, "thread_summaries", &family).await, 0);
    assert_eq!(rows_left(&pool, "events", &family).await, 0);

    // The audit record survives on the `ops` aggregate, naming the ids and
    // nothing that was said.
    let payload: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM events \
         WHERE event_type = 'ThreadsDeleted' AND aggregate = 'ops' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .fetch_optional(&pool)
    .await
    .expect("read the audit record");
    let payload = payload.expect("a delete must leave exactly one record behind");
    let text = payload.to_string();
    assert!(text.contains(&grandparent.to_string()), "{text}");
    assert!(
        !text.contains("something to delete"),
        "the record must carry no content from the thread: {text}"
    );

    cleanup(&pool, &family).await;
    pool.close().await;
}

/// The security boundary, from outside. An API caller presenting no credential
/// at all is refused before anything is read, and the thread is still there
/// afterwards.
///
/// The agent-origin-token half of the same rule is tested in the engine, where
/// the per-startup secret can be installed and a token minted. An external
/// client cannot mint one, which is itself the point.
#[tokio::test]
async fn delete_refuses_an_unattributed_caller() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    seed_thread(&pool, thread_id, None, "idle").await;

    // `http_client` sends no device header, which is what a bare curl is.
    let resp = http_client()
        .post(format!("{}/api/v1/threads/delete", base_url()))
        .json(&json!({ "thread_id": thread_id.to_string() }))
        .send()
        .await
        .expect("delete request failed");
    let status = resp.status().as_u16();
    assert!(
        status == 401 || status == 403,
        "an unattributed caller must be refused, got {status}"
    );

    assert_eq!(
        rows_left(&pool, "thread_summaries", &[thread_id]).await,
        1,
        "a refused delete must leave the thread alone"
    );

    // The preflight leaks sub-thread titles, so it is gated the same way.
    let preflight = http_client()
        .get(format!(
            "{}/api/v1/threads/delete-preflight?thread_id={}",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("preflight request failed");
    let preflight_status = preflight.status().as_u16();
    assert!(
        preflight_status == 401 || preflight_status == 403,
        "the preflight must refuse the same caller, got {preflight_status}"
    );

    cleanup(&pool, &[thread_id]).await;
    pool.close().await;
}

/// The refusal archive and delete share, over real HTTP. A running descendant
/// blocks the whole family, and nothing is removed.
#[tokio::test]
async fn delete_refuses_a_running_descendant() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    seed_thread(&pool, parent, None, "idle").await;
    seed_thread(&pool, child, Some(parent), "running").await;
    let family = [parent, child];

    let resp = client
        .post(format!("{}/api/v1/threads/delete", base_url()))
        .json(&json!({ "thread_id": parent.to_string() }))
        .send()
        .await
        .expect("delete request failed");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");

    assert_eq!(status, 409, "a running descendant blocks delete: {body:?}");
    assert_eq!(body["reason"], "descendants_blocking", "{body:?}");
    let blocking = body["blocking"]
        .as_array()
        .unwrap_or_else(|| panic!("response missing `blocking`: {body:?}"));
    assert!(
        blocking
            .iter()
            .any(|b| b["thread_id"].as_str() == Some(&child.to_string())),
        "the refusal must name the running child: {blocking:?}"
    );

    assert_eq!(
        rows_left(&pool, "thread_summaries", &family).await,
        2,
        "a refused cascade must delete nothing"
    );
    assert_eq!(rows_left(&pool, "events", &family).await, 2);

    cleanup(&pool, &family).await;
    pool.close().await;
}

/// A parent parked on a question needs the user, so delete and archive both
/// refuse it (ADR 0259).
#[tokio::test]
async fn delete_refuses_a_parent_waiting_on_an_answer() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    seed_thread(&pool, thread_id, None, "waiting_for_user_answer").await;

    let resp = client
        .post(format!("{}/api/v1/threads/delete", base_url()))
        .json(&json!({ "thread_id": thread_id.to_string() }))
        .send()
        .await
        .expect("delete request failed");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");

    assert_eq!(status, 409, "a parked parent blocks delete: {body:?}");
    assert_eq!(body["reason"], "parent_not_deletable", "{body:?}");
    assert_eq!(
        rows_left(&pool, "thread_summaries", &[thread_id]).await,
        1,
        "a refused delete must leave the thread alone"
    );

    let archived = client
        .post(format!("{}/api/v1/threads/archive", base_url()))
        .json(&json!({ "thread_id": thread_id.to_string() }))
        .send()
        .await
        .expect("archive request failed");
    let archive_status = archived.status().as_u16();
    let archive_body: serde_json::Value = archived.json().await.expect("Invalid JSON");
    assert_eq!(archive_status, 409, "a parked parent blocks archive too");
    assert_eq!(
        archive_body["reason"], "parent_not_archivable",
        "{archive_body:?}"
    );

    cleanup(&pool, &[thread_id]).await;
    pool.close().await;
}
