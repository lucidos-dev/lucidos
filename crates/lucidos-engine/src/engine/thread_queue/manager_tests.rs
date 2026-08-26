//! Integration tests for the [`ThreadQueue`] manager: real Postgres
//! (`setup_test_db`) + real EventBus + a gated mock executor, so admission /
//! queueing / drain / recovery mechanics are exercised end-to-end without an
//! LLM. The pure decision logic is tested separately in `policy.rs`.

use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Executor whose `execute` parks until the test releases a permit —
/// `release_one()` lets exactly one execution finish, so tests control slot
/// lifetimes deterministically.
struct GatedExecutor {
    gate: Arc<tokio::sync::Semaphore>,
    executed: Arc<std::sync::Mutex<Vec<Uuid>>>,
    prepared: Arc<AtomicUsize>,
}

impl GatedExecutor {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            gate: Arc::new(tokio::sync::Semaphore::new(0)),
            executed: Arc::new(std::sync::Mutex::new(Vec::new())),
            prepared: Arc::new(AtomicUsize::new(0)),
        })
    }

    fn release_one(&self) {
        self.gate.add_permits(1);
    }

    fn executed_ids(&self) -> Vec<Uuid> {
        self.executed.lock().unwrap().clone()
    }
}

#[async_trait]
impl ThreadQueueExecutor for GatedExecutor {
    async fn prepare(&self, _request: &mut ThreadQueueRequest) {
        self.prepared.fetch_add(1, Ordering::SeqCst);
    }

    async fn execute(&self, entry: ExecutableEntry) {
        self.executed.lock().unwrap().push(entry.id);
        let permit = self
            .gate
            .acquire()
            .await
            .expect("gate semaphore closed mid-test");
        permit.forget();
    }
}

/// Executor that records the chain depth each hook observes, and parks
/// `execute` on a gate like [`GatedExecutor`] so the test controls slot
/// lifetimes.
///
/// The bug this exists for was invisible from outside the queue. `prepare` is
/// awaited inline by `submit` and read the fire's depth. `execute` runs one
/// `tokio::spawn` later and read 0, because a task-local does not follow a
/// spawn.
struct DepthProbeExecutor {
    gate: Arc<tokio::sync::Semaphore>,
    prepared: Arc<std::sync::Mutex<Vec<u32>>>,
    executed: Arc<std::sync::Mutex<Vec<u32>>>,
    /// Fires as `execute` starts, so a test can await the spawned task instead
    /// of sleeping for it.
    entered: tokio::sync::mpsc::UnboundedSender<u32>,
}

impl DepthProbeExecutor {
    fn new() -> (Arc<Self>, tokio::sync::mpsc::UnboundedReceiver<u32>) {
        let (entered, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Arc::new(Self {
                gate: Arc::new(tokio::sync::Semaphore::new(0)),
                prepared: Arc::new(std::sync::Mutex::new(Vec::new())),
                executed: Arc::new(std::sync::Mutex::new(Vec::new())),
                entered,
            }),
            rx,
        )
    }

    fn release_one(&self) {
        self.gate.add_permits(1);
    }

    fn prepared_depths(&self) -> Vec<u32> {
        self.prepared.lock().unwrap().clone()
    }

    fn executed_depths(&self) -> Vec<u32> {
        self.executed.lock().unwrap().clone()
    }
}

#[async_trait]
impl ThreadQueueExecutor for DepthProbeExecutor {
    async fn prepare(&self, _request: &mut ThreadQueueRequest) {
        self.prepared
            .lock()
            .unwrap()
            .push(crate::scheduler::user_tasks::current_event_trigger_depth());
    }

    async fn execute(&self, _entry: ExecutableEntry) {
        let depth = crate::scheduler::user_tasks::current_event_trigger_depth();
        self.executed.lock().unwrap().push(depth);
        let _ = self.entered.send(depth);
        let permit = self
            .gate
            .acquire()
            .await
            .expect("gate semaphore closed mid-test");
        permit.forget();
    }
}

fn test_policy(max_total: usize) -> CapacityPolicy {
    CapacityPolicy {
        max_concurrent_total: max_total,
        max_concurrent_event_trigger: max_total,
        max_concurrent_cron: max_total,
        max_concurrent_sub_thread: max_total,
        max_concurrent_coding_agent: max_total,
        max_concurrent_per_trigger: 1,
        max_queued_per_trigger: 2,
        // Background-only tests want no reserved floor (it only governs the
        // background-vs-user reclaim priority); user-priority tests set it
        // explicitly via set_policy.
        reserved_background: 0,
        overflow: OverflowPolicy::DropOldest,
        max_event_trigger_depth: DEFAULT_MAX_EVENT_TRIGGER_DEPTH,
    }
}

/// Acquire a user-initiated pool slot in a detached task, holding the guard
/// until the returned sender is dropped (or fired). While the task waits for a
/// slot the entry shows as `queued` in `user_entries()`; once admitted it
/// shows as `admitted`. Dropping the sender releases the slot.
fn spawn_user_slot(queue: Arc<ThreadQueue>, thread_id: Uuid) -> oneshot::Sender<()> {
    let (release_tx, release_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _guard = queue
            .acquire_user_slot(Some(thread_id), "user message".to_string())
            .await;
        let _ = release_rx.await; // hold the slot until the test releases it
                                  // _guard drops here → slot released + drain.
    });
    release_tx
}

/// Status of the in-memory user-initiated entry for `thread_id`, if present.
async fn user_status(queue: &ThreadQueue, thread_id: Uuid) -> Option<&'static str> {
    queue
        .user_entries()
        .await
        .into_iter()
        .find(|e| e.thread_id == Some(thread_id))
        .map(|e| e.status)
}

fn test_trigger_config(id: &str) -> crate::triggers::TriggerConfig {
    crate::triggers::TriggerConfig {
        id: id.to_string(),
        name: format!("Trigger {id}"),
        slug: id.to_string(),
        schedule: vec![],
        timezone: "UTC".to_string(),
        run: crate::triggers::TriggerRun::Intent {
            intent: "test intent".to_string(),
        },
        on: vec![],
        paused: false,
        last_run: None,
        last_run_status: None,
        app_id: None,
        go_to_review: false,
        group_id: None,
        side_effect_grant: vec![],
        plugin_id: None,
        model: None,
        reasoning_effort: None,
    }
}

fn event_trigger_request(trigger_id: &str) -> ThreadQueueRequest {
    ThreadQueueRequest::EventTrigger {
        trigger_id: trigger_id.to_string(),
        event_type: "TestEvent".to_string(),
        event_payload: serde_json::json!({"n": 1}),
        depth: 1,
        origin_thread_id: None,
        source_event_id: None,
    }
}

fn sub_thread_request(child_thread_id: Uuid) -> ThreadQueueRequest {
    ThreadQueueRequest::SubThread {
        // Stamped by `submit` from the submitting task's chain depth.
        depth: 0,
        prompt: "do the thing".to_string(),
        child_thread_id,
        parent_thread_id: None,
        spawning_event_id: None,
        title: None,
        model: None,
        reasoning_effort: None,
        pre_emitted_origin: None,
        origin: None,
    }
}

fn cron_request(trigger_id: &str) -> ThreadQueueRequest {
    ThreadQueueRequest::Cron {
        trigger_id: trigger_id.to_string(),
    }
}

struct Fixture {
    pool: sqlx::PgPool,
    db: String,
    bus: EventBus,
    queue: Arc<ThreadQueue>,
    executor: Arc<GatedExecutor>,
    trigger_configs: Arc<std::sync::RwLock<HashMap<String, crate::triggers::TriggerConfig>>>,
    /// Where the overflow guard's `TriggerDisabled` would project the on-disk
    /// trigger definition. Held so it outlives every queue built from it.
    workspace: tempfile::TempDir,
}

async fn fixture(max_total: usize) -> Fixture {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let trigger_configs: Arc<std::sync::RwLock<HashMap<String, crate::triggers::TriggerConfig>>> =
        Arc::new(std::sync::RwLock::new(HashMap::new()));
    let workspace = tempfile::tempdir().expect("temp workspace");
    let (queue, executor) = gated_queue(&pool, &bus, &trigger_configs, workspace.path(), max_total);
    Fixture {
        pool,
        db,
        bus,
        queue,
        executor,
        trigger_configs,
        workspace,
    }
}

async fn row_status(pool: &sqlx::PgPool, entry_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM thread_queue WHERE id = $1")
        .bind(entry_id)
        .fetch_optional(pool)
        .await
        .expect("thread_queue query")
}

async fn row_thread_id(pool: &sqlx::PgPool, entry_id: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT thread_id FROM thread_queue WHERE id = $1")
        .bind(entry_id)
        .fetch_one(pool)
        .await
        .expect("thread_queue query")
}

/// `(queued_at, admitted_at)` for one entry, so a test can pin that recovery
/// and the follow-up admit leave the original stamps alone.
async fn row_stamps(pool: &sqlx::PgPool, entry_id: Uuid) -> (DateTime<Utc>, Option<DateTime<Utc>>) {
    sqlx::query_as("SELECT queued_at, admitted_at FROM thread_queue WHERE id = $1")
        .bind(entry_id)
        .fetch_one(pool)
        .await
        .expect("thread_queue stamps query")
}

/// How many times `event_type` was persisted for this entry. Recovery of a
/// `queued` row must add none: a re-emitted `ThreadQueued` would reset
/// `queued_at` and mark work as re-fired that never ran.
async fn entry_event_count(pool: &sqlx::PgPool, entry_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1 AND aggregate_id = $2")
        .bind(event_type)
        .bind(entry_id.to_string())
        .fetch_one(pool)
        .await
        .expect("events count query")
}

/// Create the `thread_summaries` row a started TRIGGER fire leaves behind.
/// `TriggerStarted` is the real starter event of a trigger thread. Its row is
/// what the boot sweep reads to decide an admitted entry already did its work.
async fn materialize_trigger_thread(bus: &EventBus, thread_id: Uuid, trigger_id: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: crate::engine::thread_events::ThreadEvent::TriggerStarted {
            trigger_id: trigger_id.to_string(),
            trigger_name: Some(format!("Trigger {trigger_id}")),
            prompt: Some("run it".to_string()),
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
            go_to_review: false,
            model: None,
            reasoning_effort: None,
        },
        meta: crate::engine::thread_events::EventMeta {
            channel: Some(crate::engine::thread_events::EventChannel::Trigger),
            ..crate::engine::thread_events::EventMeta::NONE
        },
    })
    .await
    .expect("TriggerStarted emit");
}

/// The `emitting_trigger_id` carried by each of one entry's three lifecycle
/// frames, in arrival order. Waits until all three have come off the bus.
async fn entry_frame_markers(
    rx: &mut tokio::sync::broadcast::Receiver<crate::engine::event_bus::EmittedEvent>,
    entry_id: Uuid,
) -> Vec<(&'static str, Option<String>)> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut seen: Vec<(&'static str, Option<String>)> = Vec::new();
        while seen.len() < 3 {
            let emitted = rx.recv().await.expect("bus channel stays open");
            let name = match &emitted.typed {
                BusEvent::System(SystemEvent::ThreadQueued { entry_id: id, .. })
                    if *id == entry_id =>
                {
                    "ThreadQueued"
                }
                BusEvent::System(SystemEvent::ThreadQueueAdmitted { entry_id: id, .. })
                    if *id == entry_id =>
                {
                    "ThreadQueueAdmitted"
                }
                BusEvent::System(SystemEvent::ThreadQueueCompleted { entry_id: id })
                    if *id == entry_id =>
                {
                    "ThreadQueueCompleted"
                }
                _ => continue,
            };
            seen.push((name, emitted.emitting_trigger_id.clone()));
        }
        seen
    })
    .await
    .expect("all three lifecycle frames arrive")
}

/// A manager wired to a fresh [`GatedExecutor`]. The one construction site, so
/// a new `ThreadQueue::new` argument lands in one place.
fn gated_queue(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, crate::triggers::TriggerConfig>>>,
    workspace: &std::path::Path,
    max_total: usize,
) -> (Arc<ThreadQueue>, Arc<GatedExecutor>) {
    let executor = GatedExecutor::new();
    let queue = queue_with_executor(
        pool,
        bus,
        trigger_configs,
        workspace,
        max_total,
        executor.clone(),
    );
    (queue, executor)
}

/// A manager wired to a caller-supplied executor. `set_executor` is a
/// `OnceLock`, so a fixture that wants a different probe has to build its own
/// queue rather than swap one in.
fn queue_with_executor(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    trigger_configs: &Arc<std::sync::RwLock<HashMap<String, crate::triggers::TriggerConfig>>>,
    workspace: &std::path::Path,
    max_total: usize,
    executor: Arc<dyn ThreadQueueExecutor>,
) -> Arc<ThreadQueue> {
    let queue = Arc::new(ThreadQueue::new(
        pool.clone(),
        bus.clone(),
        trigger_configs.clone(),
        workspace.to_path_buf(),
        Arc::new(tokio::sync::Mutex::new(())),
        test_policy(max_total),
    ));
    queue.set_executor(executor);
    queue
}

/// A fresh manager over the same database, standing in for the next engine
/// process. The old process's in-flight executions died with it.
fn restarted_queue(f: &Fixture, max_total: usize) -> (Arc<ThreadQueue>, Arc<GatedExecutor>) {
    gated_queue(
        &f.pool,
        &f.bus,
        &f.trigger_configs,
        f.workspace.path(),
        max_total,
    )
}

async fn cron_row_count(pool: &sqlx::PgPool, trigger_id: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM thread_queue WHERE trigger_id = $1")
        .bind(trigger_id)
        .fetch_one(pool)
        .await
        .expect("thread_queue count query")
}

/// Emit a `ThreadQueued` (+ optional `ThreadQueueAdmitted`) for a cron trigger
/// straight through the bus, creating a projection row WITHOUT going through
/// `submit` (which now coalesces). Reproduces the pre-restart projection state a
/// restart storm leaves behind — multiple cron rows for one trigger.
async fn emit_cron_queued(bus: &EventBus, trigger_id: &str, admitted: bool) -> Uuid {
    let entry_id = Uuid::new_v4();
    let request = serde_json::to_value(cron_request(trigger_id)).expect("serialize cron request");
    bus.emit(BusEvent::System(SystemEvent::ThreadQueued {
        entry_id,
        kind: ThreadQueueKind::Cron,
        trigger_id: Some(trigger_id.to_string()),
        trigger_name: Some(format!("Trigger {trigger_id}")),
        thread_id: None,
        summary: format!("{trigger_id} (scheduled)"),
        request,
        requeued: false,
        actor: None,
    }))
    .await
    .expect("ThreadQueued emit");
    if admitted {
        bus.emit(BusEvent::System(SystemEvent::ThreadQueueAdmitted {
            entry_id,
            thread_id: None,
            actor: None,
        }))
        .await
        .expect("ThreadQueueAdmitted emit");
    }
    entry_id
}

/// Poll until `cond` returns true (queue completion fans out through spawned
/// tasks, so DB state lands asynchronously after a release).
async fn wait_until<F, Fut>(mut cond: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if cond().await {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("condition not reached within 10s");
}

#[tokio::test]
async fn submit_within_capacity_admits_executes_and_completes() {
    let f = fixture(2).await;

    let outcome = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(outcome.admitted, "first submit must admit under capacity");
    assert_eq!(outcome.position, 0);
    // Persisted active-session record: row exists with status 'admitted'.
    assert_eq!(
        row_status(&f.pool, outcome.entry_id).await.as_deref(),
        Some("admitted")
    );
    assert_eq!(f.executor.executed_ids(), vec![outcome.entry_id]);
    // prepare ran inline before submit returned.
    assert_eq!(f.executor.prepared.load(Ordering::SeqCst), 1);

    // Work finishes → slot releases, row deleted, completion resolves.
    f.executor.release_one();
    outcome
        .completion
        .await
        .expect("completion channel must resolve");
    let pool = f.pool.clone();
    let id = outcome.entry_id;
    wait_until(|| {
        let pool = pool.clone();
        async move { row_status(&pool, id).await.is_none() }
    })
    .await;

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn submit_over_capacity_queues_then_drains_in_fifo_order() {
    let f = fixture(1).await;

    let a = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    let b = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    let c = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(a.admitted);
    assert!(!b.admitted, "over capacity → queued, not spawned");
    assert!(!c.admitted);
    assert_eq!(b.position, 1);
    assert_eq!(c.position, 2);
    assert_eq!(
        row_status(&f.pool, b.entry_id).await.as_deref(),
        Some("queued")
    );

    // A finishes → B (and only B) admits, FIFO.
    f.executor.release_one();
    let pool = f.pool.clone();
    let b_id = b.entry_id;
    wait_until(|| {
        let pool = pool.clone();
        async move { row_status(&pool, b_id).await.as_deref() == Some("admitted") }
    })
    .await;
    assert_eq!(
        row_status(&f.pool, c.entry_id).await.as_deref(),
        Some("queued"),
        "C must wait its turn"
    );
    assert_eq!(
        f.executor.executed_ids(),
        vec![a.entry_id, b.entry_id],
        "execution order must be FIFO"
    );

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn drop_entry_deletes_row_and_resolves_completion() {
    let f = fixture(1).await;

    let a = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    let b = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(!b.admitted);

    f.queue
        .drop_entry(b.entry_id, "test drop", None)
        .await
        .expect("queued entry must be droppable");
    b.completion
        .await
        .expect("drop must resolve the completion channel");
    assert_eq!(row_status(&f.pool, b.entry_id).await, None);

    // Dropping an admitted (running) entry is refused.
    let err = f
        .queue
        .drop_entry(a.entry_id, "test drop", None)
        .await
        .expect_err("running entries are not droppable");
    assert!(err.contains("not queued"), "got: {err}");

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn run_now_force_admits_ignoring_caps() {
    let f = fixture(1).await;

    let a = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    let b = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(a.admitted);
    assert!(!b.admitted);

    f.queue
        .run_now(b.entry_id, None)
        .await
        .expect("queued entry must be runnable now");
    assert_eq!(
        row_status(&f.pool, b.entry_id).await.as_deref(),
        Some("admitted"),
        "run-now bypasses the capacity check"
    );
    assert_eq!(f.executor.executed_ids(), vec![a.entry_id, b.entry_id]);

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn per_trigger_fifo_and_drop_oldest_overflow() {
    let f = fixture(10).await;
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    // per-trigger concurrency 1: first fire admits, the rest queue.
    let a = f
        .queue
        .submit(event_trigger_request("trig-a"), None, None)
        .await;
    let b = f
        .queue
        .submit(event_trigger_request("trig-a"), None, None)
        .await;
    let c = f
        .queue
        .submit(event_trigger_request("trig-a"), None, None)
        .await;
    assert!(a.admitted);
    assert!(!b.admitted, "per-trigger cap of 1 → second fire queues");
    assert!(!c.admitted);

    // max_queued_per_trigger = 2 → the next fire overflows; DropOldest
    // drops B (the oldest queued) and keeps the newcomer.
    let d = f
        .queue
        .submit(event_trigger_request("trig-a"), None, None)
        .await;
    assert!(!d.admitted);
    let pool = f.pool.clone();
    let b_id = b.entry_id;
    wait_until(|| {
        let pool = pool.clone();
        async move { row_status(&pool, b_id).await.is_none() }
    })
    .await;
    b.completion
        .await
        .expect("dropped entry must resolve its completion channel");
    assert_eq!(
        row_status(&f.pool, c.entry_id).await.as_deref(),
        Some("queued")
    );
    assert_eq!(
        row_status(&f.pool, d.entry_id).await.as_deref(),
        Some("queued")
    );

    // A different trigger is unaffected by trig-a's backlog (cross-trigger
    // is best-effort, capacity allows it).
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-b".to_string(), test_trigger_config("trig-b"));
    let other = f
        .queue
        .submit(event_trigger_request("trig-b"), None, None)
        .await;
    assert!(other.admitted, "other triggers admit independently");

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn cron_coalesces_redundant_fires_on_submit() {
    let f = fixture(5).await;
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    // First cron fire admits and runs (the executor parks it as the in-flight
    // fire).
    let a = f.queue.submit(cron_request("trig-a"), None, None).await;
    assert!(a.admitted, "first cron fire admits");
    assert_eq!(
        row_status(&f.pool, a.entry_id).await.as_deref(),
        Some("admitted")
    );

    // A second cron fire while one is active → coalesced: not admitted, no queue
    // row, and its completion resolves immediately so the cron loop / missed-grace
    // submitter proceeds to the next occurrence rather than hanging.
    let b = f.queue.submit(cron_request("trig-a"), None, None).await;
    assert!(!b.admitted, "redundant cron fire must not admit");
    assert_eq!(b.position, 0);
    assert_eq!(
        row_status(&f.pool, b.entry_id).await,
        None,
        "a coalesced fire creates no thread_queue row"
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), b.completion)
        .await
        .expect("coalesced completion must resolve immediately")
        .expect("completion channel ok");

    // A third fire also coalesces — the backlog can never grow past one.
    let c = f.queue.submit(cron_request("trig-a"), None, None).await;
    assert!(!c.admitted);
    assert_eq!(row_status(&f.pool, c.entry_id).await, None);

    // Only the first fire ever executed; exactly one cron row exists.
    assert_eq!(f.executor.executed_ids(), vec![a.entry_id]);
    assert_eq!(cron_row_count(&f.pool, "trig-a").await, 1);

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn cron_coalesces_against_a_queued_fire() {
    let f = fixture(1).await; // single slot
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    // Occupy the only slot with background work so the cron fire can't admit.
    let blocker = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(blocker.admitted);

    // First cron fire queues (pool full).
    let a = f.queue.submit(cron_request("trig-a"), None, None).await;
    assert!(!a.admitted);
    assert_eq!(
        row_status(&f.pool, a.entry_id).await.as_deref(),
        Some("queued")
    );

    // Second cron fire coalesces against the queued one — no second row.
    let b = f.queue.submit(cron_request("trig-a"), None, None).await;
    assert!(!b.admitted);
    assert_eq!(row_status(&f.pool, b.entry_id).await, None);
    assert_eq!(
        cron_row_count(&f.pool, "trig-a").await,
        1,
        "the queued cron backlog never deepens past one"
    );

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn recover_collapses_duplicate_cron_rows_to_one() {
    let f = fixture(5).await;
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    // Simulate the projection state a restart storm leaves: three cron rows for
    // one trigger (one in-flight `admitted` that died with the process, two
    // still `queued`).
    let admitted_id = emit_cron_queued(&f.bus, "trig-a", true).await;
    let queued1 = emit_cron_queued(&f.bus, "trig-a", false).await;
    let queued2 = emit_cron_queued(&f.bus, "trig-a", false).await;
    assert_eq!(cron_row_count(&f.pool, "trig-a").await, 3);

    let (queue2, executor2) = restarted_queue(&f, 5);
    queue2.recover_persisted_entries().await;

    // The duplicates are coalesced away — exactly one cron row survives (the
    // oldest, re-queued), and a drain re-fires it exactly once.
    assert_eq!(
        cron_row_count(&f.pool, "trig-a").await,
        1,
        "recovery collapses duplicate cron fires to a single entry"
    );
    assert!(
        row_status(&f.pool, queued1).await.is_none()
            && row_status(&f.pool, queued2).await.is_none(),
        "the later duplicate cron rows are dropped"
    );
    assert_eq!(
        row_status(&f.pool, admitted_id).await.as_deref(),
        Some("queued"),
        "the oldest cron fire is kept and re-queued"
    );
    // A drain re-fires exactly the single surviving cron entry (execution is
    // spawned, so poll rather than assert synchronously).
    queue2.drain().await;
    wait_until(|| {
        let ex = executor2.clone();
        async move { ex.executed_ids() == vec![admitted_id] }
    })
    .await;

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn drain_skips_paused_triggers_and_drops_deleted_ones() {
    let f = fixture(1).await;
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    let blocker = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    let fire = f
        .queue
        .submit(event_trigger_request("trig-a"), None, None)
        .await;
    assert!(blocker.admitted);
    assert!(!fire.admitted);

    // Pause the trigger, then free capacity: the fire must stay queued.
    f.trigger_configs
        .write()
        .unwrap()
        .get_mut("trig-a")
        .unwrap()
        .paused = true;
    f.executor.release_one();
    blocker.completion.await.expect("blocker completes");
    // Drain ran on completion; paused trigger entries stay queued.
    assert_eq!(
        row_status(&f.pool, fire.entry_id).await.as_deref(),
        Some("queued")
    );

    // Resume → drain admits it.
    f.trigger_configs
        .write()
        .unwrap()
        .get_mut("trig-a")
        .unwrap()
        .paused = false;
    f.queue.drain().await;
    assert_eq!(
        row_status(&f.pool, fire.entry_id).await.as_deref(),
        Some("admitted")
    );

    // A queued fire whose trigger was deleted gets dropped at drain time.
    let orphan = f
        .queue
        .submit(event_trigger_request("trig-a"), None, None)
        .await;
    assert!(
        !orphan.admitted,
        "per-trigger cap holds while the fire runs"
    );
    f.trigger_configs.write().unwrap().remove("trig-a");
    f.queue.drain().await;
    assert_eq!(row_status(&f.pool, orphan.entry_id).await, None);

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn recover_requeues_admitted_trigger_entries_after_restart() {
    let f = fixture(5).await;
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    let a = f
        .queue
        .submit(event_trigger_request("trig-a"), None, None)
        .await;
    assert!(a.admitted);
    assert_eq!(
        row_status(&f.pool, a.entry_id).await.as_deref(),
        Some("admitted")
    );

    // "Restart" with the entry still bound to no thread: the process died
    // before the fire minted one, so nothing ran and the fire is owed.
    let (queue2, executor2) = restarted_queue(&f, 5);
    queue2.recover_persisted_entries().await;

    // The admitted-but-dead fire re-queued (status back to 'queued')…
    assert_eq!(
        row_status(&f.pool, a.entry_id).await.as_deref(),
        Some("queued")
    );
    // …and a drain re-fires it.
    queue2.drain().await;
    assert_eq!(
        row_status(&f.pool, a.entry_id).await.as_deref(),
        Some("admitted")
    );
    assert_eq!(executor2.executed_ids(), vec![a.entry_id]);

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn recover_hands_off_spawn_entries_whose_thread_materialized() {
    let f = fixture(5).await;
    let child = Uuid::new_v4();

    let a = f.queue.submit(sub_thread_request(child), None, None).await;
    assert!(a.admitted);

    // Materialize the thread, which is what the dead process's execution did
    // before crashing.
    emit_message_received(&f.bus, child).await;

    let (queue2, _executor2) = restarted_queue(&f, 5);
    queue2.recover_persisted_entries().await;

    // The spawn already happened — the entry completes (row deleted) and
    // ownership passes to thread-level recovery (chat settle / CC resume).
    assert_eq!(row_status(&f.pool, a.entry_id).await, None);

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

/// THE restart-replay bug: a cron fire ran, parked on `ask_user_question`, and
/// the engine restarted. Parking emits no terminal event, so nothing ever
/// completed the entry and its row sat at `admitted`. The boot sweep re-queued
/// it and the whole trigger ran a second time, five minutes after the first.
///
/// The fire's thread is bound to the entry, so the sweep can see the work
/// already started and hand the thread to thread-level recovery instead.
#[tokio::test]
async fn recover_hands_off_a_cron_fire_whose_thread_started() {
    let f = fixture(5).await;
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    let entry_id = emit_cron_queued(&f.bus, "trig-a", true).await;
    let fired_thread = Uuid::new_v4();
    f.queue.record_entry_thread(entry_id, fired_thread).await;
    materialize_trigger_thread(&f.bus, fired_thread, "trig-a").await;

    let (queue2, executor2) = restarted_queue(&f, 5);
    queue2.recover_persisted_entries().await;

    assert_eq!(
        row_status(&f.pool, entry_id).await,
        None,
        "a fire that already started completes instead of re-queuing"
    );
    // Nothing re-fires. `prepare` is awaited INLINE inside `drain`, ahead of
    // the spawn, so a zero here is deterministic. An empty `executed_ids` would
    // only mean the spawned task had not run yet.
    queue2.drain().await;
    assert_eq!(
        executor2.prepared.load(Ordering::SeqCst),
        0,
        "the parked thread is thread-level recovery's to own, not the queue's"
    );

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

/// A handed-off fire must not consume its trigger's coalescing slot. The
/// handoff row already ran. The queued sibling behind it never did, so dropping
/// it as a "duplicate scheduled fire" would lose a run nobody notices.
#[tokio::test]
async fn a_handed_off_cron_fire_leaves_its_queued_sibling_alone() {
    let f = fixture(5).await;
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    let started = emit_cron_queued(&f.bus, "trig-a", true).await;
    let waiting = emit_cron_queued(&f.bus, "trig-a", false).await;
    let fired_thread = Uuid::new_v4();
    f.queue.record_entry_thread(started, fired_thread).await;
    materialize_trigger_thread(&f.bus, fired_thread, "trig-a").await;

    let (queue2, executor2) = restarted_queue(&f, 5);
    queue2.recover_persisted_entries().await;

    assert_eq!(
        row_status(&f.pool, started).await,
        None,
        "the fire that already ran hands off"
    );
    assert_eq!(
        row_status(&f.pool, waiting).await.as_deref(),
        Some("queued"),
        "the sibling that never ran survives the handoff"
    );
    queue2.drain().await;
    wait_until(|| {
        let ex = executor2.clone();
        async move { ex.executed_ids() == vec![waiting] }
    })
    .await;

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

/// The residual case that keeps the guard above from being over-eager. This
/// fire got as far as minting a thread id and no further, so nothing ran and
/// the fire is still owed.
///
/// Its sibling, an entry bound to no thread at all, is covered by
/// `recover_requeues_admitted_trigger_entries_after_restart`.
#[tokio::test]
async fn recover_requeues_an_admitted_cron_fire_whose_thread_never_existed() {
    let f = fixture(5).await;
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    let entry_id = emit_cron_queued(&f.bus, "trig-a", true).await;
    f.queue.record_entry_thread(entry_id, Uuid::new_v4()).await;

    let (queue2, executor2) = restarted_queue(&f, 5);
    queue2.recover_persisted_entries().await;

    assert_eq!(
        row_status(&f.pool, entry_id).await.as_deref(),
        Some("queued"),
        "no thread materialized, so the fire is re-queued"
    );
    queue2.drain().await;
    wait_until(|| {
        let ex = executor2.clone();
        async move { ex.executed_ids() == vec![entry_id] }
    })
    .await;

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

/// The branch the handoff must leave alone. A `queued` row never ran, so it
/// reloads silently: no `ThreadQueued` re-emit, and the original `queued_at`
/// stands so the backlog-age notification keeps counting from the real wait.
#[tokio::test]
async fn recover_loads_a_queued_row_silently_with_its_original_queued_at() {
    let f = fixture(5).await;
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    let entry_id = emit_cron_queued(&f.bus, "trig-a", false).await;
    let (queued_at, admitted_at) = row_stamps(&f.pool, entry_id).await;
    assert_eq!(admitted_at, None, "a queued row was never admitted");

    let (queue2, _executor2) = restarted_queue(&f, 5);
    queue2.recover_persisted_entries().await;

    assert_eq!(
        row_status(&f.pool, entry_id).await.as_deref(),
        Some("queued")
    );
    assert_eq!(
        row_stamps(&f.pool, entry_id).await.0,
        queued_at,
        "reloading a queued row must not restart its wait"
    );
    assert_eq!(
        entry_event_count(&f.pool, entry_id, "ThreadQueued").await,
        1,
        "a row that never ran is not re-queued, so it emits no second ThreadQueued"
    );

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

/// A cron entry binds no thread at submit time, so admission alone leaves the
/// row unbound. The fire reports the thread it mints, which is the half that
/// gives the boot handoff above something to test.
#[tokio::test]
async fn admitting_a_cron_entry_records_its_thread_id() {
    let f = fixture(5).await;
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    let a = f.queue.submit(cron_request("trig-a"), None, None).await;
    assert!(a.admitted);
    assert_eq!(
        row_thread_id(&f.pool, a.entry_id).await,
        None,
        "a trigger fire creates its thread at execution, not at admission"
    );
    let admitted_at = row_stamps(&f.pool, a.entry_id).await.1;
    assert!(admitted_at.is_some());

    let fired_thread = Uuid::new_v4();
    f.queue.record_entry_thread(a.entry_id, fired_thread).await;

    assert_eq!(
        row_thread_id(&f.pool, a.entry_id).await,
        Some(fired_thread),
        "the fire's thread lands on its queue row"
    );
    assert_eq!(
        row_status(&f.pool, a.entry_id).await.as_deref(),
        Some("admitted"),
        "recording the thread does not move the entry out of Running"
    );
    assert_eq!(
        row_stamps(&f.pool, a.entry_id).await.1,
        admitted_at,
        "the panel's Running age counts from the first admission"
    );

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn set_policy_persists_and_reloads() {
    let f = fixture(5).await;

    let mut policy = test_policy(9);
    policy.overflow = OverflowPolicy::PauseTrigger;
    f.queue
        .set_policy(policy.clone(), None)
        .await
        .expect("set_policy emits CapacityPolicyChanged");

    assert_eq!(f.queue.policy().await, policy);
    // A fresh boot reconstructs the policy from the latest event.
    assert_eq!(ThreadQueue::load_policy(&f.pool).await, policy);

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

/// Every lifecycle frame of a trigger fire's entry names its owning trigger, so
/// the matcher can keep that fire from waking the trigger.
///
/// None of the three can read the ambient scope. `ThreadQueued` goes out before
/// the fire starts, and `ThreadQueueCompleted` comes from the sibling task that
/// joins it. `ThreadQueueCompleted` is the one that matters most: it is
/// persisted, so a broad subscription really does wake on it.
#[tokio::test]
async fn a_trigger_fires_queue_frames_name_the_trigger_that_owns_them() {
    let f = fixture(10).await;
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    let mut rx = f.bus.subscribe();
    let a = f
        .queue
        .submit(event_trigger_request("trig-a"), None, None)
        .await;
    assert!(a.admitted);
    f.executor.release_one(); // execution finishes → the sibling task completes

    for (name, marker) in entry_frame_markers(&mut rx, a.entry_id).await {
        assert_eq!(marker.as_deref(), Some("trig-a"), "{name} lost its owner");
    }

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

/// The fail-open direction. A sub-thread a fire spawns is not the fire, so its
/// frames must carry no marker even though `submit` runs inside the fire's
/// scope. Inherit it and the trigger stops hearing about work it asked for,
/// which is the one failure that never recovers.
#[tokio::test]
async fn a_sub_thread_a_fire_submits_does_not_inherit_the_fires_marker() {
    let f = fixture(10).await;
    let mut rx = f.bus.subscribe();
    let queue = f.queue.clone();
    let sub = crate::scheduler::user_tasks::ACTIVE_TRIGGER_ID
        .scope("trig-a".to_string(), async move {
            queue
                .submit(sub_thread_request(Uuid::new_v4()), None, None)
                .await
        })
        .await;
    assert!(sub.admitted);
    f.executor.release_one();

    for (name, marker) in entry_frame_markers(&mut rx, sub.entry_id).await {
        assert_eq!(marker, None, "{name} inherited the parent fire's marker");
    }

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn pause_trigger_overflow_pauses_via_bus_event() {
    let f = fixture(10).await;
    let mut policy = test_policy(10);
    policy.overflow = OverflowPolicy::PauseTrigger;
    f.queue.set_policy(policy, None).await.expect("set_policy");
    f.trigger_configs
        .write()
        .unwrap()
        .insert("trig-a".to_string(), test_trigger_config("trig-a"));

    let mut rx = f.bus.subscribe();
    let _a = f
        .queue
        .submit(event_trigger_request("trig-a"), None, None)
        .await;
    let _b = f
        .queue
        .submit(event_trigger_request("trig-a"), None, None)
        .await;
    let _c = f
        .queue
        .submit(event_trigger_request("trig-a"), None, None)
        .await;
    // Fourth fire overflows (cap 2) → trigger paused through the bus, the
    // newcomer stays queued (nothing dropped).
    let d = f
        .queue
        .submit(event_trigger_request("trig-a"), None, None)
        .await;
    assert!(!d.admitted);
    assert_eq!(
        row_status(&f.pool, d.entry_id).await.as_deref(),
        Some("queued")
    );

    let disabled = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(emitted) = rx.recv().await {
                if let BusEvent::System(SystemEvent::TriggerDisabled { trigger_id, .. }) =
                    &emitted.typed
                {
                    return trigger_id.clone();
                }
            }
        }
    })
    .await
    .expect("TriggerDisabled must be emitted on pause-trigger overflow");
    assert_eq!(disabled, "trig-a");

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn user_slot_admits_under_capacity_and_counts_against_the_pool() {
    let f = fixture(2).await;
    let tid = Uuid::new_v4();
    let _release = spawn_user_slot(f.queue.clone(), tid);

    let q = f.queue.clone();
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await == Some("admitted") }
    })
    .await;

    // A user slot occupies the shared pool: a background spawn still fits the
    // 2nd slot, but a third occupant queues — the user is counted.
    let bg = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(bg.admitted, "one slot left after the user slot");
    let bg2 = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(
        !bg2.admitted,
        "user slot + 1 background fill the pool — background queues"
    );

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn user_slot_queues_at_true_pool_max_then_admits_on_release() {
    let f = fixture(1).await;
    // Fill the only slot with background work.
    let bg = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(bg.admitted);

    // A person typing now waits — user-initiated is prioritized, not exempt.
    let tid = Uuid::new_v4();
    let _release = spawn_user_slot(f.queue.clone(), tid);
    let q = f.queue.clone();
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await == Some("queued") }
    })
    .await;

    // Free the slot → the user admits.
    f.executor.release_one();
    bg.completion.await.ok();
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await == Some("admitted") }
    })
    .await;

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn reserved_background_floor_reclaims_ahead_of_waiting_user() {
    let f = fixture(2).await;
    let mut policy = test_policy(2);
    policy.reserved_background = 1; // background can always reclaim 1 slot
    f.queue.set_policy(policy, None).await.expect("set_policy");

    // Fill the pool (2) with user work.
    let u1 = Uuid::new_v4();
    let u2 = Uuid::new_v4();
    let r1 = spawn_user_slot(f.queue.clone(), u1);
    let _r2 = spawn_user_slot(f.queue.clone(), u2);
    let q = f.queue.clone();
    wait_until(|| {
        let q = q.clone();
        async move {
            user_status(&q, u1).await == Some("admitted")
                && user_status(&q, u2).await == Some("admitted")
        }
    })
    .await;

    // A background spawn and a third user both wait (pool full).
    let bg = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(!bg.admitted);
    let u3 = Uuid::new_v4();
    let _r3 = spawn_user_slot(f.queue.clone(), u3);
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, u3).await == Some("queued") }
    })
    .await;

    // Release one user slot. background_active (0) is below the floor (1) with
    // background queued → background RECLAIMS the freed slot ahead of the
    // waiting user.
    drop(r1);
    let pool = f.pool.clone();
    let bg_id = bg.entry_id;
    wait_until(|| {
        let pool = pool.clone();
        async move { row_status(&pool, bg_id).await.as_deref() == Some("admitted") }
    })
    .await;
    assert_eq!(
        user_status(&f.queue, u3).await,
        Some("queued"),
        "the user waiter yields the reclaim slot to background below the floor"
    );

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn user_waiter_preempts_background_above_the_floor() {
    let f = fixture(1).await;
    let mut policy = test_policy(1);
    policy.reserved_background = 0; // no floor → pure user priority
    f.queue.set_policy(policy, None).await.expect("set_policy");

    // Fill the slot with background, then queue a background AND a user.
    let bg1 = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(bg1.admitted);
    let bg2 = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(!bg2.admitted);
    let tid = Uuid::new_v4();
    let _release = spawn_user_slot(f.queue.clone(), tid);
    let q = f.queue.clone();
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await == Some("queued") }
    })
    .await;

    // Free the slot → no floor, so the user wins it ahead of queued background.
    f.executor.release_one();
    bg1.completion.await.ok();
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await == Some("admitted") }
    })
    .await;
    assert_eq!(
        row_status(&f.pool, bg2.entry_id).await.as_deref(),
        Some("queued"),
        "background yields the free slot to the prioritized user"
    );

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

// ---- Release-on-settle (the phantom "RUNNING" rows leak) ----

/// Minimal `CodingAgentIdled` for the settle-classifier test.
fn cc_idled() -> crate::engine::thread_events::ThreadEvent {
    crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
        has_changes: false,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
    }
}

/// Minimal `UserQuestionAsked` — the thread parks on the user
/// (`waiting_for_user_answer`), so reconcile removes its slot.
fn user_question_asked() -> crate::engine::thread_events::ThreadEvent {
    crate::engine::thread_events::ThreadEvent::UserQuestionAsked {
        tool_use_id: "tu-1".to_string(),
        cc_session_id: "sess-1".to_string(),
        question: "Pick one".to_string(),
        options: vec![],
        worktree_path: None,
        multi_select: false,
    }
}

/// Minimal `UserQuestionAnswered` — the user resolved the prompt, so the thread
/// goes back to `running` and reconcile must re-add its slot.
fn user_question_answered() -> crate::engine::thread_events::ThreadEvent {
    crate::engine::thread_events::ThreadEvent::UserQuestionAnswered {
        tool_use_id: "tu-1".to_string(),
        answer: crate::engine::thread_events::AnswerKind::FreeText {
            text: "go".to_string(),
        },
    }
}

/// Create the `thread_summaries` row (chat thread) so a later settle event's
/// projection resolves its section transition cleanly.
async fn emit_message_received(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: crate::engine::thread_events::ThreadEvent::MessageReceived {
            text: "hello".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: crate::engine::thread_events::ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: crate::engine::thread_events::EventMeta {
            channel: Some(crate::engine::thread_events::EventChannel::Chat),
            ..crate::engine::thread_events::EventMeta::NONE
        },
    })
    .await
    .expect("MessageReceived emit");
}

/// Acquire a user slot, then emit `settle_event` through the bus and assert the
/// settle subscriber reconciles the slot away (the thread left `running`) —
/// even though the chat task still holds its `UserSlotGuard` (the parked-CC
/// shape: the task never returns).
async fn assert_settle_releases(settle_event: crate::engine::thread_events::ThreadEvent) {
    let f = fixture(2).await;
    f.queue.spawn_settle_subscriber();
    let tid = Uuid::new_v4();
    emit_message_received(&f.bus, tid).await;

    let _release = spawn_user_slot(f.queue.clone(), tid);
    let q = f.queue.clone();
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await == Some("admitted") }
    })
    .await;

    f.bus
        .emit(BusEvent::Thread {
            thread_id: tid,
            event: settle_event,
            meta: crate::engine::thread_events::EventMeta::NONE,
        })
        .await
        .expect("settle event emit");

    // The subscriber releases asynchronously after the broadcast.
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await.is_none() }
    })
    .await;

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn settle_releases_user_slot_on_response_completion() {
    assert_settle_releases(
        crate::engine::thread_events::ThreadEvent::ResponseGenerated {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;
}

#[tokio::test]
async fn settle_releases_user_slot_on_response_failure() {
    assert_settle_releases(crate::engine::thread_events::ThreadEvent::ResponseFailed {
        error: "boom".to_string(),
    })
    .await;
}

#[tokio::test]
async fn settle_releases_user_slot_when_parked_on_user_question() {
    assert_settle_releases(user_question_asked()).await;
}

/// The reported bug: a user-initiated thread parks on a question (slot removed,
/// drops out of the Running set), the user answers, and the thread keeps running
/// — but the panel showed "Nothing running" because the slot was never restored.
/// reconcile must re-add the slot the moment the thread is `running` again.
#[tokio::test]
async fn reconcile_re_adds_user_slot_after_question_answered() {
    let f = fixture(2).await;
    f.queue.spawn_settle_subscriber();
    let tid = Uuid::new_v4();
    emit_message_received(&f.bus, tid).await;

    let _release = spawn_user_slot(f.queue.clone(), tid);
    let q = f.queue.clone();
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await == Some("admitted") }
    })
    .await;

    // Park: the thread asks the user a question → drops out of the Running set.
    f.bus
        .emit(BusEvent::Thread {
            thread_id: tid,
            event: user_question_asked(),
            meta: crate::engine::thread_events::EventMeta::NONE,
        })
        .await
        .expect("UserQuestionAsked emit");
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await.is_none() }
    })
    .await;

    // Resume: the user answers → status flips back to running → reconcile
    // re-adds the slot and the thread shows as running again.
    f.bus
        .emit(BusEvent::Thread {
            thread_id: tid,
            event: user_question_answered(),
            meta: crate::engine::thread_events::EventMeta::NONE,
        })
        .await
        .expect("UserQuestionAnswered emit");
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await == Some("admitted") }
    })
    .await;

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

/// The guard's backstop has to survive the park/resume round trip above.
/// `reconcile_user_slot` used to RE-FILE the resumed slot under a fresh `Uuid`,
/// so the gate's `UserSlotGuard` (which matches its own `entry_id`) could never
/// release it again. Pair that with a settle missed on broadcast lag and the
/// slot leaked for the life of the process; enough leaks and every chat POST
/// blocks on the pool. The resume now re-files under the parked key.
///
/// The fix is deliberately NOT a release-by-thread fallback: after a second
/// gate for the same thread that would drop the NEWER request's slot and admit
/// queued work above the pool limit.
#[tokio::test]
async fn a_dropped_guard_still_releases_a_slot_that_survived_a_park_resume() {
    let f = fixture(2).await;
    f.queue.spawn_settle_subscriber();
    let tid = Uuid::new_v4();
    emit_message_received(&f.bus, tid).await;

    let release = spawn_user_slot(f.queue.clone(), tid);
    let q = f.queue.clone();
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await == Some("admitted") }
    })
    .await;

    for event in [user_question_asked(), user_question_answered()] {
        let expect_admitted = matches!(
            event,
            crate::engine::thread_events::ThreadEvent::UserQuestionAnswered { .. }
        );
        f.bus
            .emit(BusEvent::Thread {
                thread_id: tid,
                event,
                meta: crate::engine::thread_events::EventMeta::NONE,
            })
            .await
            .expect("park/resume emit");
        wait_until(|| {
            let q = q.clone();
            async move { (user_status(&q, tid).await == Some("admitted")) == expect_admitted }
        })
        .await;
    }

    // The chat task ends. Its guard must still find the re-filed slot.
    drop(release);
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await.is_none() }
    })
    .await;

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn reconcile_frees_the_pool_when_thread_leaves_running_under_a_held_guard() {
    let f = fixture(1).await;
    let tid = Uuid::new_v4();
    emit_message_received(&f.bus, tid).await; // status='running', initiator='user'
                                              // The guard stays held by the detached task for the whole test, exactly
                                              // the parked-CC shape where the chat task can't release on its own.
    let _release = spawn_user_slot(f.queue.clone(), tid);
    let q = f.queue.clone();
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await == Some("admitted") }
    })
    .await;

    // Pool of 1 is saturated by the user slot → a background spawn queues.
    let bg = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(!bg.admitted, "pool of 1 is saturated by the user slot");

    // The thread leaves `running` (response completed). reconcile clears the
    // slot even though the guard is still held, and the freed slot drains the
    // queued background spawn.
    f.bus
        .emit(BusEvent::Thread {
            thread_id: tid,
            event: crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: crate::engine::thread_events::EventMeta::NONE,
        })
        .await
        .expect("ResponseGenerated emit");
    f.queue.reconcile_user_slot(tid).await;
    assert_eq!(
        user_status(&f.queue, tid).await,
        None,
        "reconcile clears the slot under a held guard once the thread leaves running"
    );

    // The freed slot drained the queued background spawn.
    let pool = f.pool.clone();
    let bg_id = bg.entry_id;
    wait_until(|| {
        let pool = pool.clone();
        async move { row_status(&pool, bg_id).await.as_deref() == Some("admitted") }
    })
    .await;

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

/// reconcile adds a slot for a running user thread, is idempotent, and removes
/// it when the thread is no longer running — driving the pool purely from
/// `thread_summaries.status`, with no settle subscriber in the loop.
#[tokio::test]
async fn reconcile_converges_pool_to_thread_status() {
    let f = fixture(2).await;
    let tid = Uuid::new_v4();

    // No row yet → not running → reconcile leaves the pool empty.
    f.queue.reconcile_user_slot(tid).await;
    assert_eq!(user_status(&f.queue, tid).await, None);

    // A running user thread → reconcile adds exactly one slot.
    emit_message_received(&f.bus, tid).await;
    f.queue.reconcile_user_slot(tid).await;
    assert_eq!(user_status(&f.queue, tid).await, Some("admitted"));

    // Idempotent: reconciling again while still running does not double-add.
    f.queue.reconcile_user_slot(tid).await;
    let count = f
        .queue
        .user_entries()
        .await
        .into_iter()
        .filter(|e| e.thread_id == Some(tid))
        .count();
    assert_eq!(
        count, 1,
        "reconcile must not double-add a running user thread"
    );

    // Thread leaves running → reconcile removes the slot.
    f.bus
        .emit(BusEvent::Thread {
            thread_id: tid,
            event: crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: crate::engine::thread_events::EventMeta::NONE,
        })
        .await
        .expect("ResponseGenerated emit");
    f.queue.reconcile_user_slot(tid).await;
    assert_eq!(user_status(&f.queue, tid).await, None);

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

/// reconcile never adds a slot for a background (non-user) running thread — only
/// `initiator = 'user'` threads occupy the user-half of the pool.
#[tokio::test]
async fn reconcile_ignores_background_thread() {
    let f = fixture(2).await;
    let tid = Uuid::new_v4();
    // An agent-driven MessageReceived → initiator='system', status='running'.
    f.bus
        .emit(BusEvent::Thread {
            thread_id: tid,
            event: crate::engine::thread_events::ThreadEvent::MessageReceived {
                text: "spawned".into(),
                user_image_hashes: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: crate::engine::thread_events::ActorMode::Agent,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: crate::engine::thread_events::EventMeta {
                channel: Some(crate::engine::thread_events::EventChannel::Chat),
                ..crate::engine::thread_events::EventMeta::NONE
            },
        })
        .await
        .expect("agent MessageReceived emit");

    f.queue.reconcile_user_slot(tid).await;
    assert_eq!(
        user_status(&f.queue, tid).await,
        None,
        "a background (initiator='system') thread must not occupy a user slot"
    );

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[tokio::test]
async fn snapshot_merges_background_rows_and_user_entries() {
    let f = fixture(5).await;
    // Background sub-thread spawn → a persisted `thread_queue` row.
    let bg = f
        .queue
        .submit(sub_thread_request(Uuid::new_v4()), None, None)
        .await;
    assert!(bg.admitted);
    // User-initiated slot → in-memory only, no row.
    let tid = Uuid::new_v4();
    let _release = spawn_user_slot(f.queue.clone(), tid);
    let q = f.queue.clone();
    wait_until(|| {
        let q = q.clone();
        async move { user_status(&q, tid).await == Some("admitted") }
    })
    .await;

    // The single merged view both the panel API and the LLM tool read.
    let snap = f.queue.snapshot().await.expect("snapshot");
    let bg_entry = snap
        .entries
        .iter()
        .find(|e| e.id == bg.entry_id)
        .expect("background entry present in snapshot");
    assert_eq!(bg_entry.kind, "sub-thread");
    assert_eq!(bg_entry.status, "admitted");
    let user_entry = snap
        .entries
        .iter()
        .find(|e| e.thread_id == Some(tid))
        .expect("user-chat entry merged into snapshot");
    assert_eq!(user_entry.kind, "user-chat");
    assert_eq!(user_entry.status, "admitted");

    f.pool.close().await;
    teardown_test_db(&f.db).await;
}

#[test]
fn affects_user_running_selects_status_transitions() {
    use crate::engine::thread_events::ThreadEvent;
    use crate::runtime::CodingAgent;

    // Status transitions reconcile must observe: parks, resumes (no gate),
    // continuations, and terminals.
    assert!(affects_user_running(&user_question_asked())); // park
    assert!(affects_user_running(&user_question_answered())); // resume (no gate)
    assert!(affects_user_running(&cc_idled())); // terminal
    assert!(affects_user_running(&ThreadEvent::ResponseGenerated {
        text: String::new(),
        images: vec![],
        model: None,
        reasoning_effort: None,
    }));
    assert!(affects_user_running(&ThreadEvent::ResponseFailed {
        error: "x".into()
    }));
    assert!(affects_user_running(&ThreadEvent::ContinuationRequested {
        reason: String::new()
    }));

    // Gate-covered starts: the gate owns the add, so reconcile must NOT fire on
    // these (a reconcile add would race the gate's unconditional add).
    assert!(!affects_user_running(&ThreadEvent::MessageReceived {
        text: "hi".into(),
        user_image_hashes: vec![],
        device_id: None,
        device: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: crate::engine::thread_events::ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    }));

    // Per-token streaming / neutral: never move status.
    assert!(!affects_user_running(&ThreadEvent::TextStreamed {
        text: "hi".into()
    }));
    assert!(!affects_user_running(&ThreadEvent::CodingAgentToolCalled {
        name: "Bash".into(),
        args: serde_json::json!({}),
        description: String::new(),
        coding_agent: CodingAgent::ClaudeCode,
        tool_use_id: String::new(),
    }));
    assert!(!affects_user_running(&ThreadEvent::ThreadSaved));
}

// ---- Event-trigger chain depth ----

/// THE boundary the bug lived at, measured with a probe.
///
/// A trigger fire hands its `run_thread` spawn to the queue, and the queue
/// hands the execution to `tokio::spawn`. `EVENT_TRIGGER_DEPTH` follows an
/// await chain and not a spawn, so `prepare` read the fire's depth and
/// `execute` read 0. With execution at 0 the chain restarts every hop, and
/// the depth cap can never end a loop that passes through spawned work.
#[tokio::test]
async fn a_fires_spawned_work_runs_at_the_fires_own_depth() {
    use crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH;

    const FIRE_DEPTH: u32 = 2;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let configs = Arc::new(std::sync::RwLock::new(HashMap::new()));
    let workspace = tempfile::tempdir().expect("temp workspace");
    let (probe, mut entered) = DepthProbeExecutor::new();
    let queue = queue_with_executor(&pool, &bus, &configs, workspace.path(), 4, probe.clone());

    // Submitted from inside the fire, exactly as the `run_thread` tool does.
    let outcome = EVENT_TRIGGER_DEPTH
        .scope(
            FIRE_DEPTH,
            queue.submit(sub_thread_request(Uuid::new_v4()), None, None),
        )
        .await;
    assert!(outcome.admitted);
    let executed_depth = entered.recv().await.expect("execute runs");

    assert_eq!(
        probe.prepared_depths(),
        vec![FIRE_DEPTH],
        "the admission hook emits, so it has to run at the fire's depth"
    );
    assert_eq!(
        executed_depth, FIRE_DEPTH,
        "a spawn does not consume a hop: spawned work runs AT the fire's depth"
    );
    assert_eq!(probe.executed_depths(), vec![FIRE_DEPTH]);

    probe.release_one();
    let _ = pool;
    teardown_test_db(&db).await;
}

/// The same request, admitted by the drainer instead of by `submit`.
///
/// The drainer runs on the completing entry's task, whose own chain depth is 0.
/// A depth read from the ambient task there would be 0 too. One queued fire
/// would then behave differently from an identical one that had a free slot.
/// The depth travels on the request precisely so both answer the same.
#[tokio::test]
async fn a_queued_spawn_keeps_its_depth_when_the_drainer_admits_it() {
    use crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH;

    const FIRE_DEPTH: u32 = 1;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let configs = Arc::new(std::sync::RwLock::new(HashMap::new()));
    let workspace = tempfile::tempdir().expect("temp workspace");
    let (probe, mut entered) = DepthProbeExecutor::new();
    // One slot, so the second submit has to wait for the drainer.
    let queue = queue_with_executor(&pool, &bus, &configs, workspace.path(), 1, probe.clone());

    let first = EVENT_TRIGGER_DEPTH
        .scope(
            FIRE_DEPTH,
            queue.submit(sub_thread_request(Uuid::new_v4()), None, None),
        )
        .await;
    assert!(first.admitted);
    assert_eq!(entered.recv().await, Some(FIRE_DEPTH));

    let second = EVENT_TRIGGER_DEPTH
        .scope(
            FIRE_DEPTH,
            queue.submit(sub_thread_request(Uuid::new_v4()), None, None),
        )
        .await;
    assert!(!second.admitted, "the pool is full, so this one queues");

    // Free the slot. The drain that follows runs on the completing task.
    probe.release_one();
    let drained_depth = entered.recv().await.expect("the queued entry drains");

    assert_eq!(
        drained_depth, FIRE_DEPTH,
        "a drained entry runs at the depth it was submitted with, not the drainer's"
    );
    assert_eq!(
        probe.prepared_depths(),
        vec![FIRE_DEPTH, FIRE_DEPTH],
        "both admissions emit at the fire's depth"
    );

    probe.release_one();
    let _ = pool;
    teardown_test_db(&db).await;
}

/// A coding-agent session emits from a spawn tree the executor's scope cannot
/// reach, so its thread carries the depth instead.
///
/// The binding lasts as long as the WORK, not as long as the thread: once the
/// entry completes, a later user Continue on the same thread starts a fresh
/// chain at 0.
#[tokio::test]
async fn an_admitted_spawns_thread_carries_the_chain_depth_until_it_completes() {
    use crate::scheduler::user_tasks::{chain_depth_for_thread, EVENT_TRIGGER_DEPTH};

    const FIRE_DEPTH: u32 = 2;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let configs = Arc::new(std::sync::RwLock::new(HashMap::new()));
    let workspace = tempfile::tempdir().expect("temp workspace");
    let (probe, mut entered) = DepthProbeExecutor::new();
    let queue = queue_with_executor(&pool, &bus, &configs, workspace.path(), 4, probe.clone());

    let child = Uuid::new_v4();
    let outcome = EVENT_TRIGGER_DEPTH
        .scope(
            FIRE_DEPTH,
            queue.submit(sub_thread_request(child), None, None),
        )
        .await;
    assert!(outcome.admitted);
    entered.recv().await.expect("execute runs");

    assert_eq!(
        chain_depth_for_thread(child),
        Some(FIRE_DEPTH),
        "every event on this thread belongs to the fire's chain, whichever task emits it"
    );

    queue.complete(outcome.entry_id).await;
    assert_eq!(
        chain_depth_for_thread(child),
        None,
        "the work is over, so a later user Continue starts a fresh chain"
    );

    probe.release_one();
    let _ = pool;
    teardown_test_db(&db).await;
}

/// The depth survives a restart on the persisted request, and the re-queued
/// entry re-fires at it.
///
/// This is the half that has to survive. The registry does not: it is rebuilt
/// when the entry is admitted, so a re-fire after a restart carries the same
/// chain the dead process was on.
#[tokio::test]
async fn a_re_queued_entry_re_fires_at_the_depth_it_was_submitted_with() {
    use crate::scheduler::user_tasks::{
        chain_depth_for_thread, forget_chain_depth, EVENT_TRIGGER_DEPTH,
    };

    const FIRE_DEPTH: u32 = 2;
    let f = fixture(4).await;
    let child = Uuid::new_v4();
    let outcome = EVENT_TRIGGER_DEPTH
        .scope(
            FIRE_DEPTH,
            f.queue.submit(sub_thread_request(child), None, None),
        )
        .await;
    assert!(outcome.admitted);

    // The previous process died mid-flight: its in-memory registry went with
    // it, and its thread never materialized, so the row is owed a re-fire.
    forget_chain_depth(child, outcome.entry_id);
    assert_eq!(chain_depth_for_thread(child), None);

    let (probe, mut entered) = DepthProbeExecutor::new();
    let restarted = queue_with_executor(
        &f.pool,
        &f.bus,
        &f.trigger_configs,
        f.workspace.path(),
        4,
        probe.clone(),
    );
    restarted.recover_persisted_entries().await;
    restarted.drain().await;
    let refired_depth = entered.recv().await.expect("the re-queued entry re-fires");

    assert_eq!(
        refired_depth, FIRE_DEPTH,
        "the re-fire reads its depth off the persisted request"
    );
    assert_eq!(
        chain_depth_for_thread(child),
        Some(FIRE_DEPTH),
        "and re-registers the thread, so emits from its own tasks stay on the chain"
    );

    restarted.complete(outcome.entry_id).await;
    f.executor.release_one();
    probe.release_one();
    teardown_test_db(&f.db).await;
}

/// A row handed to thread recovery leaves NO chain-depth binding behind.
///
/// The handoff completes the entry without an in-memory slot, so nothing would
/// ever clear one. It would outlive the work, and a later user Continue on that
/// thread would inherit a chain it has nothing to do with.
#[tokio::test]
async fn a_handed_off_row_leaves_no_chain_depth_behind() {
    use crate::scheduler::user_tasks::{
        chain_depth_for_thread, forget_chain_depth, EVENT_TRIGGER_DEPTH,
    };

    let f = fixture(4).await;
    let child = Uuid::new_v4();
    let outcome = EVENT_TRIGGER_DEPTH
        .scope(2, f.queue.submit(sub_thread_request(child), None, None))
        .await;
    assert!(outcome.admitted);
    // A live thread is what makes the boot sweep hand off instead of re-queue.
    materialize_trigger_thread(&f.bus, child, "t1").await;
    forget_chain_depth(child, outcome.entry_id);

    let (restarted, _executor) = restarted_queue(&f, 4);
    restarted.recover_persisted_entries().await;

    assert_eq!(
        chain_depth_for_thread(child),
        None,
        "the work is thread recovery's now, and a binding here would never expire"
    );

    f.executor.release_one();
    teardown_test_db(&f.db).await;
}

/// Two entries can name one thread, and the first to finish must not take the
/// other's binding with it.
///
/// A plain `thread -> depth` map let it. The surviving entry's work then fell
/// back to 0 and escaped the cap, silently, which is the failure the whole
/// carrier exists to prevent.
#[test]
fn one_entry_completing_leaves_a_sibling_entrys_binding_alone() {
    use crate::scheduler::user_tasks::{
        chain_depth_for_thread, forget_chain_depth, register_chain_depth,
    };

    let thread = Uuid::new_v4();
    let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
    register_chain_depth(thread, first, 1);
    register_chain_depth(thread, second, 3);

    assert_eq!(
        chain_depth_for_thread(thread),
        Some(3),
        "the deepest live binding wins, matching how EventBus ranks the carriers"
    );

    forget_chain_depth(thread, second);
    assert_eq!(
        chain_depth_for_thread(thread),
        Some(1),
        "the sibling's work is still on its own chain"
    );

    forget_chain_depth(thread, first);
    assert_eq!(chain_depth_for_thread(thread), None);
}

/// Re-registering one owner replaces its row rather than stacking another.
#[test]
fn re_registering_the_same_entry_does_not_stack_a_second_binding() {
    use crate::scheduler::user_tasks::{
        chain_depth_for_thread, forget_chain_depth, register_chain_depth,
    };

    let thread = Uuid::new_v4();
    let entry = Uuid::new_v4();
    register_chain_depth(thread, entry, 2);
    register_chain_depth(thread, entry, 2);
    forget_chain_depth(thread, entry);

    assert_eq!(
        chain_depth_for_thread(thread),
        None,
        "one forget clears what one owner registered"
    );
}

/// The frame that closes a fire's entry carries that fire's depth.
///
/// It goes out on the sibling task that joins the work, where the ambient scope
/// reads 0. `ThreadQueueCompleted` is persisted and therefore subscribable, so
/// at depth 0 two triggers subscribed to it would wake each other forever. The
/// self-wake gate cannot help: each wakes the OTHER.
#[tokio::test]
async fn the_frame_closing_a_fire_carries_the_fires_depth() {
    use crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH;

    const FIRE_DEPTH: u32 = 2;
    let f = fixture(4).await;
    let mut rx = f.bus.subscribe();
    let outcome = EVENT_TRIGGER_DEPTH
        .scope(
            FIRE_DEPTH,
            f.queue
                .submit(sub_thread_request(Uuid::new_v4()), None, None),
        )
        .await;
    assert!(outcome.admitted);
    f.queue.complete(outcome.entry_id).await;

    let mut completed_depth = None;
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::System(SystemEvent::ThreadQueueCompleted { entry_id }) = &emitted.typed {
            if *entry_id == outcome.entry_id {
                completed_depth = Some(emitted.depth);
            }
        }
    }
    assert_eq!(
        completed_depth,
        Some(FIRE_DEPTH),
        "the closing frame belongs to the fire's chain, not to the joining task"
    );

    f.executor.release_one();
    teardown_test_db(&f.db).await;
}
