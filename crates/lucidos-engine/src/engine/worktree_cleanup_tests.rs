//! Tests for the background worktree cleanup worker (Phase 10.2 + 10.3).
//!
//! Each test builds a real workspace dir under `tempfile::tempdir()` with a
//! `.lucidos/worktrees/thread-<short>` worktree inside a real git repo, seeds
//! the events table to control "idle age", and drives [`WorktreeCleanup::run_once`]
//! directly so we don't sleep on the hour-long real-world cycle.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use sqlx::PgPool;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EmittedEvent, EventBus, SystemEvent};
use crate::engine::git_ops::{git_cmd, worktrees_dir};
use crate::engine::thread_events::ThreadEvent;
use crate::test_support::{setup_test_db, teardown_test_db};

use super::WorktreeCleanup;

/// Build a minimal workspace + repo on disk: a tempdir, an initialized git
/// repo at its root with one commit on `main`, and `.lucidos/worktrees/`
/// pre-created. Returns `(workspace_root, repo_root)` — for our tests they
/// are the same path because `git_ops::main_worktree()` resolves from the
/// process's compile-time path. We honor that by `cd`-ing into the workspace
/// for git operations done via `git_cmd`.
async fn fresh_workspace() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let root = tmp.path().to_path_buf();
    // Initialise the repo so `worktree add` works.
    git_cmd(&["init", "--initial-branch=main"], &root)
        .await
        .expect("git init");
    git_cmd(&["config", "user.email", "cleanup@test"], &root)
        .await
        .unwrap();
    git_cmd(&["config", "user.name", "Cleanup Test"], &root)
        .await
        .unwrap();
    tokio::fs::write(root.join("seed.txt"), "seed").await.unwrap();
    git_cmd(&["add", "."], &root).await.unwrap();
    git_cmd(&["commit", "-m", "seed"], &root).await.unwrap();
    let _ = worktrees_dir(&root); // ensure dir exists
    (tmp, root)
}

/// Add a worktree on a fresh branch derived from the thread id and write some
/// junk files into it. Returns the worktree path. When `with_artifacts=true`
/// we plant `target/` and `node_modules/` so Tier 1 has something to reclaim.
async fn add_worktree_for_thread(
    repo_root: &Path,
    thread_id: Uuid,
    with_artifacts: bool,
) -> PathBuf {
    let short = &thread_id.simple().to_string()[..8];
    let dir_name = format!("thread-{}", short);
    let worktree = worktrees_dir(repo_root).join(&dir_name);
    let branch = format!("test/{}", short);
    git_cmd(
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &worktree.to_string_lossy(),
            "main",
        ],
        repo_root,
    )
    .await
    .expect("git worktree add");

    if with_artifacts {
        let target = worktree.join("target");
        let node_modules = worktree.join("node_modules");
        let cache = worktree.join(".lucidos/cache");
        tokio::fs::create_dir_all(&target).await.unwrap();
        tokio::fs::create_dir_all(&node_modules).await.unwrap();
        tokio::fs::create_dir_all(&cache).await.unwrap();
        // Make the artifacts large enough that `freed_bytes > 0` is meaningful.
        tokio::fs::write(target.join("big.bin"), vec![0u8; 4096])
            .await
            .unwrap();
        tokio::fs::write(node_modules.join("pkg.json"), b"{}")
            .await
            .unwrap();
        tokio::fs::write(cache.join("c.dat"), b"x").await.unwrap();
    }

    worktree
}

/// Insert a thread_summaries row matching the deterministic id, optionally pinned.
async fn insert_thread_summary(pool: &PgPool, thread_id: Uuid, is_pinned: bool) {
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_pinned) \
         VALUES ($1, 'cleanup test', 'claude_code', 1, NOW(), false, $2)",
    )
    .bind(thread_id)
    .bind(is_pinned)
    .execute(pool)
    .await
    .expect("insert thread_summary");
}

/// Insert a backdated MessageReceived event for the given thread. `age_secs`
/// controls how old it appears so the cleanup worker classifies the thread as
/// idle.
async fn insert_old_event(pool: &PgPool, thread_id: Uuid, age_secs: i64) {
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, 'MessageReceived', $3, NOW() - make_interval(secs => $4), $2::uuid)",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id)
    .bind(json!({"text": "old"}))
    .bind(age_secs as f64)
    .execute(pool)
    .await
    .expect("insert old event");
}

/// Build a worker pre-configured for tests: short interval + zero free-disk
/// thresholds so the alert never fires by accident. Tests that need a trigger
/// override these fields directly.
fn make_worker(pool: PgPool, bus: Arc<EventBus>, workspace: PathBuf) -> WorktreeCleanup {
    WorktreeCleanup {
        pool,
        bus,
        workspace_root: workspace,
        interval: Duration::from_secs(60),
        free_soft_bytes: 0,
        free_hard_bytes: 0,
        force_tier1_idle: Duration::from_secs(60 * 60),
        alerts: Mutex::new(super::AlertState::default()),
    }
}

/// Drain a pre-existing receiver for WorktreeCleaned events until `deadline`.
/// The receiver MUST be created before the producer emits — broadcast channels
/// drop messages for non-subscribed listeners.
async fn drain_cleaned_events(
    rx: tokio::sync::broadcast::Receiver<EmittedEvent>,
    deadline: Duration,
) -> Vec<(Uuid, u8, u64, bool)> {
    let mut stream = BroadcastStream::new(rx);
    let mut out = Vec::new();
    let until = tokio::time::Instant::now() + deadline;
    while let Ok(Some(item)) = tokio::time::timeout_at(until, stream.next()).await {
        let Ok(EmittedEvent { typed, .. }) = item else {
            continue;
        };
        if let BusEvent::Thread { thread_id, event, .. } = typed {
            if let ThreadEvent::WorktreeCleaned {
                tier,
                freed_bytes,
                branch_deleted,
            } = event
            {
                out.push((thread_id, tier, freed_bytes, branch_deleted));
            }
        }
    }
    out
}

/// Drain a pre-existing receiver for SystemEvent::NotificationCreated events.
async fn drain_notifications(
    rx: tokio::sync::broadcast::Receiver<EmittedEvent>,
    deadline: Duration,
) -> Vec<(String, String)> {
    let mut stream = BroadcastStream::new(rx);
    let mut out = Vec::new();
    let until = tokio::time::Instant::now() + deadline;
    while let Ok(Some(item)) = tokio::time::timeout_at(until, stream.next()).await {
        let Ok(EmittedEvent { typed, .. }) = item else {
            continue;
        };
        if let BusEvent::System(SystemEvent::NotificationCreated { title, message, .. }) =
            typed
        {
            out.push((title, message));
        }
    }
    out
}

const ONE_DAY: i64 = 24 * 60 * 60;
const TIER_2_AGE: i64 = 31 * ONE_DAY; // > 30 days
const TIER_1_AGE: i64 = 25 * 60 * 60; // > 24 hours

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
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
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
async fn tier_2_removes_worktree_after_30_days_clean_unpinned() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, false).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_2_AGE).await;

    let rx = bus.subscribe();
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
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
async fn pinned_threads_are_exempt_from_tier_2() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, false).await;
    insert_thread_summary(&pool, thread_id, true /* pinned */).await;
    insert_old_event(&pool, thread_id, TIER_2_AGE).await;

    let rx = bus.subscribe();
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert!(
        cleaned.iter().all(|(_, tier, _, _)| *tier != 2),
        "no Tier 2 event for a pinned thread, got: {:?}",
        cleaned
    );
    assert!(worktree.exists(), "pinned worktree must remain on disk");

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
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
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
        .filter(|(t, _)| t == "Lucidos disk space low")
        .collect();
    assert_eq!(alerts.len(), 1, "expected exactly one low-disk notification, got: {:?}", alerts);

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
        .filter(|(t, _)| t.starts_with("Lucidos") && t.contains("disk"))
        .collect();
    assert!(alerts.is_empty(), "no disk-related notifications when free disk is ample, got: {:?}", alerts);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tier_2_deletes_branch_if_fully_merged() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);

    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, false).await;
    let short = &thread_id.simple().to_string()[..8];
    let branch = format!("test/{}", short);

    // Branch has zero commits ahead of main → fully merged for our purposes.
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_2_AGE).await;

    let rx = bus.subscribe();
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    let cleaned: Vec<_> = events.into_iter().filter(|(t, ..)| *t == thread_id).collect();
    assert_eq!(cleaned.len(), 1, "Tier 2 should fire");
    let (_, _, _, branch_deleted) = cleaned[0];
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
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
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
    let worker = make_worker(pool.clone(), bus.clone(), root.clone());
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
    insert_thread_summary(&pool, small_id, true /* pinned */).await;
    insert_old_event(&pool, small_id, 60).await;

    let rows = inventory_worktrees(&pool, &root).await;
    assert!(rows.len() >= 2, "expected at least 2 rows, got {}", rows.len());

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
    assert!(small.is_pinned, "pinned flag must be carried through");

    let big = &rows[big_idx];
    assert!(!big.is_pinned, "unpinned flag must be carried through");
    assert!(big.size_bytes > small.size_bytes);
    assert!(big.last_activity.is_some());
    assert!(big.thread_title.is_some(), "thread_title must be carried through");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn available_disk_bytes_returns_some_for_existing_path() {
    use super::available_disk_bytes;
    let tmp = std::env::temp_dir();
    let bytes = available_disk_bytes(&tmp);
    assert!(bytes.is_some(), "should return Some for an existing tempdir");
    assert!(bytes.unwrap() > 0, "free space must be > 0 on a healthy host");
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
        .filter(|(t, _)| t == "Lucidos disk space low")
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
        .filter(|(t, _)| t == "Lucidos disk space low")
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
        .filter(|(t, _)| t == "Lucidos auto-cleanup running")
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
        .filter(|(t, _)| t == "Lucidos auto-cleanup running")
        .count();
    assert_eq!(
        cleanups, 0,
        "auto-cleanup notification must not fire when nothing was reclaimed; saw: {:?}",
        notifications
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

