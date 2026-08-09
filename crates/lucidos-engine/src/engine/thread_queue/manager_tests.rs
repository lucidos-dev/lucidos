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
    let queue = Arc::new(ThreadQueue::new(
        pool.clone(),
        bus.clone(),
        trigger_configs.clone(),
        workspace.path().to_path_buf(),
        Arc::new(tokio::sync::Mutex::new(())),
        test_policy(max_total),
    ));
    let executor = GatedExecutor::new();
    queue.set_executor(executor.clone());
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

    // A fresh manager recovers over the same DB.
    let queue2 = Arc::new(ThreadQueue::new(
        f.pool.clone(),
        f.bus.clone(),
        f.trigger_configs.clone(),
        f.workspace.path().to_path_buf(),
        Arc::new(tokio::sync::Mutex::new(())),
        test_policy(5),
    ));
    let executor2 = GatedExecutor::new();
    queue2.set_executor(executor2.clone());
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

    // "Restart": a fresh manager over the same DB (the old process's
    // in-flight execution died with it).
    let queue2 = Arc::new(ThreadQueue::new(
        f.pool.clone(),
        f.bus.clone(),
        f.trigger_configs.clone(),
        f.workspace.path().to_path_buf(),
        Arc::new(tokio::sync::Mutex::new(())),
        test_policy(5),
    ));
    let executor2 = GatedExecutor::new();
    queue2.set_executor(executor2.clone());
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

    // Materialize the thread (what the dead process's execution did before
    // crashing) — MessageReceived creates the thread_summaries row.
    f.bus
        .emit(BusEvent::Thread {
            thread_id: child,
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
        .expect("MessageReceived emit");

    let queue2 = Arc::new(ThreadQueue::new(
        f.pool.clone(),
        f.bus.clone(),
        f.trigger_configs.clone(),
        f.workspace.path().to_path_buf(),
        Arc::new(tokio::sync::Mutex::new(())),
        test_policy(5),
    ));
    queue2.set_executor(GatedExecutor::new());
    queue2.recover_persisted_entries().await;

    // The spawn already happened — the entry completes (row deleted) and
    // ownership passes to thread-level recovery (chat settle / CC resume).
    assert_eq!(row_status(&f.pool, a.entry_id).await, None);

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
