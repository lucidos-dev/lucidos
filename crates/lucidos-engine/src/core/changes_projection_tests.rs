use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

fn aggregate_proposed(change_id: Uuid, branch: &str, repo_root: &str) -> ThreadEvent {
    ThreadEvent::ChangeProposed {
        change_id: change_id.to_string(),
        description: Some("aggregate description".to_string()),
        files: vec!["a.rs".to_string(), "b.rs".to_string()],
        requires_restart: false,
        origin: None,
        commit_sha: None,
        branch_name: branch.to_string(),
        repo_root: repo_root.to_string(),
        hardened: true,
        incomplete: false,
        path: String::new(),
        diff: String::new(),
    }
}

fn per_commit_proposed(
    branch: &str,
    sha: &str,
    subject: &str,
    requires_restart: bool,
) -> ThreadEvent {
    ThreadEvent::ChangeProposed {
        change_id: String::new(),
        description: Some(subject.to_string()),
        files: vec!["c.rs".to_string()],
        requires_restart,
        origin: None,
        commit_sha: Some(sha.to_string()),
        branch_name: branch.to_string(),
        repo_root: String::new(),
        hardened: false,
        incomplete: false,
        path: String::new(),
        diff: String::new(),
    }
}

fn applied_event(change_id: Uuid, commits: &[&str], requires_restart: bool) -> ThreadEvent {
    ThreadEvent::ChangeApplied {
        change_id: change_id.to_string(),
        requires_restart,
        client_update: false,
        commits: commits.iter().map(|s| s.to_string()).collect(),
        thread_title: None,
        actor: None,
        pre_merge_sha: None,
        post_merge_sha: None,
        path: String::new(),
    }
}

fn discarded_event(change_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChangeDiscarded {
        change_id: change_id.to_string(),
        actor: None,
        path: String::new(),
    }
}

async fn start_cc_thread(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: format!("cc-{thread_id}"),
            branch: String::new(),
            repo_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// Buffer to put around a cutoff so the surrounding `resolved_at`
/// timestamps land strictly on the expected side. Has to absorb scheduler
/// jitter and Docker network round-trips on macOS, where 1ms is too tight.
const CUTOFF_GAP_MS: u64 = 50;

/// Returns Postgres `NOW()` so cutoffs are in the same clock as
/// `resolved_at` (set via SQL `NOW()`). Mixing Rust `Utc::now()` with
/// Postgres NOW() flakes under clock drift between host and the Postgres
/// container (notably after laptop sleep/wake on macOS Docker).
async fn pg_now(pool: &PgPool) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT NOW()")
        .fetch_one(pool)
        .await
        .expect("pg_now: SELECT NOW() failed")
}

#[tokio::test]
async fn empty_projection_has_no_pending_changes() {
    let (pool, db) = setup_test_db().await;
    let proj = ChangesProjection::new(pool);
    assert!(proj.list_pending().await.is_empty());
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn emit_change_proposed_writes_row_to_changes_table() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread_id).await;

    emit(
        &bus,
        thread_id,
        aggregate_proposed(change_id, "feat-bug1", "/repo/bug1"),
    )
    .await;

    let row: (String, String, String, Option<Uuid>) = sqlx::query_as(
        "SELECT status, branch_name, repo_root, thread_id FROM changes WHERE id = $1",
    )
    .bind(change_id)
    .fetch_one(&pool)
    .await
    .expect("row exists in changes table after ChangeProposed");
    assert_eq!(row.0, "pending");
    assert_eq!(row.1, "feat-bug1");
    assert_eq!(row.2, "/repo/bug1");
    assert_eq!(row.3, Some(thread_id));

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn aggregate_change_proposed_inserts_pending_change() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-a", "/repo"),
    )
    .await;

    let proj = ChangesProjection::new(pool);
    let pending = proj.list_pending().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, change_id);
    assert_eq!(pending[0].branch_name, "branch-a");
    assert_eq!(pending[0].repo_root, "/repo");
    assert_eq!(pending[0].thread_id, Some(thread));
    assert_eq!(pending[0].status, "pending");
    assert_eq!(pending[0].file_count, 2);
    assert!(pending[0].hardened);

    teardown_test_db(&db).await;
}

/// Re-emit with `description: None` must preserve the existing description
/// (matches the in-memory contract `if let Some(d) = description { ... }`).
/// Pre-fix the ON CONFLICT clause overwrote with EXCLUDED.description, so
/// `unwrap_or("")` blanked the description on every None re-emit.
#[tokio::test]
async fn re_emit_with_none_description_preserves_existing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-x", "/repo"),
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: None,
            files: vec!["a.rs".to_string()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: "branch-x".to_string(),
            repo_root: "/repo".to_string(),
            hardened: true,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
    )
    .await;

    let proj = ChangesProjection::new(pool);
    let row = proj.get_by_id(change_id).await.expect("row");
    assert_eq!(
        row.description, "aggregate description",
        "description must survive a None re-emit"
    );
    assert_eq!(
        row.files,
        vec!["a.rs".to_string()],
        "files still overwritten"
    );

    teardown_test_db(&db).await;
}

/// `incomplete: true` from a `ChangeProposed` proposed by a CC turn that
/// ended in `ResponseFailed` must round-trip to the `changes.incomplete`
/// column so the apply UI can surface the confirm-before-Apply warning.
/// A subsequent re-emit with `incomplete: false` (e.g. a follow-up
/// successful turn against the same branch) must clear the flag — without
/// this the confirm dialog would shadow every Apply on a once-failed
/// branch even after the user retried successfully.
#[tokio::test]
async fn incomplete_flag_round_trips_and_clears_on_re_emit() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    // First emit: turn ended in ResponseFailed → incomplete=true.
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: Some("partial work from failed run".to_string()),
            files: vec!["a.rs".to_string()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: "branch-fail".to_string(),
            repo_root: "/repo".to_string(),
            hardened: false,
            incomplete: true,
            path: String::new(),
            diff: String::new(),
        },
    )
    .await;

    let proj = ChangesProjection::new(pool.clone());
    let row = proj.get_by_id(change_id).await.expect("row");
    assert!(
        row.incomplete,
        "incomplete must round-trip to the projection column"
    );

    // Re-emit on the same branch with incomplete=false (successful follow-up).
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: Some("clean follow-up".to_string()),
            files: vec!["a.rs".to_string()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: "branch-fail".to_string(),
            repo_root: "/repo".to_string(),
            hardened: true,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
    )
    .await;

    let row = proj.get_by_id(change_id).await.expect("row");
    assert!(
        !row.incomplete,
        "successful re-emit must clear the prior failure tag"
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn re_emitted_aggregate_updates_existing_row() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-a", "/repo"),
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: Some("updated".to_string()),
            files: vec!["a.rs".to_string()],
            requires_restart: true,
            origin: None,
            commit_sha: None,
            branch_name: "branch-a".to_string(),
            repo_root: "/repo".to_string(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
    )
    .await;

    let proj = ChangesProjection::new(pool);
    let row = proj.get_by_id(change_id).await.expect("change exists");
    assert_eq!(row.description, "updated");
    assert_eq!(row.files, vec!["a.rs".to_string()]);
    assert_eq!(row.file_count, 1);
    assert!(row.requires_restart);
    assert!(!row.hardened, "re-emit with hardened=false must downgrade");

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn change_applied_transitions_to_applied_with_commits() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-a", "/repo"),
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeApplied {
            change_id: change_id.to_string(),
            requires_restart: true,
            client_update: false,
            commits: vec!["feat: x".to_string(), "fix: y".to_string()],
            thread_title: None,
            actor: None,
            pre_merge_sha: Some("aaa".to_string()),
            post_merge_sha: Some("bbb".to_string()),
            path: String::new(),
        },
    )
    .await;

    let proj = ChangesProjection::new(pool);
    assert!(proj.list_pending().await.is_empty());
    let row = proj.get_by_id(change_id).await.expect("row");
    assert_eq!(row.status, "applied");
    assert!(row.resolved_at.is_some());
    assert_eq!(
        row.commits,
        vec!["feat: x".to_string(), "fix: y".to_string()]
    );
    assert_eq!(row.pre_merge_sha.as_deref(), Some("aaa"));
    assert_eq!(row.post_merge_sha.as_deref(), Some("bbb"));
    assert!(row.requires_restart);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn change_discarded_transitions_to_discarded() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-a", "/repo"),
    )
    .await;
    emit(&bus, thread, discarded_event(change_id)).await;

    let proj = ChangesProjection::new(pool);
    assert!(proj.list_pending().await.is_empty());
    let row = proj.get_by_id(change_id).await.expect("row");
    assert_eq!(row.status, "discarded");
    assert!(row.resolved_at.is_some());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn change_reverted_transitions_to_reverted() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-a", "/repo"),
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeReverted {
            change_id: change_id.to_string(),
            actor: None,
            path: String::new(),
        },
    )
    .await;

    let proj = ChangesProjection::new(pool);
    let row = proj.get_by_id(change_id).await.expect("row");
    assert_eq!(row.status, "reverted");

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn change_hardened_sets_hardened_flag() {
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
            branch_name: "b".to_string(),
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

    let proj = ChangesProjection::new(pool);
    assert!(proj.get_by_id(change_id).await.unwrap().hardened);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn merge_resolution_started_sets_worktree_fields() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-a", "/repo"),
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::MergeResolutionStarted {
            change_id: change_id.to_string(),
            worktree_path: "/tmp/wt".to_string(),
            temp_branch: "merge-tmp/x".to_string(),
        },
    )
    .await;

    let proj = ChangesProjection::new(pool);
    let row = proj.get_by_id(change_id).await.unwrap();
    assert_eq!(row.merge_worktree_path.as_deref(), Some("/tmp/wt"));
    assert_eq!(row.merge_temp_branch.as_deref(), Some("merge-tmp/x"));

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn merge_resolution_cleared_clears_worktree_fields() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-a", "/repo"),
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::MergeResolutionStarted {
            change_id: change_id.to_string(),
            worktree_path: "/tmp/wt".to_string(),
            temp_branch: "merge-tmp/x".to_string(),
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

    let proj = ChangesProjection::new(pool);
    let row = proj.get_by_id(change_id).await.unwrap();
    assert!(row.merge_worktree_path.is_none());
    assert!(row.merge_temp_branch.is_none());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn per_commit_event_updates_existing_pending_row() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-a", "/repo"),
    )
    .await;
    emit(
        &bus,
        thread,
        per_commit_proposed("branch-a", "abc123", "fix: latest commit", true),
    )
    .await;

    let proj = ChangesProjection::new(pool);
    let row = proj.get_by_id(change_id).await.unwrap();
    assert_eq!(row.description, "fix: latest commit");
    assert!(row.requires_restart);
    assert!(!row.hardened, "new commit invalidates prior harden marker");

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn per_commit_event_without_aggregate_is_noop() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        per_commit_proposed("branch-a", "abc123", "fix: x", false),
    )
    .await;

    let proj = ChangesProjection::new(pool);
    assert!(proj.list_pending().await.is_empty());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn pending_for_thread_filters_by_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_a = Uuid::new_v4();
    let thread_b = Uuid::new_v4();
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();
    start_cc_thread(&bus, thread_a).await;
    start_cc_thread(&bus, thread_b).await;

    emit(
        &bus,
        thread_a,
        aggregate_proposed(id_a, "branch-a", "/repo"),
    )
    .await;
    emit(
        &bus,
        thread_b,
        aggregate_proposed(id_b, "branch-b", "/repo"),
    )
    .await;

    let proj = ChangesProjection::new(pool);
    let only_a = proj.pending_for_thread(thread_a).await;
    assert_eq!(only_a.len(), 1);
    assert_eq!(only_a[0].id, id_a);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn get_pending_by_branch_returns_only_pending() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-a", "/repo"),
    )
    .await;
    let proj = ChangesProjection::new(pool.clone());
    assert!(proj.get_pending_by_branch("branch-a").await.is_some());
    assert!(proj.get_pending_by_branch("branch-b").await.is_none());

    emit(&bus, thread, applied_event(change_id, &[], false)).await;
    assert!(proj.get_pending_by_branch("branch-a").await.is_none());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn has_pending_for_branch_reflects_pending_state() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    let proj = ChangesProjection::new(pool.clone());
    assert!(!proj.has_pending_for_branch("branch-a").await);

    emit(&bus, thread, aggregate_proposed(id, "branch-a", "/repo")).await;
    assert!(proj.has_pending_for_branch("branch-a").await);
    assert!(!proj.has_pending_for_branch("branch-b").await);

    emit(&bus, thread, applied_event(id, &["feat: x"], false)).await;
    assert!(
        !proj.has_pending_for_branch("branch-a").await,
        "applied → no longer pending"
    );

    teardown_test_db(&db).await;
}

/// `idx_changes_unique_pending_branch` keeps the pending count per branch
/// to one, so `other_pending_for_branch` is always false in practice.
/// `change_ops::discard_change` calls it defensively before wiping a branch.
#[tokio::test]
async fn other_pending_for_branch_returns_false_when_only_one_pending() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(&bus, thread, aggregate_proposed(id, "branch-x", "/repo")).await;
    let proj = ChangesProjection::new(pool.clone());
    assert!(!proj.other_pending_for_branch("branch-x", id).await);
    assert!(!proj.other_pending_for_branch("branch-y", id).await);

    // A different change on a different branch — still no overlap on branch-x.
    let other_id = Uuid::new_v4();
    emit(
        &bus,
        thread,
        aggregate_proposed(other_id, "branch-y", "/repo"),
    )
    .await;
    assert!(!proj.other_pending_for_branch("branch-x", id).await);
    assert!(!proj.other_pending_for_branch("branch-y", other_id).await);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn list_completed_branches_returns_distinct_branches() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();
    let id_c = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(&bus, thread, aggregate_proposed(id_a, "branch-a", "/r")).await;
    emit(&bus, thread, applied_event(id_a, &["feat: a"], false)).await;
    emit(&bus, thread, aggregate_proposed(id_b, "branch-b", "/r")).await;
    emit(&bus, thread, discarded_event(id_b)).await;
    // Pending only — must NOT appear in completed
    emit(&bus, thread, aggregate_proposed(id_c, "branch-c", "/r")).await;

    let proj = ChangesProjection::new(pool);
    let mut completed = proj.list_completed_branches().await;
    completed.sort();
    assert_eq!(
        completed,
        vec!["branch-a".to_string(), "branch-b".to_string()]
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn list_recently_applied_orders_newest_first_with_limit_and_cursor() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    for label in ["a", "b", "c"].iter() {
        let id = Uuid::new_v4();
        emit(&bus, thread, aggregate_proposed(id, label, "/r")).await;
        emit(
            &bus,
            thread,
            applied_event(id, &[&format!("commit-{label}")], false),
        )
        .await;
    }

    let proj = ChangesProjection::new(pool);
    let all = proj.list_recently_applied(10, None).await;
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].branch_name, "c", "newest first");
    assert_eq!(all[1].branch_name, "b");
    assert_eq!(all[2].branch_name, "a");

    let two = proj.list_recently_applied(2, None).await;
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].branch_name, "c");
    assert_eq!(two[1].branch_name, "b");

    let before_b = all[1].resolved_at.unwrap();
    let older = proj.list_recently_applied(10, Some(before_b)).await;
    assert_eq!(older.len(), 1);
    assert_eq!(older[0].branch_name, "a");

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn list_recently_applied_includes_reverted() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(&bus, thread, aggregate_proposed(id, "branch-x", "/r")).await;
    emit(&bus, thread, applied_event(id, &["c1"], false)).await;
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeReverted {
            change_id: id.to_string(),
            actor: None,
            path: String::new(),
        },
    )
    .await;

    let proj = ChangesProjection::new(pool);
    let recent = proj.list_recently_applied(10, None).await;
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].status, "reverted");

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn list_for_repo_filters_by_repo_root_and_paginates() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    let pa = Uuid::new_v4();
    emit(&bus, thread, aggregate_proposed(pa, "p-a", "/repo-A")).await;
    let aa1 = Uuid::new_v4();
    emit(&bus, thread, aggregate_proposed(aa1, "a-a-1", "/repo-A")).await;
    emit(&bus, thread, applied_event(aa1, &["x"], false)).await;
    let aa2 = Uuid::new_v4();
    emit(&bus, thread, aggregate_proposed(aa2, "a-a-2", "/repo-A")).await;
    emit(&bus, thread, applied_event(aa2, &["y"], false)).await;

    let bb = Uuid::new_v4();
    emit(&bus, thread, aggregate_proposed(bb, "b-b", "/repo-B")).await;
    emit(&bus, thread, applied_event(bb, &["z"], false)).await;

    let proj = ChangesProjection::new(pool);
    let (pending, applied, has_more) = proj.list_for_repo("/repo-A", 10, None).await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, pa);
    assert_eq!(applied.len(), 2);
    assert!(!has_more);
    assert!(applied.iter().all(|c| c.repo_root == "/repo-A"));

    let (_, applied2, has_more2) = proj.list_for_repo("/repo-A", 1, None).await;
    assert_eq!(applied2.len(), 1);
    assert!(has_more2);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn requires_restart_since_filters_by_resolved_at() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    // Sandwich the cutoff between sleeps so `early.resolved_at` is strictly
    // less and the next apply's `resolved_at` is strictly greater.
    let early = Uuid::new_v4();
    emit(&bus, thread, aggregate_proposed(early, "early", "/r")).await;
    emit(&bus, thread, applied_event(early, &["c"], true)).await;
    tokio::time::sleep(std::time::Duration::from_millis(CUTOFF_GAP_MS)).await;
    let cutoff = pg_now(&pool).await;
    tokio::time::sleep(std::time::Duration::from_millis(CUTOFF_GAP_MS)).await;

    let proj = ChangesProjection::new(pool.clone());
    assert!(
        !proj.requires_restart_since(cutoff).await,
        "early change applied before cutoff must not match"
    );

    let new_id = Uuid::new_v4();
    emit(&bus, thread, aggregate_proposed(new_id, "new", "/r")).await;
    emit(&bus, thread, applied_event(new_id, &["c"], true)).await;
    assert!(proj.requires_restart_since(cutoff).await);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn requires_restart_since_ignores_non_restart_changes() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    let cutoff = Utc::now() - chrono::Duration::seconds(1);

    emit(&bus, thread, aggregate_proposed(id, "b", "/r")).await;
    emit(&bus, thread, applied_event(id, &["c"], false)).await;

    let proj = ChangesProjection::new(pool);
    assert!(!proj.requires_restart_since(cutoff).await);

    teardown_test_db(&db).await;
}

/// `broadcast_changes_updated` passes a sentinel meaning "since forever" to
/// answer "is any restart-required change applied at all?". Postgres
/// `timestamptz` cannot represent `chrono::DateTime::<Utc>::MIN_UTC`
/// (year -262143) so binding it returns `error: timestamp out of range`,
/// which silently degrades to `false` and the toast never appears. The
/// `Utc` epoch (1970) is the canonical sentinel — well within the Postgres
/// timestamptz domain (4713 BC … 294276 AD).
#[tokio::test]
async fn requires_restart_since_unix_epoch_returns_correct_result() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(&bus, thread, aggregate_proposed(id, "b", "/r")).await;
    emit(&bus, thread, applied_event(id, &["c"], true)).await;

    let proj = ChangesProjection::new(pool);
    let epoch = DateTime::<Utc>::UNIX_EPOCH;
    assert!(
        proj.requires_restart_since(epoch).await,
        "epoch sentinel must surface applied restart change (regression: \
             MIN_UTC overflowed timestamptz, swallowed the error, returned false)"
    );

    teardown_test_db(&db).await;
}

fn proposed_with_files(
    change_id: Uuid,
    branch: &str,
    repo_root: &str,
    files: Vec<&str>,
) -> ThreadEvent {
    ThreadEvent::ChangeProposed {
        change_id: change_id.to_string(),
        description: Some("d".into()),
        files: files.iter().map(|s| s.to_string()).collect(),
        requires_restart: false,
        origin: None,
        commit_sha: None,
        branch_name: branch.into(),
        repo_root: repo_root.into(),
        hardened: true,
        incomplete: false,
        path: String::new(),
        diff: String::new(),
    }
}

#[tokio::test]
async fn client_update_since_detects_frontend_files() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    let cutoff = pg_now(&pool).await;
    tokio::time::sleep(std::time::Duration::from_millis(CUTOFF_GAP_MS)).await;

    emit(
        &bus,
        thread,
        proposed_with_files(id, "b", "/r", vec!["src/app.ts"]),
    )
    .await;
    emit(&bus, thread, applied_event(id, &["c"], false)).await;

    let proj = ChangesProjection::new(pool);
    assert!(proj.client_update_since(cutoff).await);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn client_update_since_ignores_non_frontend_files() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    let cutoff = Utc::now() - chrono::Duration::seconds(1);

    emit(
        &bus,
        thread,
        proposed_with_files(id, "b", "/r", vec!["src/lib.rs", "Cargo.toml"]),
    )
    .await;
    emit(&bus, thread, applied_event(id, &["c"], false)).await;

    let proj = ChangesProjection::new(pool);
    assert!(!proj.client_update_since(cutoff).await);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn client_update_since_ignores_pre_cutoff() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    emit(
        &bus,
        thread,
        proposed_with_files(id, "b", "/r", vec!["src/app.tsx"]),
    )
    .await;
    emit(&bus, thread, applied_event(id, &["c"], false)).await;
    // Cutoff in the future → no events qualify
    let cutoff = Utc::now() + chrono::Duration::seconds(60);

    let proj = ChangesProjection::new(pool);
    assert!(!proj.client_update_since(cutoff).await);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn restart_groups_since_groups_by_thread_in_apply_order() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_a = Uuid::new_v4();
    let thread_b = Uuid::new_v4();
    start_cc_thread(&bus, thread_a).await;
    start_cc_thread(&bus, thread_b).await;
    let cutoff = pg_now(&pool).await;
    tokio::time::sleep(std::time::Duration::from_millis(CUTOFF_GAP_MS)).await;

    // Sleeps between rapid applies force strictly-distinct `resolved_at`
    // timestamps (Postgres `transaction_timestamp()` has microsecond
    // resolution, so back-to-back txs can collide under heavy load and
    // make `ORDER BY resolved_at ASC` non-deterministic).
    let a1 = Uuid::new_v4();
    emit(&bus, thread_a, aggregate_proposed(a1, "a-1", "/r")).await;
    emit(
        &bus,
        thread_a,
        applied_event(a1, &["fix: a1", "fix: a2"], true),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(CUTOFF_GAP_MS)).await;

    let a2 = Uuid::new_v4();
    emit(&bus, thread_a, aggregate_proposed(a2, "a-2", "/r")).await;
    emit(&bus, thread_a, applied_event(a2, &["fix: a3"], true)).await;
    tokio::time::sleep(std::time::Duration::from_millis(CUTOFF_GAP_MS)).await;

    let b1 = Uuid::new_v4();
    emit(&bus, thread_b, aggregate_proposed(b1, "b-1", "/r")).await;
    emit(&bus, thread_b, applied_event(b1, &["feat: b1"], true)).await;

    let proj = ChangesProjection::new(pool);
    let groups = proj.restart_groups_since(cutoff).await;
    assert_eq!(groups.len(), 2, "one group per thread, got {:?}", groups);
    let g_a = groups
        .iter()
        .find(|g| g.thread_id == Some(thread_a))
        .expect("a");
    assert_eq!(
        g_a.commits,
        vec![
            "fix: a1".to_string(),
            "fix: a2".to_string(),
            "fix: a3".to_string()
        ]
    );
    let g_b = groups
        .iter()
        .find(|g| g.thread_id == Some(thread_b))
        .expect("b");
    assert_eq!(g_b.commits, vec!["feat: b1".to_string()]);
    assert!(g_a.thread_title.is_none());
    assert!(g_b.thread_title.is_none());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn restart_groups_since_ignores_non_restart() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    let cutoff = Utc::now() - chrono::Duration::seconds(1);

    emit(&bus, thread, aggregate_proposed(id, "b", "/r")).await;
    emit(&bus, thread, applied_event(id, &["x"], false)).await;

    let proj = ChangesProjection::new(pool);
    assert!(proj.restart_groups_since(cutoff).await.is_empty());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn with_merge_worktree_returns_only_pending_with_active_merge() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    let with_merge = Uuid::new_v4();
    let no_merge = Uuid::new_v4();
    let cleared = Uuid::new_v4();

    emit(&bus, thread, aggregate_proposed(with_merge, "wm", "/r")).await;
    emit(
        &bus,
        thread,
        ThreadEvent::MergeResolutionStarted {
            change_id: with_merge.to_string(),
            worktree_path: "/tmp/wt-1".into(),
            temp_branch: "merge/x".into(),
        },
    )
    .await;

    emit(&bus, thread, aggregate_proposed(no_merge, "nm", "/r")).await;

    emit(&bus, thread, aggregate_proposed(cleared, "cl", "/r")).await;
    emit(
        &bus,
        thread,
        ThreadEvent::MergeResolutionStarted {
            change_id: cleared.to_string(),
            worktree_path: "/tmp/wt-2".into(),
            temp_branch: "merge/y".into(),
        },
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::MergeResolutionCleared {
            change_id: cleared.to_string(),
        },
    )
    .await;

    let proj = ChangesProjection::new(pool);
    let active = proj.with_merge_worktree().await;
    assert_eq!(active.len(), 1, "only one with active merge: {:?}", active);
    assert_eq!(active[0].id, with_merge);
    assert_eq!(active[0].merge_worktree_path.as_deref(), Some("/tmp/wt-1"));

    teardown_test_db(&db).await;
}

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
        proj.get_by_id(change_id).await.is_none(),
        "row gone before rebuild"
    );

    let n = proj.rebuild_missing_from_events().await.unwrap();
    assert_eq!(n, 1, "one row recovered");

    let row = proj.get_by_id(change_id).await.expect("rebuilt row");
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
    let row = proj.get_by_id(change_id).await.expect("rebuilt row");
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
    let row = proj.get_by_id(change_id).await.expect("rebuilt row");
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
    let row = proj.get_by_id(change_id).await.expect("rebuilt row");
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
    let row = proj.get_by_id(change_id).await.expect("rebuilt row");
    assert_eq!(row.merge_worktree_path.as_deref(), Some("/tmp/wt-rebuild"));
    assert_eq!(row.merge_temp_branch.as_deref(), Some("merge-tmp/rebuild"));
    assert!(proj
        .with_merge_worktree()
        .await
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
    let row = proj.get_by_id(change_id).await.expect("rebuilt row");
    assert!(row.merge_worktree_path.is_none());
    assert!(row.merge_temp_branch.is_none());

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
/// runtime gates (`should_propose_change_at_idle`, the SessionEnded
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
                (thread_id, source, is_cc, created_at, last_activity, message_count, status, cc_is_external_repo) \
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
        (external_change, external_thread, "feature/UA-1764"),
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
                (SELECT thread_id FROM thread_summaries WHERE cc_is_external_repo = true)",
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
