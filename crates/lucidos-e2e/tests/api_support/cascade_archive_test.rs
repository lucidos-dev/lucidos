use crate::support::{base_url, count_events_of_type, db_url, user_client};
use serde_json::json;
use uuid::Uuid;

#[allow(clippy::too_many_arguments)]
async fn seed_thread(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    parent_thread_id: Option<Uuid>,
    is_coding_agent: bool,
    status: &str,
    archive_state: &str,
    proposed: bool,
    coding_agent_is_external_repo: bool,
) {
    sqlx::query(
        "INSERT INTO thread_summaries (\
             thread_id, parent_thread_id, source, is_coding_agent, \
             created_at, last_activity, message_count, status, archive_state, \
             coding_agent_change_state, coding_agent_is_external_repo, \
             has_response, state \
         ) VALUES ($1, $2, $3, $4, NOW(), NOW(), 0, $5, $6, $7, $8, TRUE, 'active') \
         ON CONFLICT (thread_id) DO UPDATE SET \
             parent_thread_id = EXCLUDED.parent_thread_id, \
             source = EXCLUDED.source, \
             is_coding_agent = EXCLUDED.is_coding_agent, \
             status = EXCLUDED.status, \
             archive_state = EXCLUDED.archive_state, \
             coding_agent_change_state = EXCLUDED.coding_agent_change_state, \
             coding_agent_is_external_repo = EXCLUDED.coding_agent_is_external_repo",
    )
    .bind(thread_id)
    .bind(parent_thread_id)
    .bind(if is_coding_agent {
        "claude_code"
    } else {
        "chat"
    })
    .bind(is_coding_agent)
    .bind(status)
    .bind(archive_state)
    .bind(if proposed { "proposed" } else { "none" })
    .bind(coding_agent_is_external_repo)
    .execute(pool)
    .await
    .expect("failed to seed thread_summaries row");
}

async fn cleanup_threads(pool: &sqlx::PgPool, ids: &[Uuid]) {
    let _ = sqlx::query("DELETE FROM events WHERE thread_id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM thread_summaries WHERE thread_id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await;
}

async fn count_thread_archived_events(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    count_events_of_type(pool, thread_id, "ThreadArchived").await
}

async fn seed_session_started(pool: &sqlx::PgPool, thread_id: Uuid, branch: &str) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, thread_id, aggregate, aggregate_id) \
         VALUES ($1, 'SessionStarted', $2, NOW(), $3, 'thread', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(json!({ "branch": branch, "session_id": "", "channel": "claude_code" }))
    .bind(thread_id)
    .bind(thread_id.to_string())
    .execute(pool)
    .await
    .expect("failed to seed SessionStarted event");
}

#[tokio::test]
async fn archive_cascade_archives_idle_descendants() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    seed_thread(&pool, parent_id, None, false, "idle", "inbox", false, false).await;
    seed_thread(
        &pool,
        child_id,
        Some(parent_id),
        true,
        "idle",
        "inbox",
        false,
        false,
    )
    .await;

    let url = format!("{}/api/v1/threads/archive", base_url());
    let resp = client
        .post(&url)
        .json(&json!({ "thread_id": parent_id.to_string() }))
        .send()
        .await
        .expect("archive request failed");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");

    assert_eq!(
        status, 200,
        "cascade archive of idle family must succeed: body={body:?}"
    );
    let archived = body["archived"]
        .as_array()
        .unwrap_or_else(|| panic!("response missing `archived` array: {body:?}"));
    let ids: Vec<String> = archived
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(
        ids.contains(&parent_id.to_string()),
        "parent {} missing from archived list {:?}",
        parent_id,
        ids,
    );
    assert!(
        ids.contains(&child_id.to_string()),
        "child {} missing from archived list {:?}",
        child_id,
        ids,
    );
    assert_eq!(
        body["skipped"],
        json!([]),
        "an archive that left nobody behind reports no skips: {body:?}"
    );

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert_eq!(
        count_thread_archived_events(&pool, parent_id).await,
        1,
        "parent must have exactly one ThreadArchived event"
    );
    assert_eq!(
        count_thread_archived_events(&pool, child_id).await,
        1,
        "child must have exactly one ThreadArchived event"
    );

    for id in [parent_id, child_id] {
        let reported = body["summary_versions"][id.to_string()]
            .as_i64()
            .unwrap_or_else(|| panic!("no summary version for {id}: {body:?}"));
        let stored: i64 =
            sqlx::query_scalar("SELECT summary_version FROM thread_summaries WHERE thread_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .expect("read summary_version");
        assert!(
            reported > 0 && reported <= stored,
            "{id}: reported {reported}, stored {stored}"
        );
    }

    cleanup_threads(&pool, &[parent_id, child_id]).await;
    pool.close().await;
}

#[tokio::test]
async fn archive_already_archived_thread_is_idempotent() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    seed_thread(
        &pool, thread_id, None, false, "idle", "archived", false, false,
    )
    .await;

    let url = format!("{}/api/v1/threads/archive", base_url());
    let resp = client
        .post(&url)
        .json(&json!({ "thread_id": thread_id.to_string() }))
        .send()
        .await
        .expect("archive request failed");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");

    assert_eq!(
        status, 200,
        "re-archiving an already-archived thread must be idempotent success, not 409: body={body:?}"
    );
    let archived = body["archived"]
        .as_array()
        .unwrap_or_else(|| panic!("response missing `archived` array: {body:?}"));
    assert!(
        archived.is_empty(),
        "already-archived thread has nothing to re-emit: {archived:?}"
    );

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        count_thread_archived_events(&pool, thread_id).await,
        0,
        "idempotent re-archive must not emit a duplicate ThreadArchived event"
    );

    cleanup_threads(&pool, &[thread_id]).await;
    pool.close().await;
}

#[tokio::test]
async fn archive_rejects_when_descendant_running() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    seed_thread(&pool, parent_id, None, false, "idle", "inbox", false, false).await;
    seed_thread(
        &pool,
        child_id,
        Some(parent_id),
        true,
        "running",
        "inbox",
        false,
        false,
    )
    .await;

    let url = format!("{}/api/v1/threads/archive", base_url());
    let resp = client
        .post(&url)
        .json(&json!({ "thread_id": parent_id.to_string() }))
        .send()
        .await
        .expect("archive request failed");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");

    assert_eq!(
        status, 409,
        "running descendant must block parent archive: body={body:?}"
    );
    assert_eq!(
        body["reason"], "descendants_blocking",
        "rejection reason must be descendants_blocking: {body:?}"
    );
    let blocking = body["blocking"]
        .as_array()
        .unwrap_or_else(|| panic!("response missing `blocking` array: {body:?}"));
    assert!(
        blocking
            .iter()
            .any(|b| b["thread_id"].as_str() == Some(&child_id.to_string())),
        "blocking list must name the running child {}: {blocking:?}",
        child_id,
    );

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        count_thread_archived_events(&pool, parent_id).await,
        0,
        "rejected cascade must NOT emit ThreadArchived for parent"
    );
    assert_eq!(
        count_thread_archived_events(&pool, child_id).await,
        0,
        "rejected cascade must NOT emit ThreadArchived for child"
    );

    cleanup_threads(&pool, &[parent_id, child_id]).await;
    pool.close().await;
}

#[tokio::test]
async fn archive_rejects_when_descendant_has_pending_changes() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    seed_thread(&pool, parent_id, None, false, "idle", "inbox", false, false).await;
    seed_thread(
        &pool,
        child_id,
        Some(parent_id),
        true,
        "waiting",
        "inbox",
        true,
        false,
    )
    .await;

    let url = format!("{}/api/v1/threads/archive", base_url());
    let resp = client
        .post(&url)
        .json(&json!({ "thread_id": parent_id.to_string() }))
        .send()
        .await
        .expect("archive request failed");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");

    assert_eq!(
        status, 409,
        "pending-changes descendant must block parent archive: body={body:?}"
    );
    assert_eq!(
        body["reason"], "descendants_blocking",
        "rejection reason must be descendants_blocking: {body:?}"
    );
    let blocking = body["blocking"]
        .as_array()
        .unwrap_or_else(|| panic!("response missing `blocking` array: {body:?}"));
    let entry = blocking
        .iter()
        .find(|b| b["thread_id"].as_str() == Some(&child_id.to_string()))
        .unwrap_or_else(|| panic!("blocking list must name CC child {child_id}: {blocking:?}"));
    assert_eq!(
        entry["has_pending_changes"], true,
        "blocking entry must surface has_pending_changes=true: {entry:?}"
    );

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        count_thread_archived_events(&pool, parent_id).await,
        0,
        "rejected cascade must NOT emit ThreadArchived for parent"
    );
    assert_eq!(
        count_thread_archived_events(&pool, child_id).await,
        0,
        "rejected cascade must NOT emit ThreadArchived for child"
    );

    cleanup_threads(&pool, &[parent_id, child_id]).await;
    pool.close().await;
}

#[tokio::test]
async fn archive_does_not_settle_stale_cc_sessions() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    seed_thread(&pool, parent_id, None, true, "idle", "inbox", false, false).await;
    seed_thread(
        &pool,
        child_id,
        Some(parent_id),
        true,
        "idle",
        "inbox",
        false,
        false,
    )
    .await;
    seed_session_started(&pool, parent_id, &format!("claude-code/stale-{parent_id}")).await;
    seed_session_started(&pool, child_id, &format!("claude-code/stale-{child_id}")).await;

    let url = format!("{}/api/v1/threads/archive", base_url());
    let resp = client
        .post(&url)
        .json(&json!({ "thread_id": parent_id.to_string() }))
        .send()
        .await
        .expect("archive request failed");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");

    assert_eq!(
        status, 200,
        "cascade archive of stale-waiting CC family must succeed: body={body:?}"
    );

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert_eq!(
        count_thread_archived_events(&pool, parent_id).await,
        1,
        "parent must have exactly one ThreadArchived event"
    );
    assert_eq!(
        count_thread_archived_events(&pool, child_id).await,
        1,
        "child must have exactly one ThreadArchived event"
    );

    assert_eq!(
        count_events_of_type(&pool, parent_id, "CodingAgentIdled").await,
        0,
        "archive must not settle the parent's stale CC session"
    );
    assert_eq!(
        count_events_of_type(&pool, child_id, "CodingAgentIdled").await,
        0,
        "archive must not settle the child's stale CC session"
    );

    cleanup_threads(&pool, &[parent_id, child_id]).await;
    pool.close().await;
}

async fn mark_stopped_child(pool: &sqlx::PgPool, thread_id: Uuid) {
    sqlx::query(
        "UPDATE thread_summaries SET is_stopped_child = TRUE, parent_callback_pending = TRUE \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("failed to mark the stopped child");
}

async fn completion_statuses(pool: &sqlx::PgPool, parent_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT payload->>'status' FROM events \
         WHERE thread_id = $1 AND event_type = 'ChildThreadCompleted' ORDER BY sequence",
    )
    .bind(parent_id)
    .fetch_all(pool)
    .await
    .expect("failed to read completion cards")
}

async fn post_archive(thread_id: Uuid) -> u16 {
    let client = user_client().await;
    client
        .post(format!("{}/api/v1/threads/archive", base_url()))
        .json(&json!({ "thread_id": thread_id.to_string() }))
        .send()
        .await
        .expect("archive request failed")
        .status()
        .as_u16()
}

#[tokio::test]
async fn archiving_a_stopped_child_sends_its_parent_a_canceled_card() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    seed_thread(&pool, parent_id, None, false, "idle", "inbox", false, false).await;
    seed_thread(
        &pool,
        child_id,
        Some(parent_id),
        true,
        "idle",
        "inbox",
        false,
        false,
    )
    .await;
    mark_stopped_child(&pool, child_id).await;

    assert_eq!(post_archive(child_id).await, 200);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert_eq!(
        completion_statuses(&pool, parent_id).await,
        vec!["canceled".to_string()],
        "the parent must get exactly one canceled card"
    );

    cleanup_threads(&pool, &[parent_id, child_id]).await;
    pool.close().await;
}

#[tokio::test]
async fn a_cascade_from_the_parent_sends_no_card_for_a_stopped_child() {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    seed_thread(&pool, parent_id, None, false, "idle", "inbox", false, false).await;
    seed_thread(
        &pool,
        child_id,
        Some(parent_id),
        true,
        "idle",
        "inbox",
        false,
        false,
    )
    .await;
    mark_stopped_child(&pool, child_id).await;

    assert_eq!(
        post_archive(parent_id).await,
        200,
        "a stopped child must not block archiving its parent"
    );
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert!(
        completion_statuses(&pool, parent_id).await.is_empty(),
        "a cascade must send the archived parent no card"
    );
    let parent_section: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(parent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(parent_section, "archived");

    cleanup_threads(&pool, &[parent_id, child_id]).await;
    pool.close().await;
}
