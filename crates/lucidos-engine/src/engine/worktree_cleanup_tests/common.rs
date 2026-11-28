use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use sqlx::PgPool;
use tokio::sync::Notify;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::core::changes::ChangeStatus;
use crate::engine::event_bus::{BusEvent, EmittedEvent, EventBus, SystemEvent};
use crate::engine::git_ops::{git_cmd, worktrees_dir};
use crate::engine::thread_events::ThreadEvent;
use crate::scheduler::notifications::{NavigateTarget, Tap};

use super::{ActiveThreads, DiskMonitor, DiskReading, FreeDiskProbe, WorktreeCleanup};

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

struct LiveAfterFirstCheck(AtomicUsize);

#[async_trait::async_trait]
impl ActiveThreads for LiveAfterFirstCheck {
    async fn is_active(&self, _thread_id: Uuid) -> bool {
        self.0.fetch_add(1, Ordering::SeqCst) > 0
    }
}

pub(crate) fn live_after_first_check() -> Arc<dyn ActiveThreads> {
    Arc::new(LiveAfterFirstCheck(AtomicUsize::new(0)))
}

pub(crate) async fn fresh_workspace() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let root = tmp.path().to_path_buf();
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
    let _ = worktrees_dir(&root);
    (tmp, root)
}

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
        tokio::fs::write(target.join("big.bin"), vec![0u8; 4096])
            .await
            .unwrap();
        tokio::fs::write(node_modules.join("pkg.json"), b"{}")
            .await
            .unwrap();
        tokio::fs::write(cache.join("c.dat"), b"x").await.unwrap();
    }

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

pub(crate) async fn insert_change(
    pool: &PgPool,
    change_id: Uuid,
    branch: &str,
    repo_root: &Path,
    status: ChangeStatus,
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

pub(crate) async fn insert_pending_change_for_thread(
    pool: &PgPool,
    thread_id: Uuid,
    repo_root: &Path,
) {
    let change_id = Uuid::new_v4();
    insert_change(
        pool,
        change_id,
        "test/pending",
        repo_root,
        ChangeStatus::Pending,
    )
    .await;
    sqlx::query("UPDATE changes SET thread_id = $2 WHERE id = $1")
        .bind(change_id)
        .bind(thread_id)
        .execute(pool)
        .await
        .expect("tie change to thread");
}

pub(crate) async fn insert_thread_summary(pool: &PgPool, thread_id: Uuid, is_saved: bool) {
    insert_thread_summary_with_archive(pool, thread_id, is_saved, "inbox").await;
}

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

pub(crate) async fn insert_session_started(
    pool: &PgPool,
    thread_id: Uuid,
    branch: &str,
    age_secs: i64,
) {
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, 'SessionStarted', $3, NOW() - make_interval(secs => $4), $2::uuid)",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id)
    .bind(json!({ "branch": branch }))
    .bind(age_secs as f64)
    .execute(pool)
    .await
    .expect("insert SessionStarted event");
}

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

pub(crate) async fn insert_child_stopped_event(
    pool: &PgPool,
    parent_id: Uuid,
    child_id: Uuid,
    age_secs: i64,
) {
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, 'ChildThreadStopped', $3, NOW() - make_interval(secs => $4), $2::uuid)",
    )
    .bind(Uuid::new_v4())
    .bind(parent_id)
    .bind(json!({ "child_thread_id": child_id.to_string() }))
    .bind(age_secs as f64)
    .execute(pool)
    .await
    .expect("insert ChildThreadStopped event");
}

pub(crate) async fn insert_stopped_child(pool: &PgPool, parent_id: Uuid, child_id: Uuid) {
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, \
                                       parent_thread_id, parent_callback_pending, is_stopped_child) \
         VALUES ($1, 'stopped child', 'claude_code', 1, NOW(), $2, TRUE, TRUE)",
    )
    .bind(child_id)
    .bind(parent_id)
    .execute(pool)
    .await
    .expect("insert stopped child");
}

pub(crate) async fn set_active_children_count(pool: &PgPool, thread_id: Uuid, count: i32) {
    sqlx::query("UPDATE thread_summaries SET active_children_count = $2 WHERE thread_id = $1")
        .bind(thread_id)
        .bind(count)
        .execute(pool)
        .await
        .expect("set active_children_count");
}

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
        workspace_root: workspace.clone(),
        interval: Duration::from_secs(60),
        soft_cap_bytes: 0,
        hard_cap_bytes: 0,
        force_tier1_idle: Duration::from_secs(60 * 60),
        stranded_grace: super::STRANDED_GRACE,
        temp_worktree_grace: super::TEMP_WORKTREE_GRACE,
        size_walk: super::walk_directory_size_bytes,
        free_disk: real_free_space_probe(workspace.clone()),
        changes,
        active_threads,
        cleanup_wake: Arc::new(Notify::new()),
    }
}

pub(crate) const GB: u64 = 1024 * 1024 * 1024;

pub(crate) fn on_a_huge_volume(free: u64) -> DiskReading {
    DiskReading {
        free_bytes: free,
        total_bytes: u64::MAX,
    }
}

pub(crate) fn real_free_space_probe(workspace: PathBuf) -> FreeDiskProbe {
    let os_probe = super::os_free_disk_probe(workspace);
    Arc::new(move || os_probe().map(|reading| on_a_huge_volume(reading.free_bytes)))
}

pub(crate) fn settable_probe(initial: Option<u64>) -> (FreeDiskProbe, Arc<Mutex<Option<u64>>>) {
    let reading = Arc::new(Mutex::new(initial));
    let probe_reading = reading.clone();
    let probe: FreeDiskProbe =
        Arc::new(move || (*probe_reading.lock().unwrap()).map(on_a_huge_volume));
    (probe, reading)
}

pub(crate) fn scripted_probe(readings: &[u64]) -> FreeDiskProbe {
    let queue = Mutex::new(readings.iter().copied().collect::<VecDeque<u64>>());
    Arc::new(move || {
        let mut queue = queue.lock().unwrap();
        let free = if queue.len() > 1 {
            queue.pop_front()
        } else {
            queue.front().copied()
        };
        free.map(on_a_huge_volume)
    })
}

pub(crate) fn make_monitor(
    bus: Arc<EventBus>,
    workspace: PathBuf,
    probe: FreeDiskProbe,
) -> DiskMonitor {
    let mut monitor = DiskMonitor::new(bus, workspace, Arc::new(Notify::new()));
    monitor.free_disk = probe;
    monitor
}

pub(crate) async fn was_woken(wake: &Notify) -> bool {
    tokio::time::timeout(Duration::from_millis(10), wake.notified())
        .await
        .is_ok()
}

pub(crate) async fn unresponsive_pool() -> PgPool {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind unresponsive listener");
    let port = listener.local_addr().expect("listener addr").port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(60))
        .connect_lazy(&format!(
            "postgres://lucidos:lucidos@127.0.0.1:{port}/stalled"
        ))
        .expect("lazy pool")
}

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

#[derive(Debug, Clone)]
pub(crate) struct CapturedNotification {
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) tap: Tap,
}

impl CapturedNotification {
    pub(crate) fn settings_view(&self) -> Option<&str> {
        match &self.tap {
            Tap::Navigate { to } if to.target == NavigateTarget::Settings => {
                to.settings_view.as_deref()
            }
            _ => None,
        }
    }
}

pub(crate) async fn drain_notifications(
    rx: tokio::sync::broadcast::Receiver<EmittedEvent>,
    deadline: Duration,
) -> Vec<CapturedNotification> {
    let mut stream = BroadcastStream::new(rx);
    let mut out = Vec::new();
    let until = tokio::time::Instant::now() + deadline;
    while let Ok(Some(item)) = tokio::time::timeout_at(until, stream.next()).await {
        let Ok(EmittedEvent { typed, .. }) = item else {
            continue;
        };
        if let BusEvent::System(SystemEvent::NotificationCreated {
            title,
            message,
            tap,
            ..
        }) = typed
        {
            out.push(CapturedNotification {
                title,
                message,
                tap,
            });
        }
    }
    out
}

const ONE_DAY: i64 = 24 * 60 * 60;
pub(crate) const TIER_2_AGE: i64 = 31 * ONE_DAY;
pub(crate) const TIER_1_AGE: i64 = 25 * 60 * 60;
pub(crate) const TIER_0_AGE_SECS: i64 = 90 * 60;
