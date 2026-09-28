//! The recommended cleanup: a finished worktree goes, every other one loses
//! only its build artifacts, and live, pinned or stranded trees are untouched.

use super::common::*;
use super::{run_recommended_cleanup, worktree_size_breakdown, ActiveThreads, WorktreeSize};
use crate::engine::event_bus::EventBus;
use crate::engine::git_ops::git_cmd;
use crate::test_support::{setup_test_db, teardown_test_db};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// The shape of the one worktree a scenario plants.
#[derive(Clone, Copy)]
enum Tree {
    /// Branch at main, clean: finished unless the thread has something pending.
    AtMain,
    /// Branch at main with an untracked file.
    AtMainDirty,
    /// A commit ahead of main, plus build artifacts.
    AheadWithArtifacts,
    /// Ahead with artifacts, then its git admin dir deleted.
    Stranded,
}

struct Scenario {
    tree: Tree,
    saved: bool,
    pending_change: bool,
    active_children: i32,
    active: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    removed_count: u32,
    cleaned_count: u32,
    /// Tiers of the `WorktreeCleaned` events for the scenario's thread.
    event_tiers: Vec<u8>,
    tree_left: bool,
    target_left: bool,
    branch_left: bool,
}

impl Scenario {
    fn of(tree: Tree) -> Self {
        Self {
            tree,
            saved: false,
            pending_change: false,
            active_children: 0,
            active: false,
        }
    }

    async fn run(self) -> Outcome {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let (_tmp, root) = fresh_workspace().await;
        let thread_id = Uuid::new_v4();
        let worktree = match self.tree {
            Tree::AtMain | Tree::AtMainDirty => {
                add_worktree_at_main_for_thread(&root, thread_id).await
            }
            Tree::AheadWithArtifacts | Tree::Stranded => {
                add_worktree_for_thread(&root, thread_id, true).await
            }
        };
        match self.tree {
            Tree::AtMainDirty => tokio::fs::write(worktree.join("notes.txt"), b"wip")
                .await
                .unwrap(),
            Tree::Stranded => strand_worktree(&worktree).await,
            Tree::AtMain | Tree::AheadWithArtifacts => {}
        }
        insert_thread_summary(&pool, thread_id, self.saved).await;
        if self.pending_change {
            insert_pending_change_for_thread(&pool, thread_id, &root).await;
        }
        if self.active_children > 0 {
            set_active_children_count(&pool, thread_id, self.active_children).await;
        }
        insert_old_event(&pool, thread_id, 60).await;
        let probe: Arc<dyn ActiveThreads> = if self.active {
            active_threads(&[thread_id])
        } else {
            no_active_threads()
        };

        let rx = bus.subscribe();
        let result = run_recommended_cleanup(&pool, &bus, &root, probe.as_ref(), None).await;
        let event_tiers = drain_cleaned_events(rx, Duration::from_millis(200))
            .await
            .into_iter()
            .filter(|(t, ..)| *t == thread_id)
            .map(|(_, tier, ..)| tier)
            .collect();
        let short = &thread_id.simple().to_string()[..8];
        let outcome = Outcome {
            removed_count: result.removed_count,
            cleaned_count: result.cleaned_count,
            event_tiers,
            tree_left: worktree.exists(),
            target_left: worktree.join("target").exists(),
            branch_left: branch_exists(&root, &format!("test/{short}")).await,
        };
        pool.close().await;
        teardown_test_db(&db_name).await;
        outcome
    }
}

async fn branch_exists(repo: &Path, branch: &str) -> bool {
    git_cmd(&["rev-parse", "--verify", "--quiet", branch], repo)
        .await
        .map(|o| o.status.success())
        .expect("git rev-parse ran")
}

#[tokio::test]
async fn a_finished_worktree_is_removed_with_its_merged_branch() {
    let outcome = Scenario::of(Tree::AtMain).run().await;
    assert_eq!(outcome.removed_count, 1);
    assert_eq!(outcome.event_tiers, vec![0]);
    assert!(!outcome.tree_left);
    assert!(!outcome.branch_left, "a fully merged branch goes with it");
}

#[tokio::test]
async fn an_unmerged_worktree_loses_only_its_build_artifacts() {
    let outcome = Scenario::of(Tree::AheadWithArtifacts).run().await;
    assert_eq!(outcome.removed_count, 0);
    assert_eq!(outcome.cleaned_count, 1);
    assert_eq!(outcome.event_tiers, vec![1]);
    assert!(outcome.tree_left);
    assert!(!outcome.target_left);
    assert!(outcome.branch_left);
}

#[tokio::test]
async fn a_dirty_worktree_is_kept() {
    let outcome = Scenario::of(Tree::AtMainDirty).run().await;
    assert_eq!(outcome.removed_count, 0);
    assert!(outcome.tree_left);
    assert!(outcome.branch_left);
}

#[tokio::test]
async fn a_worktree_with_a_pending_change_is_kept() {
    let outcome = Scenario {
        pending_change: true,
        ..Scenario::of(Tree::AtMain)
    }
    .run()
    .await;
    assert_eq!(outcome.removed_count, 0);
    assert!(outcome.tree_left);
}

#[tokio::test]
async fn a_worktree_that_owes_a_fan_in_is_kept() {
    let outcome = Scenario {
        active_children: 1,
        ..Scenario::of(Tree::AtMain)
    }
    .run()
    .await;
    assert_eq!(outcome.removed_count, 0);
    assert!(outcome.tree_left);
}

#[tokio::test]
async fn a_pinned_thread_is_untouched() {
    let outcome = Scenario {
        saved: true,
        ..Scenario::of(Tree::AheadWithArtifacts)
    }
    .run()
    .await;
    assert_eq!((outcome.removed_count, outcome.cleaned_count), (0, 0));
    assert!(outcome.event_tiers.is_empty());
    assert!(outcome.target_left);

    let finished = Scenario {
        saved: true,
        ..Scenario::of(Tree::AtMain)
    }
    .run()
    .await;
    assert!(finished.tree_left, "a pin keeps even a finished worktree");
}

#[tokio::test]
async fn a_live_thread_is_untouched() {
    let outcome = Scenario {
        active: true,
        ..Scenario::of(Tree::AheadWithArtifacts)
    }
    .run()
    .await;
    assert_eq!((outcome.removed_count, outcome.cleaned_count), (0, 0));
    assert!(outcome.target_left);

    let finished = Scenario {
        active: true,
        ..Scenario::of(Tree::AtMain)
    }
    .run()
    .await;
    assert!(finished.tree_left);
}

#[tokio::test]
async fn a_stranded_worktree_is_left_to_the_worker() {
    let outcome = Scenario::of(Tree::Stranded).run().await;
    assert_eq!((outcome.removed_count, outcome.cleaned_count), (0, 0));
    assert!(outcome.tree_left);
    assert!(outcome.target_left);
}

#[test]
fn the_size_breakdown_splits_artifacts_from_source_in_one_walk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for (path, bytes) in [
        ("target/debug/a.bin", 100),
        ("node_modules/pkg/index.js", 50),
        (".lucidos/cache/c.dat", 10),
        (".lucidos/ports", 5),
        ("src/main.rs", 20),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, vec![0u8; bytes]).unwrap();
    }
    assert_eq!(
        worktree_size_breakdown(root, &["target", "node_modules", ".lucidos/cache"]),
        WorktreeSize {
            total_bytes: 185,
            artifact_bytes: 160,
        }
    );
}

#[tokio::test]
async fn a_tree_git_cannot_read_is_not_finished() {
    let (pool, db_name) = setup_test_db().await;
    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    let worktree = add_worktree_at_main_for_thread(&root, thread_id).await;
    insert_thread_summary(&pool, thread_id, false).await;
    strand_worktree(&worktree).await;
    let changes = crate::core::changes_projection::ChangesProjection::new(pool.clone());
    assert!(!super::is_finished_worktree(&pool, &changes, thread_id, &worktree).await);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn every_cleaned_event_carries_the_callers_actor() {
    use crate::engine::event_bus::{BusEvent, EmittedEvent};
    use crate::engine::thread_events::{MessageOrigin, ThreadEvent};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, root) = fresh_workspace().await;
    let thread_id = Uuid::new_v4();
    add_worktree_for_thread(&root, thread_id, true).await;
    insert_thread_summary(&pool, thread_id, false).await;
    insert_old_event(&pool, thread_id, 60).await;
    let actor = MessageOrigin::Device {
        device_id: "test-device".to_string(),
    };

    let mut rx = bus.subscribe();
    let probe = no_active_threads();
    run_recommended_cleanup(&pool, &bus, &root, probe.as_ref(), Some(actor.clone())).await;

    let mut actors = Vec::new();
    while let Ok(Ok(EmittedEvent { typed, .. })) =
        tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
    {
        if let BusEvent::Thread {
            event: ThreadEvent::WorktreeCleaned { .. },
            meta,
            ..
        } = typed
        {
            actors.push(meta.actor);
        }
    }
    assert_eq!(actors, vec![Some(actor)]);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_artifact_directory_holding_tracked_files_is_not_prunable() {
    let (_tmp, root) = fresh_workspace().await;
    let worktree = add_worktree_at_main_for_thread(&root, Uuid::new_v4()).await;
    for dir in ["target", "node_modules"] {
        tokio::fs::create_dir_all(worktree.join(dir)).await.unwrap();
    }
    tokio::fs::write(worktree.join("node_modules/vendored.js"), b"x")
        .await
        .unwrap();
    tokio::fs::write(worktree.join("target/build.bin"), b"x")
        .await
        .unwrap();
    git_cmd(&["add", "node_modules/vendored.js"], &worktree)
        .await
        .unwrap();
    git_cmd(&["commit", "-m", "vendor"], &worktree)
        .await
        .unwrap();

    assert_eq!(
        super::prunable_artifact_dirs(&worktree).await,
        vec!["target"]
    );
}
