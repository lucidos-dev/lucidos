use super::common::*;
use crate::engine::event_bus::EventBus;
use crate::engine::git_ops::git_cmd;
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

    let freed = prune_build_artifacts(&worktree)
        .await
        .expect("expected non-zero prune");
    assert!(freed > 0, "expected non-zero freed bytes, got {}", freed);

    // Post: artifacts gone, worktree itself stays.
    assert!(worktree.exists());
    assert!(!worktree.join("target").exists());
    assert!(!worktree.join("node_modules").exists());
    assert!(!worktree.join(".lucidos/cache").exists());

    // Second run is a no-op (returns None) — nothing left to prune.
    assert!(
        prune_build_artifacts(&worktree).await.is_none(),
        "second prune must be a no-op when there's nothing left"
    );
}

/// A repo can commit a directory named like a build output. Its edits are
/// source, so the prune leaves it and still strips the untracked ones.
#[tokio::test]
async fn prune_build_artifacts_keeps_a_directory_holding_tracked_files() {
    use super::prune_build_artifacts;

    let (_tmp, root) = fresh_workspace().await;
    let worktree = add_worktree_for_thread(&root, Uuid::new_v4(), true).await;
    tokio::fs::write(worktree.join("target/source.txt"), b"committed")
        .await
        .unwrap();
    git_cmd(&["add", "-f", "target/source.txt"], &worktree)
        .await
        .unwrap();
    git_cmd(&["commit", "-m", "track a file under target"], &worktree)
        .await
        .unwrap();
    tokio::fs::write(worktree.join("target/source.txt"), b"uncommitted edit")
        .await
        .unwrap();

    assert!(prune_build_artifacts(&worktree).await.is_some());
    assert_eq!(
        std::fs::read(worktree.join("target/source.txt")).unwrap(),
        b"uncommitted edit"
    );
    assert!(!worktree.join("node_modules").exists());
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

    // Two worktrees at main: one idle and finished, one whose thread is live.
    let done_id = Uuid::new_v4();
    let _done_wt = add_worktree_at_main_for_thread(&root, done_id).await;
    insert_thread_summary(&pool, done_id, false).await;
    insert_old_event(&pool, done_id, 60).await;
    let live_id = Uuid::new_v4();
    let _live_wt = add_worktree_at_main_for_thread(&root, live_id).await;
    insert_thread_summary(&pool, live_id, false).await;
    insert_old_event(&pool, live_id, 60).await;

    // A stranded tree: git cannot say what it tracks, so a strip frees nothing.
    let stranded_id = Uuid::new_v4();
    let stranded_wt = add_worktree_for_thread(&root, stranded_id, true).await;
    strand_worktree(&stranded_wt).await;
    insert_thread_summary(&pool, stranded_id, false).await;
    insert_old_event(&pool, stranded_id, 60).await;

    let rows = inventory_worktrees(&pool, &root, active_threads(&[live_id]).as_ref()).await;
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
    assert!(
        big.artifact_bytes >= 32 * 1024 && big.artifact_bytes < big.size_bytes,
        "artifact share must be measured apart from the source"
    );
    assert!(!big.is_finished, "a branch ahead of main is not finished");
    assert!(!big.is_active);
    assert!(big.last_activity.is_some());

    let live = rows
        .iter()
        .find(|r| r.thread_id == live_id)
        .expect("live thread inventory row");
    assert!(live.is_active);
    assert!(!live.is_finished, "a live tree is never reported finished");
    let done = rows
        .iter()
        .find(|r| r.thread_id == done_id)
        .expect("finished thread inventory row");
    assert!(done.is_finished);
    assert!(!done.is_active);
    let stranded = rows
        .iter()
        .find(|r| r.thread_id == stranded_id)
        .expect("stranded thread inventory row");
    assert_eq!(stranded.artifact_bytes, 0);
    assert!(!stranded.is_finished);
    assert!(
        big.thread_title.is_some(),
        "thread_title must be carried through"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn prefix_upper_bound_sorts_above_every_id_with_the_prefix() {
    use super::prefix_upper_bound;
    assert_eq!(prefix_upper_bound("4d4839e7"), "4d4839e8");
    assert_eq!(prefix_upper_bound("0123456f"), "0123456g");
    assert_eq!(prefix_upper_bound("01234569"), "0123456:");
    let id = "0123456f-ffff-ffff-ffff-ffffffffffff";
    assert!(id < prefix_upper_bound("0123456f").as_str());
}

/// The plan Postgres picks for `sql`, with sequential scans priced out so a
/// near-empty test table still shows which index the query can use. A
/// generic plan is what a prepared statement gets after its first few runs.
async fn plan_for(pool: &sqlx::PgPool, sql: &str, param_types: &str, args: &str) -> String {
    let mut tx = pool.begin().await.unwrap();
    for setup in [
        "SET LOCAL enable_seqscan = off",
        "SET LOCAL plan_cache_mode = force_generic_plan",
    ] {
        sqlx::query(setup).execute(&mut *tx).await.unwrap();
    }
    sqlx::query(&format!("PREPARE planned({param_types}) AS {sql}"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let lines: Vec<(String,)> = sqlx::query_as(&format!("EXPLAIN EXECUTE planned({args})"))
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DEALLOCATE planned")
        .execute(&mut *tx)
        .await
        .unwrap();
    lines
        .into_iter()
        .map(|(l,)| l)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every Disk Usage open and every cleanup tick runs one lookup per worktree.
/// Each must be an index probe, even under a generic plan.
#[tokio::test]
async fn short_thread_lookup_is_served_by_the_prefix_index() {
    use super::SHORT_THREAD_LOOKUP_SQL;
    let (pool, db_name) = setup_test_db().await;
    let plan = plan_for(
        &pool,
        SHORT_THREAD_LOOKUP_SQL,
        "text, text, text",
        "'4d4839e7%', '4d4839e7', '4d4839e8'",
    )
    .await;
    assert!(
        plan.contains("idx_events_thread_aggregate_id_pattern"),
        "the short-id lookup must use the prefix index, got:\n{plan}"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Unfenced, `MAX(created)` walks `idx_events_created` backwards over every
/// event in the workspace.
#[tokio::test]
async fn last_activity_reads_only_the_threads_own_events() {
    use super::THREAD_EVENT_CREATED_FENCED;
    let (pool, db_name) = setup_test_db().await;
    let plan = plan_for(
        &pool,
        &format!("SELECT MAX(created) {THREAD_EVENT_CREATED_FENCED}"),
        "uuid",
        "'4d4839e7-0000-0000-0000-000000000000'",
    )
    .await;
    assert!(
        plan.contains("Index Cond: (aggregate_id =") && !plan.contains("idx_events_created"),
        "last activity must read the thread's rows by aggregate id, got:\n{plan}"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Newest by timestamp, not by insertion: a backfill writes old-dated rows
/// late, and reading one as the newest would make a live thread look idle.
#[tokio::test]
async fn last_activity_is_the_newest_timestamp_not_the_last_insert() {
    use super::{last_activity_age, lookup_last_activity};
    let (pool, db_name) = setup_test_db().await;
    let thread_id = Uuid::new_v4();
    insert_old_event(&pool, thread_id, 60).await;
    insert_old_event(&pool, thread_id, 10 * 24 * 3600).await;

    let age = last_activity_age(&pool, thread_id)
        .await
        .expect("a thread with events has an age");
    assert!(
        age < Duration::from_secs(3600),
        "age must come from the 60 s old event, got {age:?}"
    );
    let (newest,): (chrono::DateTime<chrono::Utc>,) = sqlx::query_as(
        "SELECT created FROM events WHERE aggregate_id = $1::text ORDER BY created DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(lookup_last_activity(&pool, thread_id).await, Some(newest));

    assert!(last_activity_age(&pool, Uuid::new_v4()).await.is_none());
    assert!(lookup_last_activity(&pool, Uuid::new_v4()).await.is_none());

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

/// An empty session map with nothing registered anywhere.
fn idle_engine_probe() -> (
    super::EngineActiveThreads,
    Arc<crate::engine::agent_session::SpawnsInFlight>,
    crate::engine::tools::bash_background::BackgroundBashRegistry,
) {
    let sessions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let spawns = Arc::new(crate::engine::agent_session::SpawnsInFlight::default());
    let registry = crate::engine::tools::bash_background::BackgroundBashRegistry::new();
    let probe = super::EngineActiveThreads::new(sessions, spawns.clone(), registry.clone());
    (probe, spawns, registry)
}

/// A spawn sets up the worktree, hardlinks included, well before its session
/// registers. The spawn registry covers that window.
#[tokio::test]
async fn a_spawn_in_flight_makes_its_thread_active() {
    use super::ActiveThreads;

    let (probe, spawns, _registry) = idle_engine_probe();
    let thread_id = Uuid::new_v4();
    assert!(!probe.is_active(thread_id).await, "nothing running yet");
    {
        let _slot = spawns.enter(thread_id);
        assert!(probe.is_active(thread_id).await, "the spawn is in flight");
        assert!(
            !probe.is_active(Uuid::new_v4()).await,
            "another thread's spawn does not count"
        );
    }
    assert!(
        !probe.is_active(thread_id).await,
        "a returned spawn does not count"
    );
}

/// A background task builds in the worktree after its session ended the turn,
/// so the session map alone cannot see it.
#[tokio::test]
async fn a_running_background_task_makes_its_thread_active() {
    use super::ActiveThreads;

    let tmp = tempfile::tempdir().unwrap();
    let (probe, _spawns, registry) = idle_engine_probe();
    let thread_id = Uuid::new_v4();
    assert!(!probe.is_active(thread_id).await, "nothing running yet");

    let (task_id, _finished) = registry
        .spawn("sleep 30", 60, tmp.path(), &[], Some(thread_id), None)
        .await
        .expect("spawn");
    assert!(probe.is_active(thread_id).await, "the task is running");
    assert!(
        !probe.is_active(Uuid::new_v4()).await,
        "another thread's task does not count"
    );

    assert!(registry.kill(&task_id).await);
    assert!(
        registry
            .wait_for_finish(&task_id, Duration::from_secs(10))
            .await
    );
    assert!(
        !probe.is_active(thread_id).await,
        "a finished task does not count"
    );
}

#[test]
fn the_tier_1_idle_window_shrinks_with_each_pressure_level() {
    use super::{DiskPressure, TIER_1_IDLE};
    let forced = Duration::from_secs(60 * 60);
    let window = |free| DiskPressure::classify(Some(free), 20 * GB, 5 * GB).tier1_idle(forced);
    assert_eq!(window(50 * GB), TIER_1_IDLE, "comfortable");
    assert_eq!(window(10 * GB), forced, "soft");
    assert_eq!(window(GB), Duration::ZERO, "hard");
    assert_eq!(
        DiskPressure::classify(None, 20 * GB, 5 * GB).tier1_idle(forced),
        TIER_1_IDLE,
        "a failed probe reads as comfortable"
    );
}

#[tokio::test]
async fn hard_threshold_emits_auto_cleanup_when_bytes_freed() {
    // Below hard pressure, when Tier 1 actually reclaims space, the user gets
    // a distinct "auto-cleanup running" notification reporting the freed
    // bytes.
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
        .filter(|n| n.title == "Lucidos reclaimed disk space")
        .collect();
    assert_eq!(
        cleanups.len(),
        1,
        "expected an auto-cleanup notification when forced Tier 1 freed bytes, got: {:?}",
        cleanups
    );
    // The sibling alert deep-links to the same page, so this one must too.
    assert_eq!(cleanups[0].settings_view(), Some("disk-usage"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn hard_threshold_no_auto_cleanup_when_nothing_freed() {
    // Below hard pressure but no idle worktrees to reclaim → no auto-cleanup
    // notification: the action notif must not claim a reclaim that never ran.
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
        .filter(|n| n.title == "Lucidos reclaimed disk space")
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
// Disk notifications: where a tap lands, and the route the body names.
// ---------------------------------------------------------------------------

/// Every body this module can produce, so a rule about the copy covers all of
/// them rather than the one branch a test remembered.
fn every_disk_body() -> Vec<String> {
    use super::auto_cleanup_body;
    use super::disk_monitor::disk_low_body;
    vec![
        disk_low_body(3 * GB, 40 * GB, 5 * GB), // large Lucidos footprint
        disk_low_body(3 * GB, GB, 5 * GB),      // pressure is elsewhere
        auto_cleanup_body(2 * GB, 7 * GB),
    ]
}

/// Disk Usage is a subpanel of System. A body stopping at "Settings → Disk
/// Usage" names a page with no Disk Usage on it. Someone who met the
/// notification away from the tap has only this route.
#[test]
fn every_disk_body_links_the_page_the_tap_opens() {
    const LINK: &str = "[Settings → System → Disk Usage](settings:disk-usage)";
    for body in every_disk_body() {
        assert!(body.contains(LINK), "{body}");
    }
}

/// The remedy still has to differ. One shared destination must not collapse
/// into one shared piece of advice: cleaning here reclaims real space only when
/// Lucidos is the one holding it.
#[test]
fn the_two_low_disk_branches_keep_their_own_remedy() {
    use super::disk_monitor::disk_low_body;
    let large = disk_low_body(3 * GB, 40 * GB, 5 * GB);
    let small = disk_low_body(3 * GB, GB, 5 * GB);
    assert!(large.contains("clean idle ones"), "{large}");
    assert!(
        !large.contains("other apps"),
        "a large footprint is not somebody else's problem: {large}"
    );
    assert!(small.contains("other apps"), "{small}");
    assert!(
        !small.contains("clean idle ones"),
        "cleaning 1 GB does not answer a 3 GB shortfall: {small}"
    );
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
