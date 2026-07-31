//! Tests for the background worktree cleanup worker (Phase 10.2 + 10.3).
//!
//! Each test builds a real workspace dir under `tempfile::tempdir()` with a
//! `.lucidos/worktrees/thread-<short>` worktree inside a real git repo, seeds
//! the events table to control "idle age", and drives [`WorktreeCleanup::run_once`]
//! directly so we don't sleep on the hour-long real-world cycle.

use std::collections::HashSet;
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

use super::{ActiveThreads, WorktreeCleanup};

/// Captured at construction — matches what production sees in one pass.
struct StaticActiveThreads(HashSet<Uuid>);

#[async_trait::async_trait]
impl ActiveThreads for StaticActiveThreads {
    async fn is_active(&self, thread_id: Uuid) -> bool {
        self.0.contains(&thread_id)
    }
}

pub(crate) fn no_active_threads() -> Arc<dyn ActiveThreads> {
    Arc::new(StaticActiveThreads(HashSet::new()))
}

pub(crate) fn active_threads(ids: &[Uuid]) -> Arc<dyn ActiveThreads> {
    Arc::new(StaticActiveThreads(ids.iter().copied().collect()))
}

/// Build a minimal workspace + repo on disk: a tempdir, an initialized git
/// repo at its root with one commit on `main`, and `.lucidos/worktrees/`
/// pre-created. Returns `(workspace_root, repo_root)` — for our tests they
/// are the same path because `git_ops::main_worktree()` resolves from the
/// process's compile-time path. We honor that by `cd`-ing into the workspace
/// for git operations done via `git_cmd`.
pub(crate) async fn fresh_workspace() -> (tempfile::TempDir, PathBuf) {
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
    tokio::fs::write(root.join("seed.txt"), "seed")
        .await
        .unwrap();
    git_cmd(&["add", "."], &root).await.unwrap();
    git_cmd(&["commit", "-m", "seed"], &root).await.unwrap();
    let _ = worktrees_dir(&root); // ensure dir exists
    (tmp, root)
}

/// Add a worktree on a fresh branch derived from the thread id and write some
/// junk files into it. Returns the worktree path. When `with_artifacts=true`
/// we plant `target/` and `node_modules/` so Tier 1 has something to reclaim.
pub(crate) async fn add_worktree_for_thread(
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

    // Commit so the branch is ahead of main; without this Tier 0 would
    // claim every fixture worktree, masking the Tier 1/2 behavior under
    // test. Tier-0-eligible tests use `add_worktree_at_main_for_thread`.
    tokio::fs::write(worktree.join("branch_marker.txt"), b"on branch")
        .await
        .unwrap();
    git_cmd(&["add", "branch_marker.txt"], &worktree)
        .await
        .unwrap();
    git_cmd(&["commit", "-m", "branch work"], &worktree)
        .await
        .unwrap();

    worktree
}

/// Like `add_worktree_for_thread` but leaves the branch at main HEAD with no
/// commits — the state Tier 0 looks for (after Apply has merged the thread's
/// work and `change_ops::reset_worktree_to_main_after_apply` has reset the
/// working tree).
pub(crate) async fn add_worktree_at_main_for_thread(repo_root: &Path, thread_id: Uuid) -> PathBuf {
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
    worktree
}

/// Strand a worktree the way the real bug does: read its `.git` link and delete
/// the admin dir under `<repo>/.git/worktrees/<name>`, leaving the working dir
/// on disk but git-detached — every `git` call from it then fails.
pub(crate) async fn strand_worktree(worktree: &Path) {
    let dotgit = worktree.join(".git");
    let content = tokio::fs::read_to_string(&dotgit)
        .await
        .expect("read worktree .git file");
    let target = content
        .trim()
        .strip_prefix("gitdir:")
        .expect("gitdir: line")
        .trim();
    let admin = {
        let p = PathBuf::from(target);
        if p.is_absolute() {
            p
        } else {
            worktree.join(p)
        }
    };
    tokio::fs::remove_dir_all(&admin)
        .await
        .expect("remove admin dir to strand worktree");
}

/// Add a temporary worktree (`<prefix><change_id.as_simple()>`) on a fresh
/// branch, mirroring what the apply/harden/merge flows create. Returns the path.
pub(crate) async fn add_temp_worktree(
    repo_root: &Path,
    prefix: &str,
    change_id: Uuid,
    branch: &str,
) -> PathBuf {
    let dir_name = format!("{}{}", prefix, change_id.simple());
    let worktree = worktrees_dir(repo_root).join(&dir_name);
    git_cmd(
        &[
            "worktree",
            "add",
            "-b",
            branch,
            &worktree.to_string_lossy(),
            "main",
        ],
        repo_root,
    )
    .await
    .expect("git worktree add temp");
    worktree
}

/// Insert a `changes` row with the given status — the temp-sweep DB liveness
/// gate keys off `status` (a `pending` row keeps the temp worktree alive).
pub(crate) async fn insert_change(
    pool: &PgPool,
    change_id: Uuid,
    branch: &str,
    repo_root: &Path,
    status: &str,
) {
    sqlx::query(
        "INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, status, created_at) \
         VALUES ($1, $2, $3, $4, $5, 0, '{}'::text[], false, $6, NOW())",
    )
    .bind(change_id)
    .bind(Uuid::new_v4())
    .bind(branch)
    .bind(repo_root.to_string_lossy().to_string())
    .bind("temp sweep test change")
    .bind(status)
    .execute(pool)
    .await
    .expect("insert change row");
}

/// Insert a thread_summaries row matching the deterministic id, optionally
/// saved. Archive state defaults to `'inbox'` (a non-archived thread the user
/// might still return to).
pub(crate) async fn insert_thread_summary(pool: &PgPool, thread_id: Uuid, is_saved: bool) {
    insert_thread_summary_with_archive(pool, thread_id, is_saved, "inbox").await;
}

/// Insert a thread_summaries row with an explicit `archive_state` — used by the
/// retention tests, where `'archived'` is the signal that lets the cleanup
/// worker reclaim a worktree even while free disk is comfortable.
pub(crate) async fn insert_thread_summary_with_archive(
    pool: &PgPool,
    thread_id: Uuid,
    is_saved: bool,
    archive_state: &str,
) {
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_saved, archive_state) \
         VALUES ($1, 'cleanup test', 'claude_code', 1, NOW(), false, $2, $3)",
    )
    .bind(thread_id)
    .bind(is_saved)
    .bind(archive_state)
    .execute(pool)
    .await
    .expect("insert thread_summary");
}

/// Insert a backdated MessageReceived event for the given thread. `age_secs`
/// controls how old it appears so the cleanup worker classifies the thread as
/// idle.
pub(crate) async fn insert_old_event(pool: &PgPool, thread_id: Uuid, age_secs: i64) {
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

/// Insert a backdated `ChildThreadCompleted` event on `parent_id` referencing
/// `child_id`. Used by the fan-in retention tests (ADR 0011, B2): when this is
/// the parent's latest event, the parent still owes a resume and its worktree
/// must be kept. `sequence` is bigserial, so inserting this after the parent's
/// other events makes it the latest by sequence; `age_secs` keeps `created`
/// backdated so the thread still reads as idle.
pub(crate) async fn insert_child_completed_event(
    pool: &PgPool,
    parent_id: Uuid,
    child_id: Uuid,
    age_secs: i64,
) {
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, 'ChildThreadCompleted', $3, NOW() - make_interval(secs => $4), $2::uuid)",
    )
    .bind(Uuid::new_v4())
    .bind(parent_id)
    .bind(json!({
        "child_thread_id": child_id.to_string(),
        "status": "success",
        "summary": "done",
    }))
    .bind(age_secs as f64)
    .execute(pool)
    .await
    .expect("insert ChildThreadCompleted event");
}

/// Set a thread's `active_children_count` directly — the fan-in retention tests
/// use this to simulate a parent with a direct child still running.
pub(crate) async fn set_active_children_count(pool: &PgPool, thread_id: Uuid, count: i32) {
    sqlx::query("UPDATE thread_summaries SET active_children_count = $2 WHERE thread_id = $1")
        .bind(thread_id)
        .bind(count)
        .execute(pool)
        .await
        .expect("set active_children_count");
}

/// Build a worker pre-configured for tests: short interval + zero free-disk
/// thresholds so the alert never fires by accident. Tests that need a trigger
/// override these fields directly.
pub(crate) fn make_worker(pool: PgPool, bus: Arc<EventBus>, workspace: PathBuf) -> WorktreeCleanup {
    make_worker_with_active(pool, bus, workspace, no_active_threads())
}

pub(crate) fn make_worker_with_active(
    pool: PgPool,
    bus: Arc<EventBus>,
    workspace: PathBuf,
    active_threads: Arc<dyn ActiveThreads>,
) -> WorktreeCleanup {
    let changes = crate::core::changes_projection::ChangesProjection::new(pool.clone());
    WorktreeCleanup {
        pool,
        bus,
        workspace_root: workspace,
        interval: Duration::from_secs(60),
        free_soft_bytes: 0,
        free_hard_bytes: 0,
        force_tier1_idle: Duration::from_secs(60 * 60),
        // Default to the production grace windows; tests that drive removal of a
        // freshly-created fixture override these to ZERO (mtime-gated paths) or
        // rely on backdated events (the Some-arm stranded path).
        stranded_grace: super::STRANDED_GRACE,
        temp_worktree_grace: super::TEMP_WORKTREE_GRACE,
        // Default to the production threshold; tests that exercise the
        // small-vs-large branching override this.
        large_footprint_bytes: super::LARGE_FOOTPRINT_BYTES,
        alerts: Mutex::new(super::AlertState::default()),
        changes,
        active_threads,
    }
}

/// Drain a pre-existing receiver for WorktreeCleaned events until `deadline`.
/// The receiver MUST be created before the producer emits — broadcast channels
/// drop messages for non-subscribed listeners.
pub(crate) async fn drain_cleaned_events(
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
        if let BusEvent::Thread {
            thread_id,
            event:
                ThreadEvent::WorktreeCleaned {
                    tier,
                    freed_bytes,
                    branch_deleted,
                },
            ..
        } = typed
        {
            out.push((thread_id, tier, freed_bytes, branch_deleted));
        }
    }
    out
}

/// Drain a pre-existing receiver for SystemEvent::NotificationCreated events.
pub(crate) async fn drain_notifications(
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
        if let BusEvent::System(SystemEvent::NotificationCreated { title, message, .. }) = typed {
            out.push((title, message));
        }
    }
    out
}

const ONE_DAY: i64 = 24 * 60 * 60;
pub(crate) const TIER_2_AGE: i64 = 31 * ONE_DAY; // > 30 days
pub(crate) const TIER_1_AGE: i64 = 25 * 60 * 60; // > 24 hours
pub(crate) const TIER_0_AGE_SECS: i64 = 90 * 60; // > 1h grace
