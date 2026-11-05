//! The archived removal: an archived thread's worktree goes once idle past
//! `ARCHIVED_REMOVAL_GRACE`, dirty or ahead of main, and its work stays on the
//! branch. Plan: `docs/plans/2026-10-12-archive-removes-thread-worktrees.md`.

use super::common::*;
use super::{commit_worktree_or_err, move_worktree_aside, SNAPSHOT_COMMIT_MESSAGE};
use crate::engine::event_bus::{EmittedEvent, EventBus};
use crate::engine::git_ops::{git_cmd, worktrees_dir};
use crate::test_support::{setup_test_db, teardown_test_db};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Past the one-hour grace.
const PAST_GRACE: i64 = 90 * 60;

struct Fixture {
    pool: sqlx::PgPool,
    db_name: String,
    bus: Arc<EventBus>,
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let (_tmp, root) = fresh_workspace().await;
        Self {
            pool,
            db_name,
            bus: Arc::new(bus),
            _tmp,
            root,
        }
    }

    /// An archived thread idle for `age_secs`, whose session ran on its own
    /// branch, with a worktree holding one commit ahead of main.
    async fn archived_thread(&self, age_secs: i64) -> (Uuid, PathBuf, String) {
        let thread_id = Uuid::new_v4();
        let worktree = add_worktree_for_thread(&self.root, thread_id, true).await;
        let branch = branch_of(thread_id);
        insert_thread_summary_with_archive(&self.pool, thread_id, false, "archived").await;
        insert_session_started(&self.pool, thread_id, &branch, age_secs).await;
        insert_old_event(&self.pool, thread_id, age_secs).await;
        (thread_id, worktree, branch)
    }

    async fn run_pass(&self) -> Vec<(Uuid, u8, u64, bool)> {
        self.run_pass_with(no_active_threads()).await
    }

    async fn run_pass_with(
        &self,
        active: Arc<dyn super::ActiveThreads>,
    ) -> Vec<(Uuid, u8, u64, bool)> {
        let rx: tokio::sync::broadcast::Receiver<EmittedEvent> = self.bus.subscribe();
        make_worker_with_active(
            self.pool.clone(),
            self.bus.clone(),
            self.root.clone(),
            active,
        )
        .run_once()
        .await;
        drain_cleaned_events(rx, Duration::from_millis(200)).await
    }

    async fn teardown(self) {
        self.pool.close().await;
        teardown_test_db(&self.db_name).await;
    }
}

fn branch_of(thread_id: Uuid) -> String {
    format!("test/{}", &thread_id.simple().to_string()[..8])
}

async fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
    let out = git_cmd(args, dir).await.ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn tip(root: &Path, branch: &str) -> Option<String> {
    git_out(
        root,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
    )
    .await
}

fn cleaned(events: &[(Uuid, u8, u64, bool)], thread_id: Uuid) -> Vec<(u8, bool)> {
    events
        .iter()
        .filter(|(t, ..)| *t == thread_id)
        .map(|(_, tier, _, deleted)| (*tier, *deleted))
        .collect()
}

/// Whether the pass removed the whole tree. Tier 1 may still strip the build
/// artifacts of a tree it keeps.
fn removed(events: &[(Uuid, u8, u64, bool)], thread_id: Uuid) -> bool {
    cleaned(events, thread_id)
        .iter()
        .any(|(tier, _)| *tier == 2)
}

/// Leave a modified tracked file and an untracked one, the two kinds of dirt.
async fn make_dirty(worktree: &Path) {
    tokio::fs::write(worktree.join("seed.txt"), "edited, never committed")
        .await
        .unwrap();
    tokio::fs::write(worktree.join("notes.md"), "untracked work")
        .await
        .unwrap();
}

/// Archived, dirty and ahead of main. The tree goes, and
/// every uncommitted byte is on the branch first.
#[tokio::test]
async fn a_dirty_archived_tree_is_snapshotted_on_its_branch_then_removed() {
    let fx = Fixture::new().await;
    let (thread_id, worktree, branch) = fx.archived_thread(PAST_GRACE).await;
    let before = tip(&fx.root, &branch).await.expect("branch exists");
    make_dirty(&worktree).await;

    let events = fx.run_pass().await;

    assert_eq!(cleaned(&events, thread_id), vec![(2, false)]);
    assert!(
        !worktree.exists(),
        "the archived tree is gone, target/ with it"
    );
    let after = tip(&fx.root, &branch).await.expect("the branch survives");
    assert_ne!(after, before, "the snapshot moved the branch forward");
    assert_eq!(
        git_out(&fx.root, &["log", "-1", "--format=%s", &branch])
            .await
            .as_deref(),
        Some(SNAPSHOT_COMMIT_MESSAGE)
    );
    assert_eq!(
        git_out(&fx.root, &["show", &format!("{branch}:seed.txt")])
            .await
            .as_deref(),
        Some("edited, never committed")
    );
    assert_eq!(
        git_out(&fx.root, &["show", &format!("{branch}:notes.md")])
            .await
            .as_deref(),
        Some("untracked work")
    );
    assert!(
        git_out(&fx.root, &["show", &format!("{branch}:target/big.bin")])
            .await
            .is_none(),
        "build output is stripped before the snapshot, never committed"
    );
    assert!(
        git_out(&fx.root, &["merge-base", "--is-ancestor", &before, &after])
            .await
            .is_some(),
        "the earlier commits are still on the branch"
    );
    fx.teardown().await;
}

/// The squash-applied and set-aside cases: archived, clean, ahead of main. The
/// tree goes and the branch stays exactly where it was.
#[tokio::test]
async fn a_clean_archived_tree_ahead_of_main_is_removed_and_its_branch_kept() {
    let fx = Fixture::new().await;
    let (thread_id, worktree, branch) = fx.archived_thread(PAST_GRACE).await;
    let before = tip(&fx.root, &branch).await;

    let events = fx.run_pass().await;

    assert_eq!(cleaned(&events, thread_id), vec![(2, false)]);
    assert!(!worktree.exists());
    assert_eq!(
        tip(&fx.root, &branch).await,
        before,
        "no commit added or lost"
    );
    fx.teardown().await;
}

/// Undo or Move to Current inside the grace finds the tree warm.
#[tokio::test]
async fn an_archived_tree_inside_the_grace_is_kept() {
    let fx = Fixture::new().await;
    let (thread_id, worktree, _) = fx.archived_thread(10 * 60).await;
    make_dirty(&worktree).await;

    let events = fx.run_pass().await;

    assert!(cleaned(&events, thread_id).is_empty());
    assert!(worktree.exists());
    fx.teardown().await;
}

/// A snapshot must never land on a branch that is not the thread's own.
#[tokio::test]
async fn dirt_on_a_branch_that_is_not_the_threads_own_keeps_the_tree() {
    let fx = Fixture::new().await;
    let (thread_id, worktree, _) = fx.archived_thread(PAST_GRACE).await;
    git_cmd(&["checkout", "-q", "-b", "user-feature"], &worktree)
        .await
        .unwrap();
    let before = tip(&fx.root, "user-feature").await;
    make_dirty(&worktree).await;

    let events = fx.run_pass().await;

    assert!(!removed(&events, thread_id));
    assert!(worktree.exists(), "the tree and its dirt stay");
    assert_eq!(
        tip(&fx.root, "user-feature").await,
        before,
        "no commit landed"
    );
    fx.teardown().await;
}

/// A live session, spawn or background task keeps the tree, archived or not.
#[tokio::test]
async fn an_archived_tree_in_use_is_kept() {
    let fx = Fixture::new().await;
    let (thread_id, worktree, branch) = fx.archived_thread(PAST_GRACE).await;
    let before = tip(&fx.root, &branch).await;
    make_dirty(&worktree).await;

    let events = fx.run_pass_with(active_threads(&[thread_id])).await;

    assert!(cleaned(&events, thread_id).is_empty());
    assert!(worktree.exists());
    assert_eq!(tip(&fx.root, &branch).await, before, "no snapshot either");
    fx.teardown().await;
}

/// A pending change on an archived thread surfaces it in Current. The user's
/// decision is still open, so the tree waits for it.
#[tokio::test]
async fn an_archived_tree_with_a_pending_change_is_kept() {
    let fx = Fixture::new().await;
    let (thread_id, worktree, _) = fx.archived_thread(PAST_GRACE).await;
    insert_pending_change_for_thread(&fx.pool, thread_id, &fx.root).await;

    let events = fx.run_pass().await;

    assert!(cleaned(&events, thread_id).is_empty());
    assert!(worktree.exists());
    fx.teardown().await;
}

/// A parent's Archive cascades a `ThreadArchived` to each child, so each is
/// an archived thread in its own right and loses its own tree.
#[tokio::test]
async fn a_parent_and_its_children_archived_together_all_lose_their_trees() {
    let fx = Fixture::new().await;
    let (parent, parent_tree, _) = fx.archived_thread(PAST_GRACE).await;
    let (child, child_tree, child_branch) = fx.archived_thread(PAST_GRACE).await;
    sqlx::query("UPDATE thread_summaries SET parent_thread_id = $2 WHERE thread_id = $1")
        .bind(child)
        .bind(parent)
        .execute(&fx.pool)
        .await
        .unwrap();
    make_dirty(&child_tree).await;

    let events = fx.run_pass().await;

    assert_eq!(cleaned(&events, parent), vec![(2, false)]);
    assert_eq!(cleaned(&events, child), vec![(2, false)]);
    assert!(!parent_tree.exists() && !child_tree.exists());
    assert_eq!(
        git_out(&fx.root, &["log", "-1", "--format=%s", &child_branch])
            .await
            .as_deref(),
        Some(SNAPSHOT_COMMIT_MESSAGE)
    );
    fx.teardown().await;
}

/// Restart after the snapshot landed but before the tree went: the next pass
/// removes it and adds no second commit.
#[tokio::test]
async fn a_restart_between_snapshot_and_removal_finishes_on_the_next_pass() {
    let fx = Fixture::new().await;
    let (thread_id, worktree, branch) = fx.archived_thread(PAST_GRACE).await;
    make_dirty(&worktree).await;
    // What the pass that died had done.
    commit_worktree_or_err(&worktree, SNAPSHOT_COMMIT_MESSAGE)
        .await
        .expect("snapshot");
    let snapshot = tip(&fx.root, &branch).await;

    let events = fx.run_pass().await;

    assert_eq!(cleaned(&events, thread_id), vec![(2, false)]);
    assert!(!worktree.exists());
    assert_eq!(tip(&fx.root, &branch).await, snapshot);
    fx.teardown().await;
}

/// Restart after the tree was moved aside but before it was deleted. Nothing
/// partial is left at the thread path, the branch is free to check out again,
/// and the next pass deletes the leftover.
#[tokio::test]
async fn a_restart_mid_removal_leaves_no_partial_tree_and_the_next_pass_finishes() {
    let fx = Fixture::new().await;
    let (_thread_id, worktree, branch) = fx.archived_thread(PAST_GRACE).await;
    let before = tip(&fx.root, &branch).await;

    let aside = move_worktree_aside(&worktree, &fx.root)
        .await
        .expect("moved aside");
    // The engine dies here.
    assert!(
        !worktree.exists(),
        "the thread path is empty, not half-deleted"
    );
    assert!(aside.exists());
    assert!(
        aside
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(super::RECLAIMING_PREFIX),
        "the leftover names itself"
    );

    fx.run_pass().await;

    assert!(!aside.exists(), "the next pass deletes the leftover");
    assert_eq!(
        tip(&fx.root, &branch).await,
        before,
        "the branch is untouched"
    );
    let resumed = git_cmd(
        &["worktree", "add", &worktree.to_string_lossy(), &branch],
        &fx.root,
    )
    .await
    .expect("git worktree add");
    assert!(
        resumed.status.success(),
        "a resume can check the branch out again: {}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    fx.teardown().await;
}

/// A leftover from an earlier crashed removal of the same tree does not block
/// the next removal.
#[tokio::test]
async fn a_leftover_of_the_same_tree_does_not_block_its_removal() {
    let fx = Fixture::new().await;
    let (thread_id, worktree, _) = fx.archived_thread(PAST_GRACE).await;
    let name = worktree.file_name().unwrap().to_string_lossy().into_owned();
    let leftover = worktrees_dir(&fx.root).join(format!("{}{name}", super::RECLAIMING_PREFIX));
    tokio::fs::create_dir_all(leftover.join("target"))
        .await
        .unwrap();
    tokio::fs::write(leftover.join("target/big.bin"), vec![0u8; 1024])
        .await
        .unwrap();

    let events = fx.run_pass().await;

    assert_eq!(cleaned(&events, thread_id), vec![(2, false)]);
    assert!(!worktree.exists());
    assert!(!leftover.exists());
    fx.teardown().await;
}

/// Idle for the first `n` liveness checks, live from then on.
struct LiveAfterChecks {
    idle_checks: usize,
    seen: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl super::ActiveThreads for LiveAfterChecks {
    async fn is_active(&self, _thread_id: Uuid) -> bool {
        self.seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= self.idle_checks
    }
}

/// The snapshot strips build output and commits, which takes minutes. A
/// session that starts meanwhile keeps the tree; the snapshot stays on the
/// branch, so nothing is lost either way.
#[tokio::test]
async fn a_session_that_starts_during_the_snapshot_keeps_the_tree() {
    let fx = Fixture::new().await;
    let (thread_id, worktree, branch) = fx.archived_thread(PAST_GRACE).await;
    make_dirty(&worktree).await;
    // Idle at the pass's first check and the one before the snapshot.
    let active = Arc::new(LiveAfterChecks {
        idle_checks: 2,
        seen: Default::default(),
    });

    let events = fx.run_pass_with(active).await;

    assert!(!removed(&events, thread_id));
    assert!(worktree.exists(), "the live session keeps its tree");
    assert_eq!(
        git_out(&fx.root, &["log", "-1", "--format=%s", &branch])
            .await
            .as_deref(),
        Some(SNAPSHOT_COMMIT_MESSAGE),
        "the snapshot it made is on the branch"
    );
    fx.teardown().await;
}

/// A clone inside the tree is a repository of its own. `git add -A` would
/// record only a pointer to it, so removing the tree would lose its files.
#[tokio::test]
async fn a_nested_repository_keeps_the_tree() {
    let fx = Fixture::new().await;
    let (thread_id, worktree, branch) = fx.archived_thread(PAST_GRACE).await;
    let before = tip(&fx.root, &branch).await;
    let clone = worktree.join("vendor/some-clone");
    tokio::fs::create_dir_all(&clone).await.unwrap();
    git_cmd(&["init", "-q"], &clone).await.unwrap();
    tokio::fs::write(clone.join("work.txt"), "kept only here")
        .await
        .unwrap();

    let events = fx.run_pass().await;

    assert!(!removed(&events, thread_id));
    assert!(clone.join("work.txt").exists(), "the nested work stays");
    assert_eq!(tip(&fx.root, &branch).await, before, "no pointer commit");
    fx.teardown().await;
}

/// Commits on a detached HEAD are on no branch, so removing the tree could
/// lose them.
#[tokio::test]
async fn a_detached_head_keeps_the_tree() {
    let fx = Fixture::new().await;
    let (thread_id, worktree, _) = fx.archived_thread(PAST_GRACE).await;
    git_cmd(&["checkout", "-q", "--detach"], &worktree)
        .await
        .unwrap();
    git_cmd(
        &["commit", "-q", "--allow-empty", "-m", "on no branch"],
        &worktree,
    )
    .await
    .unwrap();

    let events = fx.run_pass().await;

    assert!(!removed(&events, thread_id));
    assert!(worktree.exists());
    fx.teardown().await;
}

/// Under hard disk pressure the grace drops to zero. Liveness, not idle age,
/// protects a tree then.
#[tokio::test]
async fn hard_pressure_removes_an_archived_tree_inside_the_grace() {
    let fx = Fixture::new().await;
    let (thread_id, worktree, branch) = fx.archived_thread(5 * 60).await;
    let before = tip(&fx.root, &branch).await;
    let rx = fx.bus.subscribe();
    let mut worker = make_worker(fx.pool.clone(), fx.bus.clone(), fx.root.clone());
    worker.free_hard_bytes = u64::MAX;
    worker.free_soft_bytes = u64::MAX;

    worker.run_once().await;

    let events = drain_cleaned_events(rx, Duration::from_millis(200)).await;
    assert!(removed(&events, thread_id));
    assert!(!worktree.exists());
    assert_eq!(tip(&fx.root, &branch).await, before, "the branch stays");
    fx.teardown().await;
}
