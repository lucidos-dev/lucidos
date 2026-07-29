use super::*;
use super::cp_helpers::*;

/// Simulates the historical write-through gap: events exist in the store
/// but their `changes` row was never written. `rebuild_missing_from_events`
/// must replay the lifecycle into a single row with the correct terminal
/// status, original `created_at`, and event-time `resolved_at`.
#[tokio::test]
async fn rebuild_missing_from_events_recovers_applied_change() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-rebuild", "/r"),
    )
    .await;
    emit(
        &bus,
        thread,
        applied_event(change_id, &["feat: x", "fix: y"], true),
    )
    .await;

    // Simulate the historical projection gap: drop the row, keep the events.
    sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await
        .unwrap();
    let proj = ChangesProjection::new(pool.clone());
    assert!(
        proj.get_by_id(change_id).await.unwrap().is_none(),
        "row gone before rebuild"
    );

    let n = proj.rebuild_missing_from_events().await.unwrap();
    assert_eq!(n, 1, "one row recovered");

    let row = proj.get_by_id(change_id).await.unwrap().expect("rebuilt row");
    assert_eq!(row.status, "applied");
    assert_eq!(row.branch_name, "branch-rebuild");
    assert_eq!(
        row.commits,
        vec!["feat: x".to_string(), "fix: y".to_string()]
    );
    assert!(row.requires_restart);
    assert!(row.resolved_at.is_some());
    assert!(row.hardened, "aggregate_proposed seeds hardened=true");

    // Idempotent: a second rebuild finds nothing missing.
    assert_eq!(proj.rebuild_missing_from_events().await.unwrap(), 0);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn rebuild_missing_from_events_recovers_discarded_change() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-d", "/r"),
    )
    .await;
    emit(&bus, thread, discarded_event(change_id)).await;

    sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await
        .unwrap();
    let proj = ChangesProjection::new(pool.clone());

    assert_eq!(proj.rebuild_missing_from_events().await.unwrap(), 1);
    let row = proj.get_by_id(change_id).await.unwrap().expect("rebuilt row");
    assert_eq!(row.status, "discarded");
    assert!(row.resolved_at.is_some());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn rebuild_missing_from_events_uses_latest_proposed_payload() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-l", "/r"),
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: Some("latest desc".to_string()),
            files: vec!["x.rs".to_string(), "y.rs".to_string(), "z.rs".to_string()],
            requires_restart: true,
            origin: None,
            commit_sha: None,
            branch_name: "branch-l".to_string(),
            repo_root: "/r".to_string(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
    )
    .await;

    sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await
        .unwrap();
    let proj = ChangesProjection::new(pool.clone());

    proj.rebuild_missing_from_events().await.unwrap();
    let row = proj.get_by_id(change_id).await.unwrap().expect("rebuilt row");
    assert_eq!(row.description, "latest desc", "latest proposed wins");
    assert_eq!(row.file_count, 3);
    assert!(row.requires_restart);
    assert!(
        !row.hardened,
        "latest hardened=false stays false absent ChangeHardened"
    );
    assert_eq!(row.status, "pending", "no terminal event → pending");

    teardown_test_db(&db).await;
}

/// `ChangeHardened` after the latest `ChangeProposed(hardened=false)` must
/// flip `hardened` to true on the rebuilt row.
#[tokio::test]
async fn rebuild_missing_from_events_applies_change_hardened_after_proposed() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: Some("d".to_string()),
            files: vec!["a.rs".to_string()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: "branch-h2".to_string(),
            repo_root: "/r".to_string(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeHardened {
            change_id: change_id.to_string(),
            actor: None,
        },
    )
    .await;

    sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await
        .unwrap();
    let proj = ChangesProjection::new(pool.clone());

    proj.rebuild_missing_from_events().await.unwrap();
    let row = proj.get_by_id(change_id).await.unwrap().expect("rebuilt row");
    assert!(
        row.hardened,
        "ChangeHardened after ChangeProposed flips hardened on"
    );
    assert_eq!(row.status, "pending");

    teardown_test_db(&db).await;
}

/// A change recovered with an active merge worktree must surface in
/// `with_merge_worktree()` so startup cleanup can reach it.
#[tokio::test]
async fn rebuild_missing_from_events_carries_active_merge_worktree() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-mw", "/r"),
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::MergeResolutionStarted {
            change_id: change_id.to_string(),
            worktree_path: "/tmp/wt-rebuild".to_string(),
            temp_branch: "merge-tmp/rebuild".to_string(),
        },
    )
    .await;

    sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await
        .unwrap();
    let proj = ChangesProjection::new(pool.clone());

    proj.rebuild_missing_from_events().await.unwrap();
    let row = proj.get_by_id(change_id).await.unwrap().expect("rebuilt row");
    assert_eq!(row.merge_worktree_path.as_deref(), Some("/tmp/wt-rebuild"));
    assert_eq!(row.merge_temp_branch.as_deref(), Some("merge-tmp/rebuild"));
    assert!(proj
        .with_merge_worktree()
        .await
        .unwrap()
        .iter()
        .any(|c| c.id == change_id));

    teardown_test_db(&db).await;
}

/// MergeResolutionCleared after Started must clear the worktree fields,
/// so a recovered row doesn't claim a worktree that's already gone.
#[tokio::test]
async fn rebuild_missing_from_events_clears_resolved_merge_worktree() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-mc", "/r"),
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::MergeResolutionStarted {
            change_id: change_id.to_string(),
            worktree_path: "/tmp/wt-c".to_string(),
            temp_branch: "merge-tmp/c".to_string(),
        },
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::MergeResolutionCleared {
            change_id: change_id.to_string(),
        },
    )
    .await;

    sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await
        .unwrap();
    let proj = ChangesProjection::new(pool.clone());

    proj.rebuild_missing_from_events().await.unwrap();
    let row = proj.get_by_id(change_id).await.unwrap().expect("rebuilt row");
    assert!(row.merge_worktree_path.is_none());
    assert!(row.merge_temp_branch.is_none());

    teardown_test_db(&db).await;
}

/// Pins the "forward find above guarantees at least one aggregate
/// ChangeProposed row" invariant in `rebuild_one_from_events`: with a single
/// aggregate ChangeProposed (so forward and reverse both hit the same row),
/// rebuild must NOT panic. Pre-harden the `.unwrap()` on `.rev().find()`
/// implicitly relied on this; the `.expect("…guarantees…")` is now the
/// documented contract. The test would panic at the expect message if anyone
/// ever broke the chain by removing the forward-find early return.
#[tokio::test]
async fn rebuild_one_from_events_does_not_panic_when_only_aggregate_proposed_exists() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-single", "/r"),
    )
    .await;

    sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await
        .unwrap();
    let proj = ChangesProjection::new(pool.clone());

    let recovered = proj.rebuild_missing_from_events().await.unwrap();
    assert_eq!(recovered, 1, "single aggregate ChangeProposed → one rebuilt row");
    let row = proj
        .get_by_id(change_id)
        .await
        .unwrap()
        .expect("rebuilt row present");
    assert_eq!(row.branch_name, "branch-single");
    assert_eq!(row.status, "pending");

    teardown_test_db(&db).await;
}

/// Healthy projection: every change_id in events already has a row.
/// rebuild_missing_from_events must recover nothing — no spurious work.
#[tokio::test]
async fn rebuild_missing_from_events_is_noop_when_projection_healthy() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-h", "/r"),
    )
    .await;
    emit(&bus, thread, applied_event(change_id, &["c"], false)).await;

    let proj = ChangesProjection::new(pool.clone());
    assert_eq!(proj.rebuild_missing_from_events().await.unwrap(), 0);

    teardown_test_db(&db).await;
}

/// External repo CC threads must not have rows in the changes table — the
/// runtime gates (`may_touch_change_state_at_idle`, the SessionEnded
/// cleanup path) all skip propose_change for external repos, and the
/// cleanup migration removes any rows that survived from before those
/// gates were added. This test pins the SQL pattern itself: run the same
/// DELETE the migration runs, and verify external-repo rows are removed
/// while internal-repo rows are preserved.
#[tokio::test]
async fn cleanup_sql_drops_external_repo_changes_keeps_internal() {
    let (pool, db) = setup_test_db().await;

    let internal_thread = Uuid::new_v4();
    let external_thread = Uuid::new_v4();
    let internal_change = Uuid::new_v4();
    let external_change = Uuid::new_v4();

    sqlx::query(
            "INSERT INTO thread_summaries \
                (thread_id, source, is_coding_agent, created_at, last_activity, message_count, status, coding_agent_is_external_repo) \
             VALUES \
                ($1, 'claude_code', TRUE, NOW(), NOW(), 0, 'idle', FALSE), \
                ($2, 'claude_code', TRUE, NOW(), NOW(), 0, 'idle', TRUE)",
        )
        .bind(internal_thread)
        .bind(external_thread)
        .execute(&pool)
        .await
        .unwrap();

    for (cid, tid, branch) in [
        (internal_change, internal_thread, "claude-code/internal"),
        (external_change, external_thread, "feature/ticket-123"),
    ] {
        sqlx::query(
                "INSERT INTO changes \
                    (id, request_id, thread_id, branch_name, repo_root, description, file_count, files, requires_restart, status, created_at) \
                 VALUES ($1, $2, $3, $4, '/r', '', 0, '{}'::text[], FALSE, 'pending', NOW())",
            )
            .bind(cid)
            .bind(Uuid::nil())
            .bind(tid)
            .bind(branch)
            .execute(&pool)
            .await
            .unwrap();
    }

    // Mirrors the cleanup migration: keep both in sync if either changes.
    sqlx::query(
        "DELETE FROM changes WHERE thread_id IN \
                (SELECT thread_id FROM thread_summaries WHERE coding_agent_is_external_repo = true)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let remaining: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM changes ORDER BY branch_name")
        .fetch_all(&pool)
        .await
        .unwrap();

    assert_eq!(
        remaining,
        vec![internal_change],
        "external-repo change must be deleted; internal-repo change must survive"
    );

    teardown_test_db(&db).await;
}
