//! Cascade archive API e2e tests.
//!
//! The pure-logic + DB-projection coverage of the cascade decision lives in
//! `crates/lucidos-engine/src/api/threads.rs::cascade_tests`. The point of
//! these e2e tests is to validate the WIRING: real HTTP → real EventBus →
//! real `thread_summaries` projection → real response shape, against a booted
//! e2e workspace.
//!
//! Setup pattern: seed `thread_summaries` rows directly (mirroring the
//! `seed_cc_thread_summary` and `drawer-family-collapse.spec.ts` precedents)
//! so each scenario is fast + deterministic, then POST `/api/v1/threads/archive`
//! and assert on the response + the events the EventBus writes. Real CC sub-
//! thread spawn would force an LLM round-trip we don't need to exercise.

use crate::support::{base_url, count_events_of_type, db_url, http_client};
use serde_json::json;
use uuid::Uuid;

/// Seed a row in `thread_summaries` for a parent or descendant thread,
/// covering every column the cascade gate consults. Cleanup is the caller's
/// responsibility via `cleanup_threads`.
#[allow(clippy::too_many_arguments)]
async fn seed_thread(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    parent_thread_id: Option<Uuid>,
    is_coding_agent: bool,
    status: &str,
    archive_state: &str,
    coding_agent_proposed: bool,
    coding_agent_is_external_repo: bool,
) {
    sqlx::query(
        "INSERT INTO thread_summaries (\
             thread_id, parent_thread_id, source, is_coding_agent, \
             created_at, last_activity, message_count, status, archive_state, \
             coding_agent_proposed, coding_agent_is_external_repo, \
             has_response, state \
         ) VALUES ($1, $2, $3, $4, NOW(), NOW(), 0, $5, $6, $7, $8, TRUE, 'active') \
         ON CONFLICT (thread_id) DO UPDATE SET \
             parent_thread_id = EXCLUDED.parent_thread_id, \
             source = EXCLUDED.source, \
             is_coding_agent = EXCLUDED.is_coding_agent, \
             status = EXCLUDED.status, \
             archive_state = EXCLUDED.archive_state, \
             coding_agent_proposed = EXCLUDED.coding_agent_proposed, \
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
    .bind(coding_agent_proposed)
    .bind(coding_agent_is_external_repo)
    .execute(pool)
    .await
    .expect("failed to seed thread_summaries row");
}

async fn cleanup_threads(pool: &sqlx::PgPool, ids: &[Uuid]) {
    // Best-effort cleanup so a hung test doesn't bleed rows into the next run.
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

/// Seed a `SessionStarted` event row directly (bypassing EventBus — these
/// tests own the projection via `seed_thread`). The branch is deliberately a
/// non-existent `claude-code/*` ref so that, if the archive path were to call
/// `end_stale_waiting_session`, its `proposal_files_for_branch` lookup fails
/// fast and the only observable side-effect is the synthetic settle
/// `CodingAgentIdled` it emits. That settle event is the canary the
/// regression test asserts on.
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

/// Cascade success: parent + idle CC sub-thread both get `ThreadArchived`.
#[tokio::test]
async fn archive_cascade_archives_idle_descendants() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Parent chat thread, idle, in inbox so the parent-gate admits Archive.
    seed_thread(&pool, parent_id, None, false, "idle", "inbox", false, false).await;
    // Idle CC sub-thread, inbox, no pending changes — fully cascadable.
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

    // EventBus emits run their own per-event transactions after the handler
    // releases the FOR UPDATE lock — give the projection a beat to settle.
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

    cleanup_threads(&pool, &[parent_id, child_id]).await;
    pool.close().await;
}

/// Idempotent re-archive: archiving an already-archived thread returns 200
/// with an empty `archived` list and emits NO new `ThreadArchived` event —
/// it is NOT a 409. This is the API-boundary guard for the stuck-Archive-button
/// bug: a client whose `meta.section` desynced to 'inbox' (missed SSE / failed
/// archive HTTP on a flaky PWA) re-POSTs archive; the old behaviour 409'd
/// (`parent_not_archivable`), which the client rolled back into the button
/// reappearing. Now the re-POST succeeds, so the client's optimistic
/// 'archived' flip stands and the button disappears.
#[tokio::test]
async fn archive_already_archived_thread_is_idempotent() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let thread_id = Uuid::new_v4();
    // Already-archived chat thread, idle, no descendants.
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

    // No ThreadArchived event should have been emitted (the row was seeded
    // directly without one, so the count must stay at zero).
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        count_thread_archived_events(&pool, thread_id).await,
        0,
        "idempotent re-archive must not emit a duplicate ThreadArchived event"
    );

    cleanup_threads(&pool, &[thread_id]).await;
    pool.close().await;
}

/// Cascade reject: a Running CC sub-thread blocks the parent's archive with
/// 409 `descendants_blocking`. NO `ThreadArchived` event lands.
#[tokio::test]
async fn archive_rejects_when_descendant_running() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    seed_thread(&pool, parent_id, None, false, "idle", "inbox", false, false).await;
    // Running CC sub-thread — the exact blocking combo per `is_blocking`.
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

    // Even if EventBus emits were lagging behind, no ThreadArchived should
    // ever land in a rejected cascade — the handler rolls back the
    // FOR UPDATE tx and returns before any emit loop.
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

/// Cascade reject: a CC sub-thread with pending changes (waiting + proposed)
/// blocks the parent's archive with 409 `descendants_blocking`.
#[tokio::test]
async fn archive_rejects_when_descendant_has_pending_changes() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    seed_thread(&pool, parent_id, None, false, "idle", "inbox", false, false).await;
    // CC sub-thread with a pending change: status=waiting,
    // coding_agent_proposed=true. is_external_repo=false so the carve-out for
    // external-repo pending CC doesn't bypass the blocking predicate.
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

/// Regression: archiving a family of stale-waiting CC threads (a
/// `SessionStarted` on a dead branch, no live subprocess) must NOT run
/// `end_stale_waiting_session`'s heavy worktree teardown. The tell that it
/// ran is a synthetic settle `CodingAgentIdled` emitted before
/// `ThreadArchived`.
///
/// The old archive loop called `stop_agent(Archive)` unconditionally; with no
/// live session it fell through to `end_stale_waiting_session`, which
/// auto-commits the worktree, `git worktree remove --force`s it, recomputes
/// the branch diff, and tries to propose a change — per descendant, serialized
/// inside the one HTTP request. A real 8-track family (one large refactor
/// worktree) took ~60s, timed the iOS PWA fetch out, left each child visibly
/// "stuck" until its slow settle landed, and provoked a retry that ran two
/// cascades over the same worktrees concurrently. Archive must only stop a
/// LIVE subprocess: `ThreadArchived` alone settles the projection and the
/// async worktree-cleanup worker GCs the worktree on its own schedule.
#[tokio::test]
async fn archive_does_not_settle_stale_cc_sessions() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Idle CC parent + idle CC child, both in inbox (fully cascadable), each
    // carrying a SessionStarted on a dead branch — the exact stale-waiting
    // shape a CC family is left in after an engine restart.
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

    // EventBus emits run their own per-event transactions after the handler
    // releases the FOR UPDATE lock — give the projection a beat to settle.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Both members must be archived…
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

    // …and NEITHER may have a settle CodingAgentIdled — its presence means the
    // archive ran end_stale_waiting_session's heavy teardown for a stale
    // session, the root cause of the multi-minute archive hang + retry race.
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
