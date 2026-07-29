use super::*;
use super::cp_helpers::*;

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
    let only_a = proj.pending_for_thread(thread_a).await.unwrap();
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
    assert!(proj.get_pending_by_branch("branch-a").await.unwrap().is_some());
    assert!(proj.get_pending_by_branch("branch-b").await.unwrap().is_none());

    emit(&bus, thread, applied_event(change_id, &[], false)).await;
    assert!(proj.get_pending_by_branch("branch-a").await.unwrap().is_none());

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
    assert!(!proj.has_pending_for_branch("branch-a").await.unwrap());

    emit(&bus, thread, aggregate_proposed(id, "branch-a", "/repo")).await;
    assert!(proj.has_pending_for_branch("branch-a").await.unwrap());
    assert!(!proj.has_pending_for_branch("branch-b").await.unwrap());

    emit(&bus, thread, applied_event(id, &["feat: x"], false)).await;
    assert!(
        !proj.has_pending_for_branch("branch-a").await.unwrap(),
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
    assert!(!proj.other_pending_for_branch("branch-x", id).await.unwrap());
    assert!(!proj.other_pending_for_branch("branch-y", id).await.unwrap());

    // A different change on a different branch — still no overlap on branch-x.
    let other_id = Uuid::new_v4();
    emit(
        &bus,
        thread,
        aggregate_proposed(other_id, "branch-y", "/repo"),
    )
    .await;
    assert!(!proj.other_pending_for_branch("branch-x", id).await.unwrap());
    assert!(!proj.other_pending_for_branch("branch-y", other_id).await.unwrap());

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
    let mut completed = proj.list_completed_branches().await.unwrap();
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
    let all = proj.list_recently_applied(10, None).await.unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].branch_name, "c", "newest first");
    assert_eq!(all[1].branch_name, "b");
    assert_eq!(all[2].branch_name, "a");

    let two = proj.list_recently_applied(2, None).await.unwrap();
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].branch_name, "c");
    assert_eq!(two[1].branch_name, "b");

    let before_b = all[1].resolved_at.unwrap();
    let older = proj.list_recently_applied(10, Some(before_b)).await.unwrap();
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
    let recent = proj.list_recently_applied(10, None).await.unwrap();
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
    let (pending, applied, has_more) = proj.list_for_repo("/repo-A", 10, None).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, pa);
    assert_eq!(applied.len(), 2);
    assert!(!has_more);
    assert!(applied.iter().all(|c| c.repo_root == "/repo-A"));

    let (_, applied2, has_more2) = proj.list_for_repo("/repo-A", 1, None).await.unwrap();
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
        !proj.requires_restart_since(cutoff).await.unwrap(),
        "early change applied before cutoff must not match"
    );

    let new_id = Uuid::new_v4();
    emit(&bus, thread, aggregate_proposed(new_id, "new", "/r")).await;
    emit(&bus, thread, applied_event(new_id, &["c"], true)).await;
    assert!(proj.requires_restart_since(cutoff).await.unwrap());

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
    assert!(!proj.requires_restart_since(cutoff).await.unwrap());

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
        proj.requires_restart_since(epoch).await.unwrap(),
        "epoch sentinel must surface applied restart change (regression: \
             MIN_UTC overflowed timestamptz, swallowed the error, returned false)"
    );

    teardown_test_db(&db).await;
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
    assert!(proj.client_update_since(cutoff).await.unwrap());

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
    assert!(!proj.client_update_since(cutoff).await.unwrap());

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
    assert!(!proj.client_update_since(cutoff).await.unwrap());

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
    let groups = proj.restart_groups_since(cutoff).await.unwrap();
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
    assert!(proj.restart_groups_since(cutoff).await.unwrap().is_empty());

    teardown_test_db(&db).await;
}

/// The conflict-resolution duty derivation: a pending change whose latest
/// merge-lifecycle event is an unpaired `MergeConflictDetected` is an
/// in-flight conflict resolution; any closing event ends the duty, and a
/// later retry's `MergeConflictDetected` re-opens it. This is what lets an
/// auto-recovery continuation re-attach the merge duty after a stray kill
/// (`ConflictResolutionCleanupAction::HandOff` skips the closing emits so
/// the pairing stays open on purpose).
#[tokio::test]
async fn pending_conflict_change_follows_merge_event_pairing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    let change = Uuid::new_v4();
    let proj = ChangesProjection::new(pool);

    // Pending change with no merge activity → no duty.
    emit(&bus, thread, aggregate_proposed(change, "cr", "/r")).await;
    assert!(
        proj.pending_conflict_change_for_thread(thread)
            .await
            .unwrap()
            .is_none(),
        "a pending change without MergeConflictDetected is not a duty"
    );

    // Conflict resolution started → duty open.
    emit(
        &bus,
        thread,
        ThreadEvent::MergeConflictDetected {
            change_id: change.to_string(),
            files: vec!["a.rs".to_string()],
            origin: None,
        },
    )
    .await;
    let duty = proj
        .pending_conflict_change_for_thread(thread)
        .await
        .unwrap()
        .expect("unpaired MergeConflictDetected must surface the duty");
    assert_eq!(duty.id, change);

    // A real abort closes the pairing (MergeResolutionCleared +
    // ChangeApplyFailed) — the change stays pending but the duty is gone.
    emit(
        &bus,
        thread,
        ThreadEvent::MergeResolutionCleared {
            change_id: change.to_string(),
        },
    )
    .await;
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeApplyFailed {
            change_id: change.to_string(),
            error: "merge aborted".to_string(),
            actor: None,
        },
    )
    .await;
    assert!(
        proj.pending_conflict_change_for_thread(thread)
            .await
            .unwrap()
            .is_none(),
        "a closed pairing must not resurrect the duty"
    );

    assert!(
        !proj.conflict_pairing_open(thread, change).await.unwrap(),
        "the change-scoped probe agrees the pairing is closed"
    );

    // A later apply retry re-opens it.
    emit(
        &bus,
        thread,
        ThreadEvent::MergeConflictDetected {
            change_id: change.to_string(),
            files: vec!["a.rs".to_string()],
            origin: None,
        },
    )
    .await;
    assert!(
        proj.pending_conflict_change_for_thread(thread)
            .await
            .unwrap()
            .is_some(),
        "a retry's MergeConflictDetected re-opens the duty"
    );
    assert!(
        proj.conflict_pairing_open(thread, change).await.unwrap(),
        "the change-scoped probe agrees the pairing is open"
    );

    // The apply landing ends it for good (row leaves pending AND the
    // pairing closes).
    emit(&bus, thread, applied_event(change, &["fix: x"], false)).await;
    assert!(
        proj.pending_conflict_change_for_thread(thread)
            .await
            .unwrap()
            .is_none(),
        "an applied change can never be a duty"
    );
    assert!(
        !proj.conflict_pairing_open(thread, change).await.unwrap(),
        "an applied change's pairing reads closed"
    );

    teardown_test_db(&db).await;
}

/// With two open pairings on one thread (an older one stranded by a crash),
/// the NEWEST wins — the continuation that just fired belongs to the most
/// recently started merge; binding the stranded older change would ff-merge
/// the wrong branch on a clean turn end. The change-scoped probe still
/// reports both open, so change-aware callers are unaffected.
#[tokio::test]
async fn pending_conflict_change_prefers_newest_open_pairing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    let stranded = Uuid::new_v4();
    let live = Uuid::new_v4();
    let proj = ChangesProjection::new(pool);

    for (id, branch) in [(stranded, "old"), (live, "new")] {
        emit(&bus, thread, aggregate_proposed(id, branch, "/r")).await;
        emit(
            &bus,
            thread,
            ThreadEvent::MergeConflictDetected {
                change_id: id.to_string(),
                files: vec!["a.rs".to_string()],
                origin: None,
            },
        )
        .await;
    }

    let duty = proj
        .pending_conflict_change_for_thread(thread)
        .await
        .unwrap()
        .expect("two open pairings must still surface a duty");
    assert_eq!(duty.id, live, "the newest open pairing wins");
    assert!(proj.conflict_pairing_open(thread, stranded).await.unwrap());
    assert!(proj.conflict_pairing_open(thread, live).await.unwrap());

    // Closing the newest falls back to the stranded one.
    emit(
        &bus,
        thread,
        ThreadEvent::ChangeApplyFailed {
            change_id: live.to_string(),
            error: "aborted".to_string(),
            actor: None,
        },
    )
    .await;
    let duty = proj
        .pending_conflict_change_for_thread(thread)
        .await
        .unwrap()
        .expect("the stranded pairing is still open");
    assert_eq!(duty.id, stranded);

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
    let active = proj.with_merge_worktree().await.unwrap();
    assert_eq!(active.len(), 1, "only one with active merge: {:?}", active);
    assert_eq!(active[0].id, with_merge);
    assert_eq!(active[0].merge_worktree_path.as_deref(), Some("/tmp/wt-1"));

    teardown_test_db(&db).await;
}
