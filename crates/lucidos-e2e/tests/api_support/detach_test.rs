//! Coverage for `POST /api/v1/threads/:thread_id/detach`, moving a child
//! thread to top level (ADR 0278).
//!
//! This suite is the USER's path: a caller with a credential but no origin
//! token may move any nested thread. The agent's narrower rule needs a
//! thread-bound origin token, and an outside HTTP client cannot get one (see
//! `follow_up_test.rs`). The engine's tests over `authorize_child_detach`
//! cover that rule instead.
//!
//! Rows are seeded directly, as `cascade_archive_test.rs` does, so no LLM
//! round-trip is needed to build a family.

use crate::support::{base_url, count_events_of_type, db_url, http_client, user_client};
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

async fn post_detach(client: &reqwest::Client, thread: &str) -> reqwest::Response {
    client
        .post(format!("{}/api/v1/threads/{thread}/detach", base_url()))
        .send()
        .await
        .expect("detach request sends")
}

/// The user moves a grandchild of a top-level thread. The edge is cut on the
/// row, and the move is recorded once, on the former parent.
#[tokio::test]
async fn the_user_moves_a_nested_thread_to_top_level() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let (root, parent, child) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    seed(&pool, root, None, 0).await;
    seed(&pool, parent, Some(root), 1).await;
    seed(&pool, child, Some(parent), 2).await;

    let resp = post_detach(&client, &child.to_string()).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json ack");
    assert_eq!(body["child_thread_id"], child.to_string());
    assert_eq!(body["former_parent_thread_id"], parent.to_string());

    let (moved_parent, depth): (Option<Uuid>, i32) =
        sqlx::query_as("SELECT parent_thread_id, depth FROM thread_summaries WHERE thread_id = $1")
            .bind(child)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(moved_parent, None);
    assert_eq!(depth, 0);
    assert_eq!(
        count_events_of_type(&pool, parent, "ChildThreadDetached").await,
        1
    );
    assert_eq!(
        count_events_of_type(&pool, child, "ChildThreadDetached").await,
        0,
        "the move lands on the former parent, never on the child"
    );

    // A second move finds nothing to cut.
    assert_eq!(post_detach(&client, &child.to_string()).await.status(), 409);

    cleanup(&pool, &[root, parent, child]).await;
}

#[tokio::test]
async fn the_route_refuses_what_it_cannot_move() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let top = Uuid::new_v4();
    seed(&pool, top, None, 0).await;

    let resp = post_detach(&client, &top.to_string()).await;
    assert_eq!(resp.status(), 409, "a top-level thread is not a child");
    let body: serde_json::Value = resp.json().await.expect("standard error body");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|m| m.contains("already at top level")),
        "{body}"
    );

    assert_eq!(
        post_detach(&client, &Uuid::new_v4().to_string())
            .await
            .status(),
        404
    );
    assert_eq!(post_detach(&client, "not-a-uuid").await.status(), 400);

    let get = client
        .get(format!("{}/api/v1/threads/{top}/detach", base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 405, "the move is POST-only");

    cleanup(&pool, &[top]).await;
}

/// A caller presenting no credential at all is refused before anything is
/// read, like every mutating route (ADR 0169).
#[tokio::test]
async fn the_route_refuses_a_caller_with_no_credential() {
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect db");
    let (parent, child) = (Uuid::new_v4(), Uuid::new_v4());
    seed(&pool, parent, None, 0).await;
    seed(&pool, child, Some(parent), 1).await;

    let resp = post_detach(&http_client(), &child.to_string()).await;
    assert_eq!(resp.status(), 401);
    let still: Option<Uuid> =
        sqlx::query_scalar("SELECT parent_thread_id FROM thread_summaries WHERE thread_id = $1")
            .bind(child)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(still, Some(parent));

    cleanup(&pool, &[parent, child]).await;
}
