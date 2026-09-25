use super::cp_helpers::*;
use super::*;
use crate::core::changes::ChangeStatus;

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
    assert!(proj
        .get_pending_by_branch("branch-a")
        .await
        .unwrap()
        .is_some());
    assert!(proj
        .get_pending_by_branch("branch-b")
        .await
        .unwrap()
        .is_none());

    emit(&bus, thread, applied_event(change_id, &[], false)).await;
    assert!(proj
        .get_pending_by_branch("branch-a")
        .await
        .unwrap()
        .is_none());

    teardown_test_db(&db).await;
}

/// The case ADR 0106 feared, at the projection. A parent's change is applied,
/// then the parent commits again on the same branch. `emit_change_proposed`
/// asks `get_pending_by_branch` for an id to reuse; after the apply that
/// answers `None`, so the next proposal is a NEW row. The applied row keeps
/// its status and commits, and the one-pending-per-branch index does not
/// trip on it (ADR 0249).
#[tokio::test]
async fn a_proposal_after_an_apply_on_the_same_branch_is_a_new_change() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    let first = Uuid::new_v4();
    start_cc_thread(&bus, parent).await;
    let proj = ChangesProjection::new(pool.clone());

    emit(
        &bus,
        parent,
        aggregate_proposed(first, "thread-parent", "/repo"),
    )
    .await;
    emit(
        &bus,
        parent,
        applied_event(first, &["feat: first round"], false),
    )
    .await;

    assert!(
        proj.get_pending_by_branch("thread-parent")
            .await
            .unwrap()
            .is_none(),
        "the proposal path must find no pending change to fold into"
    );

    let second = Uuid::new_v4();
    emit(
        &bus,
        parent,
        aggregate_proposed(second, "thread-parent", "/repo"),
    )
    .await;

    let pending = proj.pending_for_thread(parent).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, second, "the new round is its own change");

    let applied = proj.get_by_id(first).await.unwrap().expect("applied row");
    assert_eq!(
        applied.status(),
        ChangeStatus::Applied,
        "the applied change stays applied"
    );
    assert_eq!(applied.commits, vec!["feat: first round".to_string()]);

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
    assert!(!proj
        .other_pending_for_branch("branch-y", other_id)
        .await
        .unwrap());

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
    let older = proj
        .list_recently_applied(10, Some(before_b))
        .await
        .unwrap();
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
    assert_eq!(recent[0].status(), ChangeStatus::Reverted);

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

/// The Changes panel reads `resolving_conflict` off the served frame. So a
/// reload still shows a conflict-resolving apply as in flight, with no
/// standing apply offered. It takes an open pairing on a thread that has not
/// finished: a pairing a crash stranded on a settled thread keeps its Discard.
#[tokio::test]
async fn enrich_marks_only_changes_whose_conflict_pairing_is_open() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let proj = ChangesProjection::new(pool.clone());

    let conflict = |change: Uuid| ThreadEvent::MergeConflictDetected {
        change_id: change.to_string(),
        files: vec!["a.rs".to_string()],
        origin: None,
    };
    let [resolving, untouched, cleared, stranded] =
        [(); 4].map(|_| (Uuid::new_v4(), Uuid::new_v4()));
    for (i, (thread, change)) in [resolving, untouched, cleared, stranded]
        .into_iter()
        .enumerate()
    {
        start_cc_thread(&bus, thread).await;
        emit(
            &bus,
            thread,
            aggregate_proposed(change, &format!("b{i}"), "/r"),
        )
        .await;
    }
    for (thread, status) in [
        (resolving.0, "running"),
        (untouched.0, "running"),
        (cleared.0, "running"),
        (stranded.0, "idle"),
    ] {
        sqlx::query("UPDATE thread_summaries SET status = $2 WHERE thread_id = $1")
            .bind(thread)
            .bind(status)
            .execute(&pool)
            .await
            .unwrap();
    }
    emit(&bus, resolving.0, conflict(resolving.1)).await;
    emit(&bus, stranded.0, conflict(stranded.1)).await;
    emit(&bus, cleared.0, conflict(cleared.1)).await;
    emit(
        &bus,
        cleared.0,
        ThreadEvent::MergeResolutionCleared {
            change_id: cleared.1.to_string(),
        },
    )
    .await;

    let pending = crate::core::changes::list_pending_for_readers(
        &pool,
        &proj,
        crate::core::changes::PendingScope::All,
    )
    .await
    .unwrap();
    for (thread, change) in [resolving, untouched, cleared] {
        let row = pending
            .iter()
            .find(|c| c.id == change)
            .expect("pending row");
        assert_eq!(
            row.thread_state().unwrap().resolving_conflict(),
            proj.conflict_pairing_open(thread, change).await.unwrap(),
            "the served flag and the merge guard read one definition"
        );
    }
    let flagged: Vec<Uuid> = pending
        .iter()
        .filter(|c| c.thread_state().unwrap().resolving_conflict())
        .map(|c| c.id)
        .collect();
    assert_eq!(
        flagged,
        vec![resolving.1],
        "only the open pairing on an unfinished thread is flagged"
    );
    assert!(
        proj.conflict_pairing_open(stranded.0, stranded.1)
            .await
            .unwrap(),
        "the stranded pairing is still open, so only the thread's state clears the flag"
    );

    teardown_test_db(&db).await;
}

/// Regression, 2026-08-11: the pairing must stay open for the WHOLE
/// resolution, not just until the resolver's first sign of life. `apply_change`
/// now refuses to move `main` while `conflict_pairing_open` is true, so
/// anything that closed the pairing early would re-arm the exact incident: a
/// second apply landing the merge at step 2 of the resolver's 5-step prompt and
/// then resetting its worktree mid-turn.
///
/// The mid-turn traffic replayed here is what the buggy path actually produced:
/// a `CodingAgentIdled` (emitted by `apply_now_success`'s worktree reset) and a
/// fresh `ChangeProposed` for the same change (the resolver's auto-commit
/// hook). Neither is a merge-lifecycle event, so neither may close the pairing.
#[tokio::test]
async fn conflict_pairing_stays_open_through_the_resolver_mid_turn_traffic() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    let change = Uuid::new_v4();
    let proj = ChangesProjection::new(pool);

    emit(&bus, thread, aggregate_proposed(change, "jitter", "/r")).await;
    emit(
        &bus,
        thread,
        ThreadEvent::MergeConflictDetected {
            change_id: change.to_string(),
            files: vec!["CreateThreadView.tsx".to_string()],
            origin: None,
        },
    )
    .await;
    assert!(
        proj.conflict_pairing_open(thread, change).await.unwrap(),
        "handing the merge prompt to the agent opens the pairing"
    );

    for mid_turn in [
        ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("cc-resolver".to_string()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
        aggregate_proposed(change, "jitter", "/r"),
    ] {
        emit(&bus, thread, mid_turn).await;
        assert!(
            proj.conflict_pairing_open(thread, change).await.unwrap(),
            "only a merge-lifecycle event may close the pairing, so the guard \
             stays armed for the whole resolution"
        );
    }

    // The resolver's own completion is what closes it, and only then may
    // another caller merge.
    emit(&bus, thread, applied_event(change, &["fix: jitter"], false)).await;
    assert!(
        !proj.conflict_pairing_open(thread, change).await.unwrap(),
        "the resolution's terminal hands the merge back"
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
    assert_eq!(
        active[0].merge_worktree().map(|m| m.path.as_str()),
        Some("/tmp/wt-1")
    );

    teardown_test_db(&db).await;
}

fn idled() -> ThreadEvent {
    ThreadEvent::CodingAgentIdled {
        has_changes: true,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
    }
}

/// The flags a reader sees for one change, next to the status `threads list`
/// shows for its thread.
async fn read_one(pool: &PgPool, proj: &ChangesProjection, thread: Uuid) -> (String, bool, bool) {
    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread)
            .fetch_one(pool)
            .await
            .unwrap();
    let pending = crate::core::changes::list_pending_for_readers(
        pool,
        proj,
        crate::core::changes::PendingScope::All,
    )
    .await
    .unwrap();
    let change = pending
        .iter()
        .find(|c| c.thread_id == Some(thread))
        .expect("the change is pending");
    let thread = change.thread_state().unwrap();
    (status, thread.unsettled(), thread.settling())
}

/// A session proposed, then an event delivery started a new turn and it kept
/// working. The `changes` tool read that change as settled, and an
/// orchestrator told the user the session was done. Driven by real events, so
/// the flag has to follow the status every step of the way.
#[tokio::test]
async fn a_change_reads_unsettled_whenever_its_thread_works_again() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let proj = ChangesProjection::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_thread(&bus, thread).await;
    emit(&bus, thread, idled()).await;
    emit(
        &bus,
        thread,
        aggregate_proposed(Uuid::new_v4(), "b-resume", "/r"),
    )
    .await;

    assert_eq!(
        read_one(&pool, &proj, thread).await,
        ("idle".to_string(), false, false),
        "an idle thread with no wait has finished with its change"
    );

    emit(
        &bus,
        thread,
        ThreadEvent::UserPromptInjected {
            text: "the background task finished".into(),
            mode: crate::engine::thread_events::ActorMode::Agent,
            origin: None,
            injected_message_id: None,
            delivered_event_id: None,
        },
    )
    .await;
    assert_eq!(
        read_one(&pool, &proj, thread).await,
        ("running".to_string(), true, true),
        "a thread that resumes after proposing is still working on the change"
    );

    emit(&bus, thread, idled()).await;
    assert_eq!(
        read_one(&pool, &proj, thread).await,
        ("idle".to_string(), false, false),
        "once it goes idle again, the change reads settled"
    );

    teardown_test_db(&db).await;
}

/// The list and the Apply gate read the same state. At 21:14 an Apply All hit
/// a conflict and the thread went back to work resolving it. The `changes`
/// tool then listed the change with every flag false, while `apply` on the same
/// change was refused because the thread had not finished.
#[tokio::test]
async fn the_list_and_the_apply_gate_agree_on_every_thread_state() {
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let proj = ChangesProjection::new(pool.clone());

    let states = [
        ("mid-turn", "running", 0, false),
        ("on a question card", "waiting_for_user_answer", 0, false),
        ("watching an event", "idle", 1, false),
        ("resolving a conflict", "running", 0, true),
        ("settled", "idle", 0, false),
    ];
    let mut rows = Vec::new();
    for (i, (label, status, waits, conflict)) in states.into_iter().enumerate() {
        let (thread, change) = (Uuid::new_v4(), Uuid::new_v4());
        start_cc_thread(&bus, thread).await;
        emit(&bus, thread, idled()).await;
        emit(
            &bus,
            thread,
            aggregate_proposed(change, &format!("agree-{i}"), "/r"),
        )
        .await;
        if conflict {
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
        }
        sqlx::query(
            "UPDATE thread_summaries SET status = $2, live_event_wait_count = $3 \
             WHERE thread_id = $1",
        )
        .bind(thread)
        .bind(status)
        .bind(waits)
        .execute(&pool)
        .await
        .unwrap();
        rows.push((label, change, conflict));
    }

    let listed = crate::core::changes::list_pending_for_readers(
        &pool,
        &proj,
        crate::core::changes::PendingScope::All,
    )
    .await
    .unwrap();
    for (label, change, conflict) in rows {
        let row = listed.iter().find(|c| c.id == change).expect("listed");
        let refused = crate::api::changes::change_action_refusal(
            &pool,
            change,
            crate::engine::thread_lifecycle::Action::Apply,
        )
        .await
        .unwrap();
        assert_eq!(
            row.thread_state().unwrap().unsettled(),
            refused.is_some(),
            "{label}: the list says unsettled={}, the gate says {refused:?}",
            row.thread_state().unwrap().unsettled()
        );
        assert_eq!(
            row.thread_state().unwrap().resolving_conflict(),
            conflict,
            "{label}: resolving flag"
        );
    }

    teardown_test_db(&db).await;
}

/// The threads-list count and the `changes` filter read one definition of
/// "sub-thread", so over the same tree they agree. The root's own change is in
/// neither: a completion card keeps it apart too.
#[tokio::test]
async fn the_sub_thread_count_and_the_filter_agree_on_one_tree() {
    use crate::core::changes::{list_pending_for_readers, PendingScope};
    let (pool, db) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let proj = ChangesProjection::new(pool.clone());

    // root holds its own change; child and grandchild hold one each; the
    // quiet sibling holds none.
    let [root, child, grandchild, quiet] = [(); 4].map(|_| Uuid::new_v4());
    for (i, thread) in [root, child, grandchild, quiet].into_iter().enumerate() {
        start_cc_thread(&bus, thread).await;
        if thread != quiet {
            emit(
                &bus,
                thread,
                aggregate_proposed(Uuid::new_v4(), &format!("tree-{i}"), "/r"),
            )
            .await;
        }
    }
    for (thread, parent) in [(child, root), (grandchild, child), (quiet, root)] {
        sqlx::query("UPDATE thread_summaries SET parent_thread_id = $2 WHERE thread_id = $1")
            .bind(thread)
            .bind(parent)
            .execute(&pool)
            .await
            .unwrap();
    }

    let below_root = list_pending_for_readers(&pool, &proj, PendingScope::SubThreadsOf(root))
        .await
        .unwrap();
    let mut owners: Vec<_> = below_root.iter().filter_map(|c| c.thread_id).collect();
    owners.sort();
    let mut expected = vec![child, grandchild];
    expected.sort();
    assert_eq!(
        owners, expected,
        "the filter reaches the grandchild, not the root"
    );

    let counts =
        crate::core::changes::pending_sub_thread_change_counts(&pool, &[root, child, quiet])
            .await
            .unwrap();
    assert_eq!(counts.get(&root).copied(), Some(below_root.len() as i64));
    assert_eq!(counts.get(&child).copied(), Some(1));
    assert_eq!(counts.get(&quiet), None, "nothing below the quiet sibling");

    // The list an agent reads carries the count on every row, zero included.
    let store = crate::core::store::EventStore::new(pool.clone());
    let rows = store
        .list_thread_summaries(crate::core::store::ThreadSummaryFilters {
            status: crate::core::store::StatusFilter::Any,
            sources: None,
            parent: None,
            limit: 100,
        })
        .await
        .unwrap();
    let count_of = |id: Uuid| {
        rows.iter()
            .find(|r| r.thread_id == id.to_string())
            .expect("listed")
            .pending_sub_thread_change_count
    };
    assert_eq!(count_of(root), Some(2));
    assert_eq!(count_of(grandchild), Some(0));

    // Every other read path omits it rather than claiming zero.
    let by_id = store.get_threads_by_ids(&[root.to_string()]).await.unwrap();
    assert_eq!(by_id[0].pending_sub_thread_change_count, None);

    teardown_test_db(&db).await;
}
