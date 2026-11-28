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
    let tmp = tempfile::tempdir().unwrap();
    let parent = tmp.path().to_path_buf();
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

#[tokio::test]
async fn prune_build_artifacts_strips_target_node_modules_cache() {
    use super::prune_build_artifacts;

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, true).await;

    assert!(worktree.join("target").exists());
    assert!(worktree.join("node_modules").exists());
    assert!(worktree.join(".lucidos/cache").exists());

    let freed = prune_build_artifacts(&worktree)
        .await
        .expect("expected non-zero prune");
    assert!(freed > 0, "expected non-zero freed bytes, got {}", freed);

    assert!(worktree.exists());
    assert!(!worktree.join("target").exists());
    assert!(!worktree.join("node_modules").exists());
    assert!(!worktree.join(".lucidos/cache").exists());

    assert!(
        prune_build_artifacts(&worktree).await.is_none(),
        "second prune must be a no-op when there's nothing left"
    );
}

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

    let big_id = Uuid::new_v4();
    let big_wt = add_worktree_for_thread(&root, big_id, true).await;
    tokio::fs::write(big_wt.join("target/extra.bin"), vec![0u8; 32 * 1024])
        .await
        .unwrap();
    insert_thread_summary(&pool, big_id, false).await;
    insert_old_event(&pool, big_id, 60).await;

    let small_id = Uuid::new_v4();
    let _small_wt = add_worktree_for_thread(&root, small_id, false).await;
    insert_thread_summary(&pool, small_id, true).await;
    insert_old_event(&pool, small_id, 60).await;

    let done_id = Uuid::new_v4();
    let _done_wt = add_worktree_at_main_for_thread(&root, done_id).await;
    insert_thread_summary(&pool, done_id, false).await;
    insert_old_event(&pool, done_id, 60).await;
    let live_id = Uuid::new_v4();
    let _live_wt = add_worktree_at_main_for_thread(&root, live_id).await;
    insert_thread_summary(&pool, live_id, false).await;
    insert_old_event(&pool, live_id, 60).await;

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
fn disk_reading_returns_free_and_total_for_an_existing_path() {
    use super::disk_reading;
    let tmp = tempfile::tempdir().unwrap();
    let reading = disk_reading(tmp.path()).expect("should read an existing tempdir");
    assert!(
        reading.free_bytes > 0,
        "free space must be > 0 on a healthy host"
    );
    assert!(reading.total_bytes >= reading.free_bytes);
}

#[test]
fn disk_reading_returns_none_for_missing_path() {
    use super::disk_reading;
    let bogus = Path::new("/this/path/does/not/exist/lucidos-test");
    assert!(disk_reading(bogus).is_none());
}

#[test]
fn thresholds_are_the_caps_on_a_big_volume_and_shares_on_a_small_one() {
    use super::{DiskThresholds, FREE_DISK_HARD_BYTES, FREE_DISK_SOFT_BYTES};
    let on = |total| DiskThresholds::for_volume(total, FREE_DISK_SOFT_BYTES, FREE_DISK_HARD_BYTES);
    let caps = DiskThresholds {
        soft_bytes: 20 * GB,
        hard_bytes: 5 * GB,
    };
    assert_eq!(on(80 * GB), caps, "80 GiB is where both caps start to bind");
    assert_eq!(on(1024 * GB), caps);
    assert_eq!(on(u64::MAX), caps);
    assert_eq!(
        on(50 * GB),
        DiskThresholds {
            soft_bytes: 50 * GB / 4,
            hard_bytes: 5 * GB,
        }
    );
    let small = on(20 * GB);
    assert_eq!(small.soft_bytes, 5 * GB);
    assert_eq!(small.hard_bytes, 2 * GB);
    assert_eq!(
        on(0),
        DiskThresholds {
            soft_bytes: 0,
            hard_bytes: 0
        }
    );
}

#[test]
fn no_reading_gets_the_caps() {
    use super::{DiskReading, DiskThresholds, FREE_DISK_HARD_BYTES, FREE_DISK_SOFT_BYTES};
    let of =
        |reading| DiskThresholds::for_reading(reading, FREE_DISK_SOFT_BYTES, FREE_DISK_HARD_BYTES);
    let on = |total| DiskThresholds::for_volume(total, FREE_DISK_SOFT_BYTES, FREE_DISK_HARD_BYTES);
    assert_eq!(of(None), on(u64::MAX));
    let small = DiskReading {
        free_bytes: 3 * GB,
        total_bytes: 20 * GB,
    };
    assert_eq!(of(Some(small)), on(20 * GB));
}

#[test]
fn zfs_ancestors_run_from_the_pool_root_to_the_dataset() {
    use super::zfs::dataset_and_ancestors;
    let all: Vec<_> = dataset_and_ancestors("rpool/USERDATA/home").collect();
    assert_eq!(all, ["rpool", "rpool/USERDATA", "rpool/USERDATA/home"]);
    let root: Vec<_> = dataset_and_ancestors("tank").collect();
    assert_eq!(root, ["tank"]);
}

#[test]
fn zfs_size_is_the_pool_bounded_by_every_quota_that_applies() {
    use super::zfs::size_from_listing;
    let no_quota = "tank\t0\t0\t5000\t15000\ntank/ws\t0\t0\t3000\t15000\n";
    assert_eq!(size_from_listing("tank/ws", no_quota), Some(20_000));
    let own = "tank\t0\t0\t5000\t15000\ntank/ws\t9000\t8000\t3000\t5000\n";
    assert_eq!(size_from_listing("tank/ws", own), Some(8000));
    let ancestor =
        "tank\t0\t0\t5000\t15000\ntank/a\t7000\t0\t4000\t3000\ntank/a/ws\t0\t0\t3000\t3000\n";
    assert_eq!(size_from_listing("tank/a/ws", ancestor), Some(7000));
    let ancestor_refquota =
        "tank\t0\t0\t5000\t15000\ntank/a\t0\t1000\t4000\t3000\ntank/a/ws\t0\t0\t3000\t3000\n";
    assert_eq!(
        size_from_listing("tank/a/ws", ancestor_refquota),
        Some(20_000),
        "an ancestor's refquota limits only its own data"
    );
    assert_eq!(
        size_from_listing("tank", "tank\t0\t0\t5000\t15000\n"),
        Some(20_000)
    );
    assert_eq!(
        size_from_listing("tank/ws", "tank/ws\t0\t0\t3000\t15000\n"),
        None
    );
    assert_eq!(size_from_listing("tank", "tank\t-\t0\t5000\t15000\n"), None);
    assert_eq!(
        size_from_listing("tank", &format!("tank\t0\t0\t{}\t1\n", u64::MAX)),
        None
    );
}

fn idle_engine_probe() -> (
    super::EngineActiveThreads,
    Arc<tokio::sync::Mutex<std::collections::HashMap<Uuid, crate::engine::AgentSession>>>,
    crate::engine::bash_background::BackgroundBashRegistry,
) {
    let sessions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let registry = crate::engine::bash_background::BackgroundBashRegistry::new();
    let probe = super::EngineActiveThreads::new(
        sessions.clone(),
        registry.clone(),
        crate::engine::terminal::TerminalRegistry::new(),
    );
    (probe, sessions, registry)
}

#[tokio::test]
async fn a_claimed_run_makes_its_thread_active_until_it_lets_go() {
    use super::ActiveThreads;

    let (probe, sessions, _registry) = idle_engine_probe();
    let thread_id = Uuid::new_v4();
    assert!(!probe.is_active(thread_id).await, "nothing running yet");
    let claim = crate::engine::session_claim::claim_thread_for(
        &mut *sessions.lock().await,
        thread_id,
        crate::runtime::CodingAgent::ClaudeCode,
        crate::engine::session_claim::ClaimFor::Run,
    )
    .unwrap();
    assert!(probe.is_active(thread_id).await, "the run is starting");
    assert!(
        !probe.is_active(Uuid::new_v4()).await,
        "another thread's run does not count"
    );
    drop(claim);
    assert!(
        !probe.is_active(thread_id).await,
        "a run that let go does not count"
    );
}

#[tokio::test]
async fn a_running_background_task_makes_its_thread_active() {
    use super::ActiveThreads;

    let tmp = tempfile::tempdir().unwrap();
    let (probe, _sessions, registry) = idle_engine_probe();
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
    let window = |free| {
        DiskPressure::classify(Some(on_a_huge_volume(free)), 20 * GB, 5 * GB).tier1_idle(forced)
    };
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
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let _wt = add_worktree_for_thread(&root, thread_id, true).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, 90 * 60).await;

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.hard_cap_bytes = u64::MAX;
    worker.soft_cap_bytes = u64::MAX;

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
    assert_eq!(cleanups[0].settings_view(), Some("disk-usage"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn hard_threshold_no_auto_cleanup_when_nothing_freed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.hard_cap_bytes = u64::MAX;
    worker.soft_cap_bytes = u64::MAX;

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

fn every_disk_body() -> Vec<String> {
    use super::auto_cleanup_body;
    use super::disk_monitor::disk_low_body;
    vec![
        disk_low_body(3 * GB, 40 * GB, 5 * GB),
        disk_low_body(3 * GB, GB, 5 * GB),
        auto_cleanup_body(2 * GB, 7 * GB),
    ]
}

#[test]
fn every_disk_body_links_the_page_the_tap_opens() {
    const LINK: &str = "[Settings → System → Disk Usage](settings:disk-usage)";
    for body in every_disk_body() {
        assert!(body.contains(LINK), "{body}");
    }
}

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
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.soft_cap_bytes = u64::MAX;
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events
        .into_iter()
        .filter(|(t, ..)| *t == thread_id)
        .collect();
    assert_eq!(cleaned.len(), 1, "exactly one Tier 0 event");
    let (_, tier, _, branch_deleted) = cleaned[0];
    assert_eq!(tier, 0);
    assert!(!branch_deleted, "Tier 0 keeps the merged branch");
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
    let worktree = add_worktree_for_thread(&root, thread_id, false).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_0_AGE_SECS).await;

    let rx = bus.subscribe();
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.soft_cap_bytes = u64::MAX;
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
    insert_old_event(&pool, thread_id, 30 * 60).await;

    let rx = bus.subscribe();
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.soft_cap_bytes = u64::MAX;
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
    insert_old_event(&pool, thread_id, 30).await;

    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.soft_cap_bytes = u64::MAX;
    worker.hard_cap_bytes = u64::MAX;

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
    let mut worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.soft_cap_bytes = u64::MAX;
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

#[tokio::test]
async fn tier_0_skips_thread_with_live_agent_session() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_0_AGE_SECS).await;

    let rx = bus.subscribe();
    let mut worker = make_worker_with_active(
        pool.clone(),
        bus.clone(),
        root.clone(),
        active_threads(&[thread_id]),
    );
    worker.soft_cap_bytes = u64::MAX;
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
