//! Tier 1 for a thread with nothing pending, the soft-pressure Tier 1 window,
//! and the wake that runs a cycle early (ADR 0311).

use super::common::*;
use super::ActiveThreads;
use crate::engine::event_bus::EventBus;
use crate::engine::git_ops::git_cmd;
use crate::test_support::{setup_test_db, teardown_test_db};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const HALF_HOUR: i64 = 30 * 60;

/// One non-archived thread with a worktree holding build artifacts and a
/// commit of its own, so Tier 0 never claims it.
struct Scenario {
    age_secs: i64,
    saved: bool,
    pending_change: bool,
    active_children: i32,
    soft_pressure: bool,
    active: fn() -> Arc<dyn ActiveThreads>,
}

/// What one cleanup cycle left behind.
#[derive(Debug)]
struct Outcome {
    tier_1_events: usize,
    target_left: bool,
    source_left: bool,
    branch_left: bool,
}

impl Scenario {
    /// Idle for `age_secs`, nothing pending, ample disk, no live session.
    fn idle(age_secs: i64) -> Self {
        Self {
            age_secs,
            saved: false,
            pending_change: false,
            active_children: 0,
            soft_pressure: false,
            active: no_active_threads,
        }
    }

    async fn run(self) -> Outcome {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let bus = Arc::new(bus);
        let (_tmp, root) = fresh_workspace().await;
        let thread_id = Uuid::new_v4();
        let worktree = add_worktree_for_thread(&root, thread_id, true).await;
        insert_thread_summary(&pool, thread_id, self.saved).await;
        if self.pending_change {
            insert_pending_change_for_thread(&pool, thread_id, &root).await;
        }
        if self.active_children > 0 {
            set_active_children_count(&pool, thread_id, self.active_children).await;
        }
        insert_old_event(&pool, thread_id, self.age_secs).await;

        let mut worker =
            make_worker_with_active(pool.clone(), bus.clone(), root.clone(), (self.active)());
        if self.soft_pressure {
            worker.free_soft_bytes = u64::MAX;
        }
        let rx = bus.subscribe();
        worker.run_once().await;

        let tier_1_events = drain_cleaned_events(rx, Duration::from_millis(200))
            .await
            .into_iter()
            .filter(|(t, tier, ..)| *t == thread_id && *tier == 1)
            .count();
        let short = &thread_id.simple().to_string()[..8];
        let outcome = Outcome {
            tier_1_events,
            target_left: worktree.join("target").exists(),
            source_left: worktree.join("branch_marker.txt").exists(),
            branch_left: branch_exists(&root, &format!("test/{short}")).await,
        };
        pool.close().await;
        teardown_test_db(&db_name).await;
        outcome
    }
}

async fn branch_exists(repo_root: &Path, branch: &str) -> bool {
    git_cmd(
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
        repo_root,
    )
    .await
    .is_ok_and(|o| o.status.success())
}

#[tokio::test]
async fn a_thread_with_nothing_pending_releases_only_its_build_artifacts_after_an_hour() {
    let outcome = Scenario::idle(TIER_0_AGE_SECS).run().await;
    assert_eq!(outcome.tier_1_events, 1, "{outcome:?}");
    assert!(!outcome.target_left, "{outcome:?}");
    assert!(outcome.source_left, "the source must stay: {outcome:?}");
    assert!(outcome.branch_left, "the branch must stay: {outcome:?}");
}

#[tokio::test]
async fn a_thread_idle_under_an_hour_keeps_its_build_artifacts() {
    let outcome = Scenario::idle(HALF_HOUR).run().await;
    assert_eq!(outcome.tier_1_events, 0, "{outcome:?}");
    assert!(outcome.target_left, "{outcome:?}");
}

#[tokio::test]
async fn a_saved_thread_keeps_its_build_artifacts_with_ample_disk() {
    let outcome = Scenario {
        saved: true,
        ..Scenario::idle(TIER_1_AGE)
    }
    .run()
    .await;
    assert!(outcome.target_left, "{outcome:?}");
}

#[tokio::test]
async fn a_parent_owing_fan_in_keeps_its_build_artifacts_with_ample_disk() {
    let outcome = Scenario {
        active_children: 1,
        ..Scenario::idle(TIER_1_AGE)
    }
    .run()
    .await;
    assert!(outcome.target_left, "{outcome:?}");
}

/// The cycle checks liveness, then waits on the database. A session that
/// started in that window must not lose `target/` mid-build.
#[tokio::test]
async fn a_session_that_starts_during_the_cycle_keeps_its_build_artifacts() {
    let outcome = Scenario {
        active: live_after_first_check,
        ..Scenario::idle(TIER_0_AGE_SECS)
    }
    .run()
    .await;
    assert!(outcome.target_left, "{outcome:?}");
}

#[tokio::test]
async fn soft_pressure_strips_an_hour_idle_thread_with_a_pending_change() {
    let outcome = Scenario {
        pending_change: true,
        soft_pressure: true,
        ..Scenario::idle(TIER_0_AGE_SECS)
    }
    .run()
    .await;
    assert_eq!(outcome.tier_1_events, 1, "{outcome:?}");
    assert!(!outcome.target_left, "{outcome:?}");
    assert!(outcome.source_left, "{outcome:?}");
}

/// A lookup that cannot run must never read as "nothing pending".
#[tokio::test]
async fn an_unanswered_lookup_counts_as_pending() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, root) = fresh_workspace().await;
    let worker = make_worker(pool.clone(), Arc::new(bus), root);
    let thread_id = Uuid::new_v4();

    assert!(worker.nothing_pending(thread_id).await, "answered: nothing");
    pool.close().await;
    assert!(!worker.nothing_pending(thread_id).await, "unanswered");

    teardown_test_db(&db_name).await;
}

/// A wake runs a cycle at once, not at the next interval.
#[tokio::test]
async fn a_wake_runs_a_cleanup_cycle_before_the_interval() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let bus = Arc::new(bus);
    let (_tmp, root) = fresh_workspace().await;
    let mut worker = make_worker(pool.clone(), bus, root.clone());
    worker.interval = Duration::from_secs(60 * 60);
    let wake = worker.cleanup_wake.clone();
    let task = tokio::spawn(worker.run_loop());
    // The first cycle runs at once over an empty worktrees dir.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_for_thread(&root, thread_id, true).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, TIER_0_AGE_SECS).await;
    wake.notify_one();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while worktree.join("target").exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !worktree.join("target").exists(),
        "the woken cycle should have released the artifacts"
    );

    task.abort();
    pool.close().await;
    teardown_test_db(&db_name).await;
}
