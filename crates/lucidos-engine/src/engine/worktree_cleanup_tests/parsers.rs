use super::common::*;
use crate::engine::event_bus::EventBus;
use crate::test_support::{setup_test_db, teardown_test_db};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[test]
fn parse_thread_short_recognises_deterministic_shape() {
    use super::parse_thread_short;
    assert_eq!(
        parse_thread_short("thread-01234567").as_deref(),
        Some("01234567")
    );
    assert_eq!(
        parse_thread_short("thread-deadbeef").as_deref(),
        Some("deadbeef"),
        "all hex chars accepted"
    );
}

#[test]
fn parse_thread_short_rejects_legacy_and_garbage_names() {
    use super::parse_thread_short;
    assert!(parse_thread_short("thread-").is_none());
    assert!(parse_thread_short("thread-XYZNOTHEX").is_none());
    assert!(parse_thread_short("cc-random-suffix-12345").is_none());
    assert!(
        parse_thread_short("thread-deadbeefcafe").is_none(),
        "wrong length"
    );
    assert!(parse_thread_short("not-a-thread").is_none());
}

#[test]
fn is_safe_subpath_blocks_escapes() {
    use super::is_safe_subpath;
    let parent = std::env::temp_dir();
    let child = parent.join("inner");
    std::fs::create_dir_all(&child).ok();
    assert!(is_safe_subpath(&parent, &child));
    assert!(
        !is_safe_subpath(&parent, &parent),
        "child equal to parent is not a strict subpath"
    );
    assert!(
        !is_safe_subpath(&parent, Path::new("/")),
        "root must not be considered a safe child"
    );
}

/// Standalone helper used by both the background worker (Tier 1) and the
/// disk-usage settings page (on-demand cleanup). Test it through the
/// public surface so a regression in either consumer surfaces here.
#[tokio::test]
async fn prune_build_artifacts_strips_target_node_modules_cache() {
    use super::prune_build_artifacts;

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, true).await;

    // Pre: artifacts present
    assert!(worktree.join("target").exists());
    assert!(worktree.join("node_modules").exists());
    assert!(worktree.join(".lucidos/cache").exists());

    let freed = prune_build_artifacts(&worktree).expect("expected non-zero prune");
    assert!(freed > 0, "expected non-zero freed bytes, got {}", freed);

    // Post: artifacts gone, worktree itself stays.
    assert!(worktree.exists());
    assert!(!worktree.join("target").exists());
    assert!(!worktree.join("node_modules").exists());
    assert!(!worktree.join(".lucidos/cache").exists());

    // Second run is a no-op (returns None) — nothing left to prune.
    assert!(
        prune_build_artifacts(&worktree).is_none(),
        "second prune must be a no-op when there's nothing left"
    );
}

#[tokio::test]
async fn inventory_worktrees_returns_thread_metadata_sorted_by_size() {
    use super::inventory_worktrees;

    let (pool, db_name) = setup_test_db().await;
    let (_tmp, root) = fresh_workspace().await;

    // Two worktrees: thread A is bigger than thread B. Tier-1 artifacts give
    // us a measurable size difference without needing to compute exact bytes.
    let big_id = Uuid::new_v4();
    let big_wt = add_worktree_for_thread(&root, big_id, true).await;
    tokio::fs::write(big_wt.join("target/extra.bin"), vec![0u8; 32 * 1024])
        .await
        .unwrap();
    insert_thread_summary(&pool, big_id, false).await;
    insert_old_event(&pool, big_id, 60).await;

    let small_id = Uuid::new_v4();
    let _small_wt = add_worktree_for_thread(&root, small_id, false).await;
    insert_thread_summary(&pool, small_id, true /* saved */).await;
    insert_old_event(&pool, small_id, 60).await;

    let rows = inventory_worktrees(&pool, &root).await;
    assert!(
        rows.len() >= 2,
        "expected at least 2 rows, got {}",
        rows.len()
    );

    let big_idx = rows
        .iter()
        .position(|r| r.thread_id == big_id)
        .expect("big thread inventory row");
    let small_idx = rows
        .iter()
        .position(|r| r.thread_id == small_id)
        .expect("small thread inventory row");

    assert!(
        big_idx < small_idx,
        "rows must be sorted by size desc; big idx {} should be before small idx {}",
        big_idx,
        small_idx
    );

    let small = &rows[small_idx];
    assert!(small.is_saved, "saved flag must be carried through");

    let big = &rows[big_idx];
    assert!(!big.is_saved, "unsaved flag must be carried through");
    assert!(big.size_bytes > small.size_bytes);
    assert!(big.last_activity.is_some());
    assert!(
        big.thread_title.is_some(),
        "thread_title must be carried through"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn available_disk_bytes_returns_some_for_existing_path() {
    use super::available_disk_bytes;
    let tmp = std::env::temp_dir();
    let bytes = available_disk_bytes(&tmp);
    assert!(
        bytes.is_some(),
        "should return Some for an existing tempdir"
    );
    assert!(
        bytes.unwrap() > 0,
        "free space must be > 0 on a healthy host"
    );
}

#[test]
fn available_disk_bytes_returns_none_for_missing_path() {
    use super::available_disk_bytes;
    let bogus = Path::new("/this/path/does/not/exist/lucidos-test");
    assert!(available_disk_bytes(bogus).is_none());
}

#[tokio::test]
async fn soft_threshold_does_not_re_emit_on_subsequent_ticks() {
    // Crossing into "low disk" should fire the heads-up notification once.
    // While the user stays below soft (multiple ticks), no spam.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX; // force "below soft"
    worker.free_hard_bytes = 0;

    let rx = bus.subscribe();
    worker.run_once().await;
    worker.run_once().await;
    worker.run_once().await;

    let notifications = drain_notifications(rx, Duration::from_millis(200)).await;
    let lows: Vec<_> = notifications
        .into_iter()
        .filter(|(t, _)| t == "Low disk space on your machine")
        .collect();
    assert_eq!(
        lows.len(),
        1,
        "expected exactly one low-disk notification across 3 ticks, got: {:?}",
        lows
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn soft_threshold_re_arms_after_recovery() {
    // After disk recovers above soft, sinking below should notify again.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX; // below soft
    worker.free_hard_bytes = 0;

    let rx = bus.subscribe();
    worker.run_once().await;

    worker.free_soft_bytes = 0; // recovered above soft
    worker.run_once().await;

    worker.free_soft_bytes = u64::MAX; // sinking again
    worker.run_once().await;

    let notifications = drain_notifications(rx, Duration::from_millis(200)).await;
    let lows: Vec<_> = notifications
        .into_iter()
        .filter(|(t, _)| t == "Low disk space on your machine")
        .collect();
    assert_eq!(
        lows.len(),
        2,
        "expected one notif per crossing into below-soft, got: {:?}",
        lows
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn hard_threshold_emits_auto_cleanup_when_bytes_freed() {
    // Below hard pressure, when forced Tier 1 actually reclaims space, the
    // user gets a distinct "auto-cleanup running" notification reporting
    // the freed bytes.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let _wt = add_worktree_for_thread(&root, thread_id, true).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, 90 * 60).await; // 90 min — forced Tier 1 will fire

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_hard_bytes = u64::MAX;
    worker.free_soft_bytes = u64::MAX;

    let rx = bus.subscribe();
    worker.run_once().await;

    let notifications = drain_notifications(rx, Duration::from_millis(200)).await;
    let cleanups: Vec<_> = notifications
        .into_iter()
        .filter(|(t, _)| t == "Lucidos reclaimed disk space")
        .collect();
    assert_eq!(
        cleanups.len(),
        1,
        "expected an auto-cleanup notification when forced Tier 1 freed bytes, got: {:?}",
        cleanups
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn hard_threshold_no_auto_cleanup_when_nothing_freed() {
    // Below hard pressure but no idle worktrees to reclaim → no auto-cleanup
    // notification (the soft-low heads-up still fires, but the action notif
    // shouldn't claim "running" when nothing actually ran).
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_hard_bytes = u64::MAX;
    worker.free_soft_bytes = u64::MAX;

    let rx = bus.subscribe();
    worker.run_once().await;

    let notifications = drain_notifications(rx, Duration::from_millis(200)).await;
    let cleanups: usize = notifications
        .iter()
        .filter(|(t, _)| t == "Lucidos reclaimed disk space")
        .count();
    assert_eq!(
        cleanups, 0,
        "auto-cleanup notification must not fire when nothing was reclaimed; saw: {:?}",
        notifications
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// Tier 0: applied/clean fast removal.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tier_0_removes_clean_worktree_with_no_commits_after_grace() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_0_AGE_SECS).await;

    let rx = bus.subscribe();
    // Reclaim is disk-gated now (a non-archived worktree is kept while disk is
    // comfortable) — drive Tier 0 via soft pressure.
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events
        .into_iter()
        .filter(|(t, ..)| *t == thread_id)
        .collect();
    assert_eq!(cleaned.len(), 1, "exactly one Tier 0 event");
    let (_, tier, _, branch_deleted) = cleaned[0];
    assert_eq!(tier, 0);
    assert!(branch_deleted, "Tier 0 must delete the merged branch");
    assert!(!worktree.exists(), "Tier 0 must remove the worktree dir");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tier_0_skips_branch_with_commits_ahead_of_main() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    // `add_worktree_for_thread` adds a commit, so the branch IS ahead of main.
    let worktree = add_worktree_for_thread(&root, thread_id, false).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_0_AGE_SECS).await;

    let rx = bus.subscribe();
    // Under soft pressure so the commits-ahead skip — not ample disk — is the
    // operative reason Tier 0 doesn't fire.
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events
        .into_iter()
        .filter(|(t, ..)| *t == thread_id)
        .collect();
    assert!(
        cleaned.iter().all(|(_, tier, _, _)| *tier != 0),
        "no Tier 0 event when branch has commits ahead, got: {:?}",
        cleaned
    );
    assert!(worktree.exists(), "worktree with commits must remain");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tier_0_respects_one_hour_grace_window() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    insert_thread_summary(&pool, thread_id, false).await;
    // 30 min — within the 1h grace.
    insert_old_event(&pool, thread_id, 30 * 60).await;

    let rx = bus.subscribe();
    // Under soft pressure so the 1 h grace — not ample disk — is the operative
    // reason Tier 0 doesn't fire yet.
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events
        .into_iter()
        .filter(|(t, ..)| *t == thread_id)
        .collect();
    assert!(
        cleaned.iter().all(|(_, tier, _, _)| *tier != 0),
        "no Tier 0 event within grace, got: {:?}",
        cleaned
    );
    assert!(
        worktree.exists(),
        "worktree must remain within grace window"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tier_0_fires_within_grace_under_disk_pressure() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    insert_thread_summary(&pool, thread_id, false).await;
    // 30s — well within the 1h grace.
    insert_old_event(&pool, thread_id, 30).await;

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    // Force real disk pressure: the volume sits below BOTH thresholds (in
    // production hard < soft, so under_hard implies under_soft). Soft opens the
    // reclaim gate; hard drops the Tier 0 grace to zero so it fires immediately.
    worker.free_soft_bytes = u64::MAX;
    worker.free_hard_bytes = u64::MAX;

    let rx = bus.subscribe();
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events
        .into_iter()
        .filter(|(t, ..)| *t == thread_id)
        .collect();
    assert_eq!(cleaned.len(), 1, "exactly one Tier 0 event under pressure");
    let (_, tier, _, _) = cleaned[0];
    assert_eq!(tier, 0);
    assert!(
        !worktree.exists(),
        "Tier 0 must remove worktree under pressure even within grace"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tier_0_skips_thread_with_pending_change() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_0_AGE_SECS).await;

    // Insert a pending change for this thread on its branch — Tier 0 must
    // skip while pending work awaits the user's decision.
    let short = &thread_id.simple().to_string()[..8];
    let branch = format!("test/{}", short);
    sqlx::query(
        "INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, status, created_at, thread_id) \
         VALUES ($1, $2, $3, $4, $5, 0, '{}'::text[], false, 'pending', NOW(), $6)",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(&branch)
    .bind(root.to_string_lossy().to_string())
    .bind("pending change for tier 0 test")
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("insert pending change");

    let rx = bus.subscribe();
    // Under soft pressure so the pending-change skip — not ample disk — is the
    // operative reason Tier 0 doesn't fire.
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events
        .into_iter()
        .filter(|(t, ..)| *t == thread_id)
        .collect();
    assert!(
        cleaned.iter().all(|(_, tier, _, _)| *tier != 0),
        "Tier 0 must skip thread with pending change, got: {:?}",
        cleaned
    );
    assert!(
        worktree.exists(),
        "worktree with pending change must remain"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: a Claude Code subprocess parked on `AskUserQuestion` keeps the
/// `agent_sessions` entry but emits no events while the user thinks. If the
/// wait crosses `TIER_0_GRACE` (1 h), tier 0 sees no pending change, a clean
/// worktree, and a branch with no commits — and `git branch -D`'s the live
/// session's branch out from under it. The next `propose_change_at_idle`
/// then runs `branch_changed_files(repo_root, branch_name)` against the now-
/// deleted ref, gets back an empty list, and silently skips proposing the
/// change. No `ChangeProposed` event ever fires; the user sees no Apply
/// button. Probing `agent_sessions` before any tier action is the fix.
#[tokio::test]
async fn tier_0_skips_thread_with_live_agent_session() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    // Same shape as the production trigger: branch at main HEAD, clean
    // worktree, no pending change. Without the fix, tier 0 would happily
    // delete the branch.
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_0_AGE_SECS).await;

    let rx = bus.subscribe();
    // Under soft pressure so the live-session skip — not ample disk — is the
    // operative reason no cleanup runs (active threads are exempt even when the
    // disk is tight enough that reclaim would otherwise be eligible).
    let mut worker = make_worker_with_active(
        pool.clone(),
        bus.clone(),
        root.clone(),
        active_threads(&[thread_id]),
    );
    worker.free_soft_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events
        .into_iter()
        .filter(|(t, ..)| *t == thread_id)
        .collect();
    assert!(
        cleaned.is_empty(),
        "no cleanup events for a thread with a live agent session, got: {:?}",
        cleaned
    );
    assert!(
        worktree.exists(),
        "worktree of an active Claude Code session must remain on disk"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// `worktree_git_admin_missing` — stranded detection (pure, no DB).
// ---------------------------------------------------------------------------
