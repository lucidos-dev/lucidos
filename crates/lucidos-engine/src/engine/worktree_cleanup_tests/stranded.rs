use super::common::*;
use crate::engine::event_bus::EventBus;
use crate::engine::git_ops::{git_cmd, worktrees_dir};
use crate::test_support::{setup_test_db, teardown_test_db};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn worktree_git_admin_missing_true_when_admin_dir_deleted() {
    use super::worktree_git_admin_missing;
    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    assert!(
        !worktree_git_admin_missing(&worktree),
        "a healthy worktree is not stranded"
    );
    strand_worktree(&worktree).await;
    assert!(
        worktree_git_admin_missing(&worktree),
        "deleting the admin dir makes the worktree stranded"
    );
}

#[tokio::test]
async fn worktree_git_admin_missing_false_when_dot_git_is_real_dir() {
    use super::worktree_git_admin_missing;
    let (_tmp, root) = fresh_workspace().await;
    // The repo root's `.git` is a real directory, not a gitdir link.
    assert!(!worktree_git_admin_missing(&root));
}

#[tokio::test]
async fn worktree_git_admin_missing_resolves_relative_gitdir() {
    use super::worktree_git_admin_missing;
    // P0 regression guard: a RELATIVE gitdir that resolves against the worktree
    // must NOT be flagged stranded — else a healthy relative-link worktree is
    // wrongly deleted.
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    tokio::fs::create_dir_all(wt.join("admin")).await.unwrap();
    tokio::fs::write(wt.join(".git"), "gitdir: ./admin\n")
        .await
        .unwrap();
    assert!(
        !worktree_git_admin_missing(&wt),
        "a relative gitdir whose target exists is not stranded"
    );
    // A relative gitdir whose target is gone IS stranded.
    tokio::fs::write(wt.join(".git"), "gitdir: ./gone\n")
        .await
        .unwrap();
    assert!(worktree_git_admin_missing(&wt));
}

#[tokio::test]
async fn worktree_git_admin_missing_false_for_absent_or_garbage_dot_git() {
    use super::worktree_git_admin_missing;
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    tokio::fs::create_dir_all(&wt).await.unwrap();
    assert!(!worktree_git_admin_missing(&wt), "no .git → not stranded");
    tokio::fs::write(wt.join(".git"), "not a gitdir line")
        .await
        .unwrap();
    assert!(
        !worktree_git_admin_missing(&wt),
        "garbage .git content → conservative false"
    );
}

// ---------------------------------------------------------------------------
// Stranded worktrees: git admin dir gone → git-free removal.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stranded_thread_worktree_removed_after_grace() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    strand_worktree(&worktree).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_0_AGE_SECS).await; // > 1h stranded grace

    let rx = bus.subscribe();
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events
        .into_iter()
        .filter(|(t, ..)| *t == thread_id)
        .collect();
    assert_eq!(cleaned.len(), 1, "exactly one stranded-removal event");
    let (_, tier, _, branch_deleted) = cleaned[0];
    assert_eq!(tier, 2, "stranded removal reports full-removal tier 2");
    assert!(!branch_deleted, "stranded removal never deletes a branch");
    assert!(!worktree.exists(), "stranded worktree dir must be removed");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn stranded_worktree_survives_within_grace() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    strand_worktree(&worktree).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, 30 * 60).await; // 30 min < 1h grace

    let rx = bus.subscribe();
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events
        .into_iter()
        .filter(|(t, ..)| *t == thread_id)
        .collect();
    assert!(
        cleaned.is_empty(),
        "no stranded removal within grace, got: {:?}",
        cleaned
    );
    assert!(
        worktree.exists(),
        "stranded worktree must remain within grace"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn stranded_removal_does_not_accelerate_under_disk_pressure() {
    // Unlike Tier 0 / orphan-path (which drop to a 0 grace under hard pressure),
    // stranded removal keeps a fixed grace floor: git can't prove the tree is
    // information-free, so we never nuke a recently-idle stranded worktree.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    strand_worktree(&worktree).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, 30).await; // 30s idle

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_hard_bytes = u64::MAX; // force under_hard

    let rx = bus.subscribe();
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events
        .into_iter()
        .filter(|(t, ..)| *t == thread_id)
        .collect();
    assert!(
        cleaned.is_empty(),
        "stranded grace must not drop under pressure, got: {:?}",
        cleaned
    );
    assert!(
        worktree.exists(),
        "stranded worktree must survive within the fixed grace even under pressure"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn stranded_worktree_skipped_when_session_active() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    strand_worktree(&worktree).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_0_AGE_SECS).await;

    let rx = bus.subscribe();
    let worker = make_worker_with_active(
        pool.clone(),
        bus.clone(),
        root.clone(),
        active_threads(&[thread_id]),
    );
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events
        .into_iter()
        .filter(|(t, ..)| *t == thread_id)
        .collect();
    assert!(
        cleaned.is_empty(),
        "active session must be skipped even when stranded, got: {:?}",
        cleaned
    );
    assert!(worktree.exists(), "active stranded worktree must remain");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn stranded_orphan_worktree_removed_without_event() {
    // A `thread-<short>` dir whose short resolves to no thread (None arm) that
    // is also stranded → removed via the orphan path, with no WorktreeCleaned.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let orphan_id = Uuid::new_v4(); // no events inserted → lookup returns NotFound
    let worktree = add_worktree_at_main_for_thread(&root, orphan_id).await;
    strand_worktree(&worktree).await;

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.stranded_grace = Duration::ZERO; // fresh dir, bypass mtime grace

    let rx = bus.subscribe();
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    assert!(
        events.is_empty(),
        "orphan stranded removal emits no WorktreeCleaned, got: {:?}",
        events
    );
    assert!(
        !worktree.exists(),
        "stranded orphan worktree must be removed"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// Temp-worktree sweep: harden-/apply-/merge- left by crashed flows.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn orphan_temp_harden_worktree_removed_when_change_resolved() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let change_id = Uuid::new_v4();
    let branch = format!("claude-code/{}", change_id.simple());
    let worktree = add_temp_worktree(&root, "harden-", change_id, &branch).await;
    insert_change(&pool, change_id, &branch, &root, "applied").await;

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.temp_worktree_grace = Duration::ZERO; // fresh dir, bypass mtime gate

    let rx = bus.subscribe();
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    assert!(
        events.is_empty(),
        "temp sweep emits no WorktreeCleaned (keyed on change), got: {:?}",
        events
    );
    assert!(
        !worktree.exists(),
        "temp worktree of a resolved change must be removed"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn orphan_temp_merge_worktree_removed_and_temp_branch_deleted() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let change_id = Uuid::new_v4();
    let temp_branch = format!("merge-tmp/{}", change_id.simple());
    let worktree = add_temp_worktree(&root, "merge-", change_id, &temp_branch).await;
    insert_change(&pool, change_id, &temp_branch, &root, "applied").await;

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.temp_worktree_grace = Duration::ZERO;
    worker.run_once().await;

    assert!(!worktree.exists(), "merge temp worktree must be removed");
    // The temp branch was at main HEAD (no commits ahead) → fully merged → deleted.
    let res = git_cmd(&["rev-parse", "--verify", &temp_branch], &root).await;
    let exists = matches!(res, Ok(o) if o.status.success());
    assert!(!exists, "fully-merged temp branch must be deleted");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn temp_worktree_skipped_when_change_still_pending() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let change_id = Uuid::new_v4();
    let branch = format!("claude-code/{}", change_id.simple());
    let worktree = add_temp_worktree(&root, "harden-", change_id, &branch).await;
    insert_change(&pool, change_id, &branch, &root, "pending").await;

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.temp_worktree_grace = Duration::ZERO; // mtime is not the gate here

    worker.run_once().await;

    assert!(
        worktree.exists(),
        "a pending change's temp worktree must survive — it may be retried"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn temp_worktree_survives_within_grace() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let change_id = Uuid::new_v4();
    let branch = format!("claude-code/{}", change_id.simple());
    let worktree = add_temp_worktree(&root, "harden-", change_id, &branch).await;
    insert_change(&pool, change_id, &branch, &root, "applied").await;

    // Default temp_worktree_grace (2h); the fresh dir is well within it.
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.run_once().await;

    assert!(
        worktree.exists(),
        "fresh temp worktree must survive within mtime grace"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn temp_worktree_skipped_when_dirty() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let change_id = Uuid::new_v4();
    let branch = format!("claude-code/{}", change_id.simple());
    let worktree = add_temp_worktree(&root, "harden-", change_id, &branch).await;
    insert_change(&pool, change_id, &branch, &root, "applied").await;
    tokio::fs::write(worktree.join("uncommitted.txt"), b"wip")
        .await
        .unwrap();

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.temp_worktree_grace = Duration::ZERO;
    worker.run_once().await;

    assert!(worktree.exists(), "dirty temp worktree must survive");
    assert!(
        worktree.join("uncommitted.txt").exists(),
        "uncommitted work must be preserved"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn cc_worktree_not_treated_as_temp() {
    // `cc-<uuid>` recovery worktrees must NOT be swept by the temp logic — they
    // fall through to the legacy skip even with a matching change row.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let id = Uuid::new_v4();
    let dir = worktrees_dir(&root).join(format!("cc-{}", id.simple()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(dir.join("file.txt"), b"x").await.unwrap();
    insert_change(&pool, id, "irrelevant", &root, "applied").await;

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.temp_worktree_grace = Duration::ZERO;
    worker.run_once().await;

    assert!(
        dir.exists(),
        "cc- worktree must be left untouched (legacy skip, not a temp prefix)"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
