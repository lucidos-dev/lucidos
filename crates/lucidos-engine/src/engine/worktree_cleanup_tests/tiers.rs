use super::common::*;
use crate::test_support::{setup_test_db, teardown_test_db};
use crate::engine::event_bus::EventBus;
use crate::engine::git_ops::{git_cmd, worktrees_dir};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn tier_1_strips_build_artifacts_after_24h_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, true).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_1_AGE).await;

    // Pre-conditions
    assert!(worktree.join("target").exists());
    assert!(worktree.join("node_modules").exists());
    assert!(worktree.join(".lucidos/cache").exists());

    let rx = bus.subscribe();
    // Cleanup is now disk-gated: with ample disk and a non-archived thread the
    // worktree is kept fully warm. Put the worker under soft pressure so the
    // routine 24 h Tier 1 strip is eligible (soft only — hard stays 0 so the
    // 24 h window doesn't widen to 1 h).
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert_eq!(cleaned.len(), 1, "exactly one Tier 1 event");
    let (_, tier, freed, branch_deleted) = cleaned[0];
    assert_eq!(tier, 1);
    assert!(freed > 0, "expected non-zero freed bytes, got {}", freed);
    assert!(!branch_deleted, "Tier 1 must not delete branches");

    // Worktree itself stays; only artifacts disappear.
    assert!(worktree.exists(), "worktree dir must remain after Tier 1");
    assert!(!worktree.join("target").exists(), "target/ should be gone");
    assert!(
        !worktree.join("node_modules").exists(),
        "node_modules/ should be gone"
    );
    assert!(
        !worktree.join(".lucidos/cache").exists(),
        ".lucidos/cache should be gone"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tier_2_removes_worktree_after_30_days_clean_unsaved() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, false).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_2_AGE).await;

    let rx = bus.subscribe();
    // Full removal is disk-gated now — drive it via soft pressure.
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert_eq!(cleaned.len(), 1, "exactly one Tier 2 event");
    let (_, tier, _, _) = cleaned[0];
    assert_eq!(tier, 2);
    assert!(!worktree.exists(), "Tier 2 must remove worktree dir");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn saved_threads_are_exempt_from_tier_2() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, false).await;
    insert_thread_summary(&pool, thread_id, true /* saved */).await;
    insert_old_event(&pool, thread_id, TIER_2_AGE).await;

    let rx = bus.subscribe();
    // Under soft pressure so the saved-thread exemption — not ample disk — is
    // the operative reason the worktree survives.
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert!(
        cleaned.iter().all(|(_, tier, _, _)| *tier != 2),
        "no Tier 2 event for a saved thread, got: {:?}",
        cleaned
    );
    assert!(worktree.exists(), "saved worktree must remain on disk");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn dirty_threads_are_exempt_from_tier_2_auto() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, false).await;
    // Make the worktree dirty by writing an uncommitted file.
    tokio::fs::write(worktree.join("uncommitted.txt"), b"hello")
        .await
        .unwrap();
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_2_AGE).await;

    let rx = bus.subscribe();
    // Under soft pressure so the dirty-worktree exemption — not ample disk — is
    // the operative reason the worktree survives.
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert!(
        cleaned.iter().all(|(_, tier, _, _)| *tier != 2),
        "no Tier 2 event for a dirty thread, got: {:?}",
        cleaned
    );
    assert!(worktree.exists(), "dirty worktree must remain on disk");
    assert!(
        worktree.join("uncommitted.txt").exists(),
        "uncommitted file must be preserved"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn soft_threshold_breach_emits_low_disk_notification() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let _wt = add_worktree_for_thread(&root, thread_id, true).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, 5).await; // recent — Tier 1/2 won't fire

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    // Force "we are under the soft threshold" by pretending the soft threshold
    // is the entire disk size. Hard stays at 0 so we don't trigger forced
    // Tier 1 in this test.
    worker.free_soft_bytes = u64::MAX;
    worker.free_hard_bytes = 0;

    let rx = bus.subscribe();
    worker.run_once().await;

    let notifications = drain_notifications(rx, Duration::from_millis(200)).await;
    let alerts: Vec<_> = notifications
        .into_iter()
        .filter(|(t, _)| t == "Low disk space on your machine")
        .collect();
    assert_eq!(alerts.len(), 1, "expected exactly one low-disk notification, got: {:?}", alerts);
    let (_, body) = &alerts[0];
    assert!(
        !body.contains("Lucidos disk space"),
        "body must not lead with the old 'Lucidos disk space' framing: {}",
        body,
    );
    assert!(
        body.contains("volume hosting"),
        "body must explicitly call out the volume, not Lucidos itself: {}",
        body,
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Small Lucidos footprint + low volume → message must steer the user to look
/// elsewhere on their machine, not at Lucidos.
#[tokio::test]
async fn soft_threshold_with_tiny_lucidos_footprint_blames_the_machine() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    // No worktrees on disk → Lucidos footprint is 0.
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.free_hard_bytes = 0;

    let rx = bus.subscribe();
    worker.run_once().await;

    let notifications = drain_notifications(rx, Duration::from_millis(200)).await;
    let alerts: Vec<_> = notifications
        .into_iter()
        .filter(|(t, _)| t == "Low disk space on your machine")
        .collect();
    assert_eq!(alerts.len(), 1);
    let (_, body) = &alerts[0];
    assert!(
        body.contains("other apps") || body.contains("not Lucidos") || body.contains("isn't Lucidos"),
        "tiny-footprint body must point at the user's machine, not Lucidos: {}",
        body,
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Large Lucidos footprint + low volume → message steers to Settings → Disk Usage.
#[tokio::test]
async fn soft_threshold_with_large_lucidos_footprint_suggests_cleanup() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let _wt = add_worktree_for_thread(&root, thread_id, true).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, 5).await;

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.free_hard_bytes = 0;
    // Boundary at 1 byte forces the "large footprint" branch — the planted
    // worktree's artifacts comfortably exceed that without us having to write
    // gigabytes to disk.
    worker.large_footprint_bytes = 1;

    let rx = bus.subscribe();
    worker.run_once().await;

    let notifications = drain_notifications(rx, Duration::from_millis(200)).await;
    let alerts: Vec<_> = notifications
        .into_iter()
        .filter(|(t, _)| t == "Low disk space on your machine")
        .collect();
    assert_eq!(alerts.len(), 1);
    let (_, body) = &alerts[0];
    assert!(
        body.contains("Settings") && body.contains("Disk Usage"),
        "large-footprint body must point at Settings → Disk Usage: {}",
        body,
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn hard_threshold_breach_forces_tier_1_on_recently_idle_worktrees() {
    // Below the hard threshold, Tier 1 widens from 24 h to 1 h. A worktree
    // idle for 90 minutes should get its build artifacts stripped despite
    // not crossing the normal 24 h Tier 1 line.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, true).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, 90 * 60).await; // 90 minutes idle

    assert!(worktree.join("target").exists());

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_hard_bytes = u64::MAX; // force hard-threshold path
    worker.free_soft_bytes = u64::MAX; // also under soft (which is implicit)

    let rx = bus.subscribe();
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert_eq!(cleaned.len(), 1, "Tier 1 should have fired under hard pressure");
    assert_eq!(cleaned[0].1, 1, "must be Tier 1, not Tier 2");
    assert!(!worktree.join("target").exists(), "target/ should be stripped");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn hard_threshold_does_not_force_tier_1_on_active_worktrees() {
    // Even with hard pressure, a worktree active in the last hour stays
    // untouched (FORCE_TIER_1_IDLE = 1 h floor).
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, true).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, 5 * 60).await; // 5 minutes idle — active

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_hard_bytes = u64::MAX;
    worker.free_soft_bytes = u64::MAX;

    let rx = bus.subscribe();
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert!(cleaned.is_empty(), "active worktree must not be touched: {:?}", cleaned);
    assert!(worktree.join("target").exists(), "target/ must survive — only 5 min idle");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn ample_free_disk_emits_no_notification() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let _wt = add_worktree_for_thread(&root, thread_id, true).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, 5).await;

    // make_worker defaults: free_soft/hard = 0 → never trigger
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());

    let rx = bus.subscribe();
    worker.run_once().await;

    let notifications = drain_notifications(rx, Duration::from_millis(200)).await;
    let alerts: Vec<_> = notifications
        .into_iter()
        .filter(|(t, _)| t.contains("disk") || t.contains("Disk") || t.contains("auto-cleanup"))
        .collect();
    assert!(alerts.is_empty(), "no disk-related notifications when free disk is ample, got: {:?}", alerts);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tier_0_deletes_branch_if_fully_merged() {
    // After Tier 0 was introduced, a fully-merged branch (no commits ahead
    // of main, clean worktree, no pending change) is removed by Tier 0 long
    // before Tier 2's 30d window — but the destructive call is shared, so
    // `branch_deleted=true` semantics still hold. This test pins both:
    // Tier 0 fires AND deletes the merged branch.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    let short = &thread_id.simple().to_string()[..8];
    let branch = format!("test/{}", short);

    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_2_AGE).await;

    let rx = bus.subscribe();
    // Full removal is disk-gated — drive it via soft pressure.
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert_eq!(cleaned.len(), 1, "Tier 0 should fire on fully-merged worktree");
    let (_, tier, _, branch_deleted) = cleaned[0];
    assert_eq!(tier, 0, "should be reclaimed by Tier 0, not Tier 2");
    assert!(branch_deleted, "fully-merged branch must be deleted");

    let res = git_cmd(&["rev-parse", "--verify", &branch], &root).await;
    let exists = matches!(res, Ok(o) if o.status.success());
    assert!(!exists, "branch {} must no longer exist", branch);
    let _ = worktree;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tier_2_preserves_branch_if_unmerged_commits_exist() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, false).await;
    let short = &thread_id.simple().to_string()[..8];
    let branch = format!("test/{}", short);

    // Add a commit on the branch so it diverges from main.
    tokio::fs::write(worktree.join("feature.txt"), b"feat")
        .await
        .unwrap();
    git_cmd(&["add", "."], &worktree).await.unwrap();
    git_cmd(&["commit", "-m", "feature"], &worktree)
        .await
        .unwrap();

    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_2_AGE).await;

    let rx = bus.subscribe();
    // Full removal is disk-gated — drive it via soft pressure.
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert_eq!(cleaned.len(), 1, "Tier 2 should fire");
    let (_, _, _, branch_deleted) = cleaned[0];
    assert!(
        !branch_deleted,
        "branch with unmerged commits must NOT be deleted"
    );

    let res = git_cmd(&["rev-parse", "--verify", &branch], &root).await;
    let exists = matches!(res, Ok(o) if o.status.success());
    assert!(
        exists,
        "branch {} must still exist for data preservation",
        branch
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn legacy_random_suffix_worktrees_are_skipped() {
    // Anything in `.lucidos/worktrees/` whose name doesn't match the
    // `thread-<8-hex>` shape is left alone — Phase 6.1 only stamps deterministic
    // names for new threads, and we have no way to map a random suffix back.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    // Create a legacy random-suffix dir directly in the worktrees folder.
    let legacy = worktrees_dir(&root).join("cc-random-suffix-12345");
    tokio::fs::create_dir_all(&legacy).await.unwrap();
    tokio::fs::write(legacy.join("file.txt"), b"legacy")
        .await
        .unwrap();

    let rx = bus.subscribe();
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.run_once().await;
    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    assert!(
        events.is_empty(),
        "legacy worktrees must not produce WorktreeCleaned events: {:?}",
        events
    );
    assert!(legacy.exists(), "legacy worktree dir must remain untouched");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn recent_threads_are_exempt() {
    // Thread with a fresh event (5 seconds ago) must not be touched even if
    // build artifacts and a clean tree are present.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, true).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, 5).await;

    let rx = bus.subscribe();
    // Under soft pressure so recency — not ample disk — is the operative
    // exemption for this recently-active thread.
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.free_soft_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert!(
        cleaned.is_empty(),
        "no cleanup should happen for a recently-active thread, got: {:?}",
        cleaned
    );
    assert!(worktree.join("target").exists(), "target/ must survive");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Core retention fix: while free disk is comfortable, a NON-archived thread's
/// worktree is kept even when it is fully merged + clean + long idle — the exact
/// state Tier 0 used to reclaim an hour after idle. Reopening the thread then
/// reuses the warm worktree instead of paying a cold rebuild. (The whole
/// "worktree torn down" incident started here.)
#[tokio::test]
async fn ample_disk_keeps_non_archived_merged_worktree() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    let short = &thread_id.simple().to_string()[..8];
    let branch = format!("test/{}", short);

    // Non-archived ('inbox'), idle well past the 30-day Tier 2 window.
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_2_AGE).await;

    let rx = bus.subscribe();
    // make_worker defaults: free_soft/hard = 0 → disk is "comfortable".
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert!(
        cleaned.is_empty(),
        "non-archived worktree must be kept while disk is comfortable, got: {:?}",
        cleaned
    );
    assert!(worktree.exists(), "worktree dir must remain on disk");
    let res = git_cmd(&["rev-parse", "--verify", &branch], &root).await;
    assert!(
        matches!(res, Ok(o) if o.status.success()),
        "branch {} must be preserved",
        branch
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Archive-aware reclaim: once the user ARCHIVES a thread (signals "done"), its
/// worktree is reclaimed even with comfortable disk — archiving is the explicit
/// "I'm finished with this" lever.
#[tokio::test]
async fn archived_thread_worktree_reclaimed_with_ample_disk() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;

    // Archived + fully merged + idle past the Tier 0 grace.
    insert_thread_summary_with_archive(&pool, thread_id, false, "archived").await;
    insert_old_event(&pool, thread_id, TIER_2_AGE).await;

    let rx = bus.subscribe();
    // Comfortable disk (defaults) — the archive flag alone drives reclaim.
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert_eq!(
        cleaned.len(),
        1,
        "an archived thread's worktree must be reclaimed even with ample disk"
    );
    assert_eq!(cleaned[0].1, 0, "fully-merged archived worktree → Tier 0");
    assert!(!worktree.exists(), "archived worktree dir must be removed");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Fan-in retention (ADR 0011, B2): a parent whose latest event is an
/// UNPROCESSED `ChildThreadCompleted` must keep its worktree even when archived +
/// fully merged + long idle — the exact state the companion
/// `archived_thread_worktree_reclaimed_with_ample_disk` test proves IS reclaimed
/// without the obligation. Removing it would leave the parent with nothing to
/// resume into when it reacts to the child completion (the `276f5580` incident).
#[tokio::test]
async fn fan_in_unprocessed_completion_keeps_archived_worktree() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, parent_id).await;

    // Archived + fully merged + idle past Tier 0 grace — the reclaim gate is OPEN.
    insert_thread_summary_with_archive(&pool, parent_id, false, "archived").await;
    insert_old_event(&pool, parent_id, TIER_2_AGE).await;
    // …but a child completed and the parent never processed it: the
    // ChildThreadCompleted is the parent's latest event (inserted last → highest
    // sequence), backdated so the thread still reads as idle.
    insert_child_completed_event(&pool, parent_id, child_id, TIER_2_AGE).await;

    let rx = bus.subscribe();
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == parent_id).collect();
    assert!(
        cleaned.is_empty(),
        "parent with an unprocessed child completion must keep its worktree, got: {:?}",
        cleaned
    );
    assert!(
        worktree.exists(),
        "worktree must remain — the parent still owes a fan-in resume"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Fan-in retention (ADR 0011, B2): a parent with a direct child still running
/// (`active_children_count > 0`) keeps its worktree even with the reclaim gate
/// open — it will resume when the child finishes.
#[tokio::test]
async fn fan_in_active_children_keeps_archived_worktree() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let parent_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, parent_id).await;

    insert_thread_summary_with_archive(&pool, parent_id, false, "archived").await;
    insert_old_event(&pool, parent_id, TIER_2_AGE).await;
    // A direct child is still running.
    set_active_children_count(&pool, parent_id, 1).await;

    let rx = bus.subscribe();
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == parent_id).collect();
    assert!(
        cleaned.is_empty(),
        "parent with a running child must keep its worktree, got: {:?}",
        cleaned
    );
    assert!(worktree.exists(), "worktree must remain while a child is running");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Even build-artifact stripping (Tier 1) waits for disk pressure now: with
/// comfortable disk and a non-archived thread, a day-idle worktree keeps its
/// `target/`/`node_modules/` so the next reopen is fully warm.
#[tokio::test]
async fn ample_disk_keeps_non_archived_worktree_artifacts() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, true).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_1_AGE).await; // > 24h idle

    let rx = bus.subscribe();
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert!(
        cleaned.is_empty(),
        "no stripping while disk is comfortable + thread non-archived, got: {:?}",
        cleaned
    );
    assert!(worktree.join("target").exists(), "target/ must be kept warm");
    assert!(
        worktree.join("node_modules").exists(),
        "node_modules/ must be kept warm"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

