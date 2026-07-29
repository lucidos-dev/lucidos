use super::*;
use super::cp_helpers::*;

#[tokio::test]
async fn empty_projection_has_no_pending_changes() {
    let (pool, db) = setup_test_db().await;
    let proj = ChangesProjection::new(pool);
    assert!(proj.list_pending().await.unwrap().is_empty());
    teardown_test_db(&db).await;
}

/// Closing the pool forces every subsequent query to error with
/// `sqlx::Error::PoolClosed`. Pre-fix, `list_pending` swallowed the error
/// and returned `Vec::new()` — indistinguishable from a healthy empty
/// projection. Post-fix, the error propagates so the HTTP layer can
/// surface a 500 instead of pretending the DB is fine. Anchors the
/// `-> Result<Vec<Change>, sqlx::Error>` contract for every read method on
/// `ChangesProjection`.
#[tokio::test]
async fn list_pending_propagates_db_error_instead_of_returning_empty() {
    let (pool, db) = setup_test_db().await;
    let proj = ChangesProjection::new(pool.clone());
    pool.close().await;
    let result = proj.list_pending().await;
    assert!(
        result.is_err(),
        "list_pending must return Err on DB outage, got Ok({:?}) — \
         silently returning an empty Vec masks a DB failure as 'no changes'",
        result
    );
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
    let pending = proj.list_pending().await.unwrap();
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
    let row = proj.get_by_id(change_id).await.unwrap().expect("row");
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
    let row = proj.get_by_id(change_id).await.unwrap().expect("row");
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

    let row = proj.get_by_id(change_id).await.unwrap().expect("row");
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
    let row = proj.get_by_id(change_id).await.unwrap().expect("change exists");
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
    assert!(proj.list_pending().await.unwrap().is_empty());
    let row = proj.get_by_id(change_id).await.unwrap().expect("row");
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
    assert!(proj.list_pending().await.unwrap().is_empty());
    let row = proj.get_by_id(change_id).await.unwrap().expect("row");
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
    let row = proj.get_by_id(change_id).await.unwrap().expect("row");
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
    assert!(proj.get_by_id(change_id).await.unwrap().unwrap().hardened);

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
    let row = proj.get_by_id(change_id).await.unwrap().unwrap();
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
    let row = proj.get_by_id(change_id).await.unwrap().unwrap();
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
    let row = proj.get_by_id(change_id).await.unwrap().unwrap();
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
    assert!(proj.list_pending().await.unwrap().is_empty());

    teardown_test_db(&db).await;
}

/// The reconcile path re-emits the aggregate `ChangeProposed` for the SAME
/// change id with an empty file list. The row must follow git: zero files, no
/// restart, still `pending` (the engine never resolves a change on the user's
/// behalf — commit `cca058432`), and the description refreshed. Pre-fix the row
/// kept its snapshot, so the card read "1 file · Requires engine restart" while
/// the Diff button rendered "No changes" (change `2cc8391f`).
#[tokio::test]
async fn empty_files_re_emit_zeroes_the_row_but_keeps_it_pending() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    let mut proposed = aggregate_proposed(change_id, "branch-a", "/repo");
    if let ThreadEvent::ChangeProposed {
        ref mut files,
        ref mut requires_restart,
        ..
    } = proposed
    {
        *files = vec!["crates/lucidos-engine/migrations/0001_stray.sql".to_string()];
        *requires_restart = true;
    }
    emit(&bus, thread, proposed).await;

    let proj = ChangesProjection::new(pool.clone());
    let before = proj.get_by_id(change_id).await.unwrap().unwrap();
    assert_eq!(before.file_count, 1);
    assert!(before.requires_restart);

    // The branch's commits cancelled out — reconcile.
    let mut emptied = aggregate_proposed(change_id, "branch-a", "/repo");
    if let ThreadEvent::ChangeProposed {
        ref mut files,
        ref mut description,
        ..
    } = emptied
    {
        *files = vec![];
        *description = Some("chore: drop the stray file".to_string());
    }
    emit(&bus, thread, emptied).await;

    let after = proj.get_by_id(change_id).await.unwrap().unwrap();
    assert_eq!(after.id, change_id, "same change, corrected in place");
    assert_eq!(after.status, "pending", "reconcile is not a discard");
    assert_eq!(after.file_count, 0);
    assert!(after.files.is_empty());
    assert!(
        !after.requires_restart,
        "a change with no files cannot require an engine restart"
    );
    assert_eq!(after.description, "chore: drop the stray file");

    teardown_test_db(&db).await;
}

/// `coding_agent_has_diff` is derived from the file list the event carries, not
/// hardcoded TRUE. Without this the reconcile would light the thread's Diff
/// button on a branch whose live `git diff` is empty — the very disagreement
/// the reconcile exists to remove.
#[tokio::test]
async fn has_diff_follows_the_proposed_file_list() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;

    let has_diff = |pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, bool>(
            "SELECT coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread)
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    // Ordinary proposal (non-empty files) — unchanged behaviour.
    emit(
        &bus,
        thread,
        aggregate_proposed(change_id, "branch-a", "/repo"),
    )
    .await;
    assert!(
        has_diff(pool.clone()).await,
        "a proposal with files must light the Diff button"
    );

    let mut emptied = aggregate_proposed(change_id, "branch-a", "/repo");
    if let ThreadEvent::ChangeProposed { ref mut files, .. } = emptied {
        *files = vec![];
    }
    emit(&bus, thread, emptied).await;
    assert!(
        !has_diff(pool.clone()).await,
        "an emptied change must NOT claim the thread has a diff"
    );

    teardown_test_db(&db).await;
}
