//! The *Thread Queue* — system-wide admission control for ALL thread work.
//!
//! One shared capacity pool gates every path that creates running work:
//! event-trigger fires, cron fires, agent-driven sub-thread spawns
//! (`run_thread`), coding-agent spawns (`run_coding_agent`), agent-mode chat POSTs
//! (cross-workspace tasks / `lucidos spawn-thread`), AND user-initiated chat /
//! user-typed coding-agent threads.
//!
//! User-initiated work is **prioritized, not exempt** (ADR 0008, superseding
//! ADR 0007's "user preempts and doesn't count"): it counts against
//! `max_concurrent_total`, it drains ahead of background, it ignores the
//! per-kind / per-trigger caps, and it queues when the pool is genuinely
//! full — a person can briefly wait at true pool-max. `reserved_background`
//! is a floor background can reclaim ahead of user work so priority can't
//! starve triggers/cron.
//!
//! Two flavours of occupant share the pool:
//! - **Background spawns** go through [`ThreadQueue::submit`]: the queue owns
//!   their execution (via the executor) and persists them in the
//!   `thread_queue` projection (event-sourced from `ThreadQueued` /
//!   `ThreadQueueAdmitted` / `ThreadQueueDropped` / `ThreadQueueCompleted`),
//!   so a restart re-queues work that never ran and drains it as capacity
//!   frees.
//! - **User-initiated work** goes through [`ThreadQueue::acquire_user_slot`]:
//!   the caller (chat handler) runs it itself; the queue only gates the START
//!   (back-pressure at pool-max). User occupancy is read at each admission
//!   (ADR 0443): running user threads in `thread_summaries`, plus gate
//!   reservations whose thread has not started running yet. A thread counts
//!   exactly while it is `running`, however it got there. No subscriber has to
//!   deliver an event for that to hold. Reservations live in memory only (a
//!   dead response is gone on restart, never re-fired); the panel API merges
//!   them in.
//!
//! Over capacity, work is enqueued (user waiters first, then FIFO per trigger,
//! best-effort across triggers) instead of running unbounded.
//!
//! Per the broadcast/subscribe rule, all state changes flow through
//! [`EventBus`]; the projection in `event_bus_projection_system.rs` keeps
//! the table in lockstep, and SSE consumers (the Thread Queue panel) react
//! to the same events.

mod policy;
mod request;

pub mod executor;

pub use executor::{ExecutableEntry, ThreadQueueExecutor};
pub use policy::{
    AdmissionCounts, AdmissionDecision, CapacityPolicy, OverflowPolicy, ThreadQueueKind,
    DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD, DEFAULT_MAX_EVENT_TRIGGER_DEPTH,
};
pub(crate) use request::truncate_summary;
pub use request::ThreadQueueRequest;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{MessageOrigin, ThreadEvent};
use crate::engine::thread_lifecycle::ThreadStatus;
use crate::engine::LucidosEngine;
use crate::triggers::TriggerConfig;

/// How long a trigger's oldest queued fire waits before the "significantly
/// delayed" notification fires. Wait time, never queue depth: a burst of quick
/// fires drains in seconds. One agentic fire often runs a few minutes, so a
/// shorter wait would alert whenever two fires of one trigger overlap.
const DELAY_ALERT_WAIT: Duration = Duration::from_secs(5 * 60);
/// Minimum spacing between notifications for the same trigger (and for the
/// global at-capacity notice) so a hot queue doesn't spam the inbox.
const NOTIFY_COOLDOWN: Duration = Duration::from_secs(10 * 60);
/// Safety-net drain interval — normal drains are event-driven (completion,
/// drop, policy change, trigger resume); the timer catches anything missed
/// and drives the delay notification check.
const DRAIN_INTERVAL: Duration = Duration::from_secs(60);

/// One queued spawn, in memory. Mirrors a `thread_queue` row with
/// status `'queued'` plus the non-persistable runtime handles.
struct QueueEntry {
    id: Uuid,
    kind: ThreadQueueKind,
    trigger_id: Option<String>,
    trigger_name: Option<String>,
    thread_id: Option<Uuid>,
    summary: String,
    request: ThreadQueueRequest,
    queued_at: DateTime<Utc>,
    /// Cooperative cancel from the submitter (cron task loop). Lost on
    /// restart — a re-queued entry runs uncancellable, same as a
    /// missed-grace cron catch-up. Allowed-ephemeral.
    cancel: Option<CancellationToken>,
    /// Resolved on admit-completion or drop so a waiting submitter (the cron
    /// task loop) unblocks. Allowed-ephemeral — after restart nobody waits.
    completion_tx: Option<oneshot::Sender<()>>,
}

/// One admitted (actively executing) background spawn's accounting record.
struct ActiveSlot {
    kind: ThreadQueueKind,
    trigger_id: Option<String>,
    completion_tx: Option<oneshot::Sender<()>>,
    /// The thread this entry's chain depth is registered under, so `complete`
    /// can drop the registration. `None` until the work has a thread: a trigger
    /// fire creates its own, and binds it later through `record_entry_thread`.
    chain_thread_id: Option<Uuid>,
    /// The entry's chain depth, kept so a late thread binding can register at
    /// it without re-reading the request.
    chain_depth: u32,
}

/// A gate admission, held until its guard drops. It counts toward the pool
/// until it is seen: its thread is `running`, or has started a turn since
/// admission. From then on the thread's own status counts instead.
/// In-memory only (a dead response is gone on restart).
struct UserReservation {
    thread_id: Uuid,
    summary: String,
    admitted_at: DateTime<Utc>,
    /// The thread's highest event sequence at admission. A turn start with a
    /// higher one is this reservation's turn, so the reservation is seen.
    after_seq: i64,
    seen: bool,
}

/// A thread that occupies a user slot because it is `running`: one the user
/// started, or one a live gate admission is for. Read at admission.
struct RunningThread {
    summary: String,
    revived_at: DateTime<Utc>,
}

/// A user-initiated response waiting for a free slot (pool at max). Drains
/// with priority — ahead of background, behind the reserved-background floor.
struct UserWaiter {
    /// Stable id for this waiter, and for its reservation once admitted.
    entry_id: Uuid,
    thread_id: Uuid,
    summary: String,
    queued_at: DateTime<Utc>,
    /// Resolved by the drainer once admitted, unblocking the chat task.
    wake: oneshot::Sender<()>,
}

struct QueueState {
    policy: CapacityPolicy,
    queued: VecDeque<QueueEntry>,
    active: HashMap<Uuid, ActiveSlot>,
    /// Live gate admissions, keyed by the gate's entry id.
    user_reservations: HashMap<Uuid, UserReservation>,
    /// User-initiated responses waiting for a slot (priority line, FIFO).
    user_queued: VecDeque<UserWaiter>,
    /// Per-trigger notification cooldowns (allowed-ephemeral — a restart
    /// resetting the cooldown at worst re-notifies once).
    delay_notified: HashMap<String, Instant>,
    global_notified: Option<Instant>,
}

impl QueueState {
    /// User occupancy: every occupying running thread, plus each reservation
    /// not yet seen. `running` comes from [`ThreadQueue::read_user_occupancy`].
    fn user_active(&self, running: &HashMap<Uuid, RunningThread>) -> usize {
        running.len() + self.unseen_reservations(None)
    }

    /// Unseen reservations, leaving out `except`'s own.
    fn unseen_reservations(&self, except: Option<Uuid>) -> usize {
        self.user_reservations
            .values()
            .filter(|r| !r.seen && Some(r.thread_id) != except)
            .count()
    }

    /// User occupancy without `thread_id`'s own share, for admitting a request
    /// on that thread. A follow-up whose message already set the thread
    /// `running` must not wait for a slot it holds itself.
    fn user_active_besides(
        &self,
        running: &HashMap<Uuid, RunningThread>,
        thread_id: Uuid,
    ) -> usize {
        running.len() - usize::from(running.contains_key(&thread_id))
            + self.unseen_reservations(Some(thread_id))
    }

    /// The "Lucidos is at capacity" notification, when the pool is full and
    /// its cooldown has elapsed. Call under the state lock, send after.
    fn capacity_notification_due(&mut self, user_active: usize) -> Option<(String, String)> {
        if self.active.len() + user_active < self.policy.max_concurrent_total
            || self
                .global_notified
                .is_some_and(|t| t.elapsed() < NOTIFY_COOLDOWN)
        {
            return None;
        }
        self.global_notified = Some(Instant::now());
        let total_queued = self.queued.len() + self.user_queued.len();
        Some((
            "Lucidos is at capacity".to_string(),
            format!(
                "All {} slots are busy; {total_queued} thread(s) are waiting \
                 in the Thread Queue.",
                self.policy.max_concurrent_total
            ),
        ))
    }

    /// Whether the trigger's delay cooldown has elapsed. Starts a new
    /// cooldown when it has, so the caller must then notify.
    fn delay_cooldown_elapsed(&mut self, trigger_id: &str) -> bool {
        let elapsed = self
            .delay_notified
            .get(trigger_id)
            .is_none_or(|t| t.elapsed() >= NOTIFY_COOLDOWN);
        if elapsed {
            self.delay_notified
                .insert(trigger_id.to_string(), Instant::now());
        }
        elapsed
    }

    fn counts_for(
        &self,
        kind: ThreadQueueKind,
        trigger_id: Option<&str>,
        user_active: usize,
    ) -> AdmissionCounts {
        let kind_active = self.active.values().filter(|s| s.kind == kind).count();
        let (trigger_active, trigger_queued) = match trigger_id {
            Some(tid) => (
                self.active
                    .values()
                    .filter(|s| s.trigger_id.as_deref() == Some(tid))
                    .count(),
                self.queued
                    .iter()
                    .filter(|e| e.trigger_id.as_deref() == Some(tid))
                    .count(),
            ),
            None => (0, 0),
        };
        AdmissionCounts {
            background_active: self.active.len(),
            user_active,
            kind_active,
            trigger_active,
            trigger_queued,
            user_queued: self.user_queued.len(),
        }
    }
}

/// Outcome of [`ThreadQueue::submit`].
pub struct SubmitOutcome {
    pub entry_id: Uuid,
    /// `true` = running now; `false` = waiting in the queue.
    pub admitted: bool,
    /// 1-based queue position when `admitted == false`; 0 when admitted.
    pub position: usize,
    /// `true` when admission COALESCED the entry away: a fire of this cron
    /// trigger was already active or queued, so nothing new was started and
    /// `completion` is already resolved.
    ///
    /// The scheduler's two submit sites ignore this (a dropped redundant fire
    /// is exactly what they want). It exists for the off-schedule-run path,
    /// where `admitted: false, position: 0` is otherwise indistinguishable from
    /// a queued entry and a caller would report "started" for a run that never
    /// happened. See `engine_impl::trigger_runs`.
    pub coalesced: bool,
    /// Resolves when the entry's work finishes OR the entry is dropped.
    pub completion: oneshot::Receiver<()>,
}

/// RAII handle for a gate admission from [`ThreadQueue::acquire_user_slot`].
/// Dropping it (even on panic) removes the gate's reservation or waiter, so a
/// crashed chat task can't leak capacity.
pub struct UserSlotGuard {
    queue: Arc<ThreadQueue>,
    entry_id: Uuid,
}

impl Drop for UserSlotGuard {
    fn drop(&mut self) {
        // release_user_slot is async; the drop runs in a sync context, so hand
        // the cleanup to a detached task. Reservations are in-memory, so a
        // missed release at shutdown is harmless (it dies with the process).
        let queue = self.queue.clone();
        let entry_id = self.entry_id;
        tokio::spawn(async move {
            queue.release_user_slot(entry_id).await;
        });
    }
}

/// One in-memory user-initiated pool occupant, for the panel API to merge
/// with the persisted background rows. Mirrors the displayed fields of a
/// `thread_queue` row (`kind` is always `user-chat` for these).
pub struct UserQueueEntry {
    pub id: Uuid,
    pub thread_id: Uuid,
    pub summary: String,
    /// `"admitted"` (Running) or `"queued"` (waiting for a slot).
    pub status: &'static str,
    pub queued_at: DateTime<Utc>,
    pub admitted_at: Option<DateTime<Utc>>,
}

/// One occupant of the shared pool, wire-shaped for the Thread Queue panel
/// (`GET /api/v1/thread-queue`) AND the `list_thread_queue` LLM tool.
/// Background entries (`event-trigger` / `cron` / `sub-thread` /
/// `coding-agent`) are read from the `thread_queue` projection; user-initiated
/// entries (`user-chat`) are merged in from the manager's in-memory state.
/// Both read paths go through [`ThreadQueue::snapshot`], so the panel and the
/// tool materialize the SAME view and can never diverge.
#[derive(serde::Serialize, sqlx::FromRow)]
pub struct ThreadQueueEntryView {
    pub id: Uuid,
    /// `event-trigger` | `cron` | `sub-thread` | `coding-agent` | `user-chat`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Uuid>,
    pub summary: String,
    /// `queued` | `admitted` (admitted = actively running).
    pub status: String,
    pub queued_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admitted_at: Option<DateTime<Utc>>,
}

/// The full Thread Queue view: every pool occupant plus the active capacity
/// policy. The single response shape shared by the panel API and the LLM tool.
#[derive(serde::Serialize)]
pub struct ThreadQueueSnapshot {
    pub entries: Vec<ThreadQueueEntryView>,
    pub policy: CapacityPolicy,
}

/// Event types that move a thread to `running`. A gate reservation is seen,
/// and stops counting, once its thread has one of these after the reservation.
/// The drain hint fires on them too, so the panel shows a thread that starts.
const TURN_START_EVENT_TYPES: &[&str] = &[
    "MessageReceived",
    "SessionStarted",
    "CodingAgentUserMessageSent",
    "PromptInjected",
    "ContinuationStarted",
    "ContinuationRequested",
    "UserQuestionAnswered",
    "CodingAgentPermissionResolved",
    "CommandPermissionResolved",
    "McpPermissionResolved",
    "CodingAgentPromptSent",
];

/// Thread events after which user occupancy may have moved: a turn starts, a
/// thread parks on the user, or a turn ends. The drain hint subscriber drains
/// on them. A missed one only delays a drain: admission reads occupancy fresh.
fn moves_user_occupancy(event: &ThreadEvent) -> bool {
    TURN_START_EVENT_TYPES.contains(&event.event_type())
        || matches!(
            event,
            ThreadEvent::CodingAgentInputRead {
                started_turn: true,
                ..
            }
            // → waiting_for_user_answer (parked on the user).
            | ThreadEvent::UserQuestionAsked { .. }
            | ThreadEvent::CodingAgentPermissionRequest { .. }
            | ThreadEvent::CommandPermissionRequested { .. }
            | ThreadEvent::McpPermissionRequested { .. }
            // → idle / waiting / failed (terminal or back-to-idle).
            | ThreadEvent::ResponseGenerated { .. }
            | ThreadEvent::ResponseCanceled { .. }
            | ThreadEvent::ResponseAborted { .. }
            | ThreadEvent::ResponseFailed { .. }
            | ThreadEvent::CodingAgentIdled { .. }
            | ThreadEvent::TriggerCompleted { .. }
            | ThreadEvent::SessionEnded { .. }
            | ThreadEvent::ChangeApplied { .. }
            | ThreadEvent::ChangeDiscarded { .. }
            | ThreadEvent::ThreadArchived
        )
}

/// Central admission-control manager. Lives on [`LucidosEngine`] as
/// `engine.thread_queue`; all four background spawn paths submit here.
pub struct ThreadQueue {
    pool: PgPool,
    bus: EventBus,
    trigger_configs: Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
    /// Needed because the overflow guard performs a real trigger write
    /// (`TriggerDisabled`) through the trigger write chokepoint, which owns
    /// both halves of the registry projection: the in-memory map and the
    /// on-disk `trigger.toml`.
    workspace_path: std::path::PathBuf,
    /// The engine's `trigger_write_lock`, shared so the overflow guard's write
    /// serializes against every other trigger write rather than racing them.
    trigger_write_lock: Arc<tokio::sync::Mutex<()>>,
    /// Installed via [`Self::attach_engine`] after `Arc::new(engine)` —
    /// executes admitted entries. Test fixtures install mocks.
    executor: OnceLock<Arc<dyn ThreadQueueExecutor>>,
    /// Weak engine handle, used only for push fan-out on queue notifications.
    engine: OnceLock<Weak<LucidosEngine>>,
    /// The engine's own shutdown signal (`LucidosEngine::is_shutting_down`
    /// reads the same atomic). Held rather than reached through `engine`, so a
    /// trigger fire is held even before the engine is attached.
    shutting_down: Arc<AtomicBool>,
    boot_recovery: AtomicBool,
    state: tokio::sync::Mutex<QueueState>,
}

impl ThreadQueue {
    pub fn new(
        pool: PgPool,
        bus: EventBus,
        trigger_configs: Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
        workspace_path: std::path::PathBuf,
        trigger_write_lock: Arc<tokio::sync::Mutex<()>>,
        policy: CapacityPolicy,
        shutting_down: Arc<AtomicBool>,
    ) -> Self {
        Self {
            pool,
            bus,
            trigger_configs,
            workspace_path,
            trigger_write_lock,
            executor: OnceLock::new(),
            engine: OnceLock::new(),
            shutting_down,
            boot_recovery: AtomicBool::new(false),
            state: tokio::sync::Mutex::new(QueueState {
                policy,
                queued: VecDeque::new(),
                active: HashMap::new(),
                user_reservations: HashMap::new(),
                user_queued: VecDeque::new(),
                delay_notified: HashMap::new(),
                global_notified: None,
            }),
        }
    }

    /// Load the capacity policy from the latest persisted
    /// `CapacityPolicyChanged` event; absence (or a parse failure on a
    /// legacy payload) falls back to defaults.
    pub async fn load_policy(pool: &PgPool) -> CapacityPolicy {
        let row: Option<(serde_json::Value,)> = sqlx::query_as(
            "SELECT payload FROM events WHERE event_type = 'CapacityPolicyChanged' \
             ORDER BY sequence DESC LIMIT 1",
        )
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| {
            log!(
                "[ThreadQueue] load_policy query failed: {}, using defaults",
                e
            );
            None
        });
        match row {
            Some((payload,)) => {
                // `SystemEvent::to_payload` persists the serde-tagged enum
                // form: `{"type": "CapacityPolicyChanged", "data": {"policy": …}}`.
                let policy_json = payload
                    .pointer("/data/policy")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                match serde_json::from_value(policy_json) {
                    Ok(p) => p,
                    Err(e) => {
                        log!(
                            "[ThreadQueue] CapacityPolicyChanged payload unparseable: {} — using defaults",
                            e
                        );
                        CapacityPolicy::default()
                    }
                }
            }
            None => CapacityPolicy::default(),
        }
    }

    /// Wire the engine in after `Arc::new(engine)`: installs the real
    /// executor and the weak handle for push fan-out. Called from
    /// `LucidosEngine::set_self_arc`.
    pub fn attach_engine(&self, engine: &Arc<LucidosEngine>) {
        self.engine.set(Arc::downgrade(engine)).ok();
        self.executor
            .set(Arc::new(executor::EngineThreadQueueExecutor::new(
                Arc::downgrade(engine),
            )))
            .ok();
    }

    /// Test seam: install a mock executor instead of the engine-backed one.
    pub fn set_executor(&self, executor: Arc<dyn ThreadQueueExecutor>) {
        self.executor.set(executor).ok();
    }

    pub async fn policy(&self) -> CapacityPolicy {
        self.state.lock().await.policy.clone()
    }

    /// Replace the capacity policy. Emits `CapacityPolicyChanged` (the
    /// persisted source of truth), then drains in case caps were raised.
    pub async fn set_policy(
        self: &Arc<Self>,
        policy: CapacityPolicy,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.bus
            .emit(BusEvent::System(SystemEvent::CapacityPolicyChanged {
                policy: policy.clone(),
                actor,
            }))
            .await?;
        self.state.lock().await.policy = policy;
        self.drain().await;
        Ok(())
    }

    fn trigger_name(&self, trigger_id: &str) -> Option<String> {
        self.trigger_configs
            .read()
            .ok()
            .and_then(|configs| configs.get(trigger_id).map(|c| c.name.clone()))
    }

    /// Whether the registry says the trigger is paused. An unreadable
    /// registry answers `false`, so a stuck queue still alerts.
    fn trigger_paused(&self, trigger_id: &str) -> bool {
        self.trigger_configs
            .read()
            .is_ok_and(|configs| configs.get(trigger_id).is_some_and(|c| c.paused))
    }

    /// Hold trigger work while boot reconstructs its configs and retry reports.
    pub fn begin_boot_recovery(&self) {
        self.boot_recovery
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Checked at every trigger admission, including drain hints and Run now.
    fn trigger_admission_held(&self, kind: ThreadQueueKind) -> bool {
        kind.is_trigger_fire()
            && (self.boot_recovery.load(std::sync::atomic::Ordering::SeqCst)
                || self.shutting_down.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// Submit a background spawn. Admits immediately when capacity allows
    /// (the executor's `prepare` hook runs inline, before this returns);
    /// otherwise the entry waits in the queue. Never blocks on execution.
    pub async fn submit(
        self: &Arc<Self>,
        mut request: ThreadQueueRequest,
        actor: Option<MessageOrigin>,
        cancel: Option<CancellationToken>,
    ) -> SubmitOutcome {
        // Before anything else, so the chain depth travels ON the request
        // rather than in a task-local the spawn below would drop. Read here
        // because a spawn submit is always awaited inline from the spawning
        // task, which is the task that owns the chain.
        request.stamp_caller_depth(crate::scheduler::user_tasks::current_event_trigger_depth());
        // Every spawn passes here, so this is where an unattributed one is
        // caught. Its first message would render Origin "Unknown". Tests fail
        // loudly; a release build still spawns it, since the work is real.
        let attributed = request.is_attributed();
        debug_assert!(
            attributed,
            "spawn submitted with no origin and no parent linkage: {request:?}"
        );
        if !attributed {
            log!(
                "[ThreadQueue] unattributed {:?} spawn submitted; its first message has no origin",
                request.kind()
            );
        }
        let kind = request.kind();
        let trigger_id = request.trigger_id().map(str::to_string);
        let trigger_name = trigger_id.as_deref().and_then(|t| self.trigger_name(t));
        let entry_id = Uuid::new_v4();
        let (completion_tx, completion_rx) = oneshot::channel();

        let mut entry = QueueEntry {
            id: entry_id,
            kind,
            trigger_id: trigger_id.clone(),
            trigger_name: trigger_name.clone(),
            thread_id: request.thread_id(),
            summary: request.summary(trigger_name.as_deref()),
            request,
            queued_at: Utc::now(),
            cancel,
            completion_tx: Some(completion_tx),
        };

        let (decision, user_active) = {
            let mut state = self.state.lock().await;
            let running = self.read_user_occupancy(&mut state, None).await;
            let user_active = state.user_active(&running);
            let counts = state.counts_for(kind, trigger_id.as_deref(), user_active);
            let decision = match state
                .policy
                .decide_submit(counts, kind, trigger_id.is_some())
            {
                AdmissionDecision::Admit if self.trigger_admission_held(kind) => {
                    AdmissionDecision::Queue
                }
                decision => decision,
            };
            match decision {
                AdmissionDecision::Admit => {
                    state.active.insert(
                        entry_id,
                        ActiveSlot {
                            kind,
                            trigger_id: trigger_id.clone(),
                            completion_tx: entry.completion_tx.take(),
                            chain_thread_id: entry.thread_id,
                            chain_depth: entry.request.depth(),
                        },
                    );
                }
                AdmissionDecision::Queue
                | AdmissionDecision::Overflow
                | AdmissionDecision::Coalesce => {}
            }
            (decision, user_active)
        };

        match decision {
            AdmissionDecision::Admit => {
                self.prepare_entry(&mut entry.request).await;
                self.emit_queued(&entry, false, actor).await;
                self.emit_admitted(
                    entry_id,
                    entry.thread_id,
                    entry.trigger_id.clone(),
                    entry.request.depth(),
                    None,
                )
                .await;
                self.spawn_execution(entry);
                SubmitOutcome {
                    entry_id,
                    admitted: true,
                    position: 0,
                    coalesced: false,
                    completion: completion_rx,
                }
            }
            AdmissionDecision::Queue => {
                self.emit_queued(&entry, false, actor).await;
                let (position, notify) = {
                    let mut state = self.state.lock().await;
                    state.queued.push_back(entry);
                    (
                        state.queued.len(),
                        state.capacity_notification_due(user_active),
                    )
                };
                if let Some((title, message)) = notify {
                    self.notify(title, message).await;
                }
                SubmitOutcome {
                    entry_id,
                    admitted: false,
                    position,
                    coalesced: false,
                    completion: completion_rx,
                }
            }
            AdmissionDecision::Overflow => {
                self.handle_overflow(entry, actor).await;
                SubmitOutcome {
                    entry_id,
                    admitted: false,
                    position: 0,
                    coalesced: false,
                    completion: completion_rx,
                }
            }
            AdmissionDecision::Coalesce => {
                // A fire of this cron trigger is already active or queued — this
                // one is redundant (cron fires carry no distinct payload). Drop
                // it without a persisted event (it never entered the queue), and
                // resolve completion so the awaiting submitter (cron loop /
                // missed-grace catch-up) proceeds to the next occurrence rather
                // than hanging on a fire that will never run.
                log!(
                    "[ThreadQueue] Coalesced redundant cron fire for trigger '{}'",
                    trigger_name
                        .as_deref()
                        .or(trigger_id.as_deref())
                        .unwrap_or("?")
                );
                if let Some(tx) = entry.completion_tx.take() {
                    let _ = tx.send(());
                }
                SubmitOutcome {
                    entry_id,
                    admitted: false,
                    position: 0,
                    coalesced: true,
                    completion: completion_rx,
                }
            }
        }
    }

    /// Persist a boot retry without starting execution before scheduler registration.
    pub(crate) async fn submit_boot_retry(
        self: &Arc<Self>,
        request: ThreadQueueRequest,
    ) -> Result<Option<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
        let kind = request.kind();
        assert!(kind.is_trigger_fire());
        let trigger_id = request.trigger_id().map(str::to_owned);
        let trigger_name = trigger_id.as_deref().and_then(|id| self.trigger_name(id));
        let mut state = self.state.lock().await;
        let running = self.read_user_occupancy(&mut state, None).await;
        let user_active = state.user_active(&running);
        let counts = state.counts_for(kind, trigger_id.as_deref(), user_active);
        let decision = state
            .policy
            .decide_submit(counts, kind, trigger_id.is_some());
        if decision == AdmissionDecision::Coalesce {
            return Ok(None);
        }
        let entry = QueueEntry {
            id: Uuid::new_v4(),
            kind,
            trigger_id,
            trigger_name: trigger_name.clone(),
            thread_id: None,
            summary: request.summary(trigger_name.as_deref()),
            request,
            queued_at: Utc::now(),
            cancel: None,
            completion_tx: None,
        };
        self.try_emit_queued(&entry, false, None).await?;
        let id = entry.id;
        if decision == AdmissionDecision::Overflow {
            drop(state);
            self.handle_persisted_overflow(entry).await;
        } else {
            state.queued.push_back(entry);
        }
        Ok(Some(id))
    }

    /// Apply the overflow policy: the trigger's queue is at its ceiling.
    async fn handle_overflow(self: &Arc<Self>, entry: QueueEntry, actor: Option<MessageOrigin>) {
        self.emit_queued(&entry, false, actor).await;
        self.handle_persisted_overflow(entry).await;
    }

    async fn handle_persisted_overflow(self: &Arc<Self>, entry: QueueEntry) {
        let trigger_id = entry
            .trigger_id
            .clone()
            .expect("overflow only fires for trigger-bound entries");
        let trigger_label = entry
            .trigger_name
            .clone()
            .unwrap_or_else(|| trigger_id.clone());
        let overflow = {
            let state = self.state.lock().await;
            state.policy.overflow
        };
        match overflow {
            OverflowPolicy::DropOldest => {
                let (dropped, cap) = {
                    let mut state = self.state.lock().await;
                    let idx = state
                        .queued
                        .iter()
                        .position(|e| e.trigger_id.as_deref() == Some(trigger_id.as_str()));
                    let dropped = idx.and_then(|i| state.queued.remove(i));
                    state.queued.push_back(entry);
                    (dropped, state.policy.max_queued_per_trigger)
                };
                if let Some(mut old) = dropped {
                    if let Some(tx) = old.completion_tx.take() {
                        let _ = tx.send(());
                    }
                    let reason =
                        format!("per-trigger queue cap ({cap}) reached, oldest entry dropped");
                    self.emit_dropped(old.id, &reason, None).await;
                    self.notify(
                        format!("{trigger_label} queue overflowed"),
                        format!(
                            "The queue for trigger \"{trigger_label}\" hit its cap of {cap}; \
                             the oldest waiting fire was dropped: {}",
                            old.summary
                        ),
                    )
                    .await;
                }
            }
            OverflowPolicy::PauseTrigger => {
                let cap = {
                    let mut state = self.state.lock().await;
                    state.queued.push_back(entry);
                    state.policy.max_queued_per_trigger
                };
                // Pause through the trigger write chokepoint, exactly as a user
                // pause would: the event is the record, and the registry is
                // flipped before this returns so the very next admission
                // decision already sees the trigger paused.
                crate::engine::trigger_writes::TriggerRegistryWriter {
                    event_bus: &self.bus,
                    trigger_configs: &self.trigger_configs,
                    workspace_path: &self.workspace_path,
                    write_lock: &self.trigger_write_lock,
                }
                .write_or_log(
                    crate::engine::trigger_writes::TriggerWrite::Disabled,
                    &trigger_id,
                    serde_json::json!({ "reason": "thread-queue overflow" }),
                    None,
                    "[ThreadQueue] (overflow)",
                )
                .await;
                self.notify(
                    format!("{trigger_label} paused — queue overflow"),
                    format!(
                        "Trigger \"{trigger_label}\" hit its queue cap of {cap} and was \
                         paused. Its queued fires wait in the Thread Queue; resume the \
                         trigger to continue."
                    ),
                )
                .await;
            }
        }
    }

    /// Admit whatever fits, by priority. Three passes run under one lock:
    ///
    /// 1. **Background reclaim-floor** — admit background up to
    ///    `reserved_background` so user priority can't starve triggers/cron.
    /// 2. **User priority** — admit waiting user-initiated responses (FIFO)
    ///    while the pool has room. A person waits only at true pool-max.
    /// 3. **Background fill** — admit the rest of the background backlog while
    ///    capacity allows.
    ///
    /// Within background, per-trigger FIFO is strict (once a trigger is skipped
    /// — paused or at its cap — every later entry of it is skipped too);
    /// cross-trigger order is best-effort.
    pub async fn drain(self: &Arc<Self>) {
        let (to_admit, to_drop, woke) = {
            let mut state = self.state.lock().await;
            let running = self.read_user_occupancy(&mut state, None).await;
            let mut to_admit: Vec<QueueEntry> = Vec::new();
            let mut to_drop: Vec<(QueueEntry, String)> = Vec::new();

            // Phase 1 — background reclaims up to its reserved floor first.
            let floor = state.policy.effective_reserved_background();
            self.drain_background_into(&mut state, &running, floor, &mut to_admit, &mut to_drop);
            // Phase 2 — user-initiated waiters take priority for free slots.
            let woke = self.drain_users(&mut state, &running).await;
            // Phase 3 — background fills whatever capacity remains.
            self.drain_background_into(
                &mut state,
                &running,
                usize::MAX,
                &mut to_admit,
                &mut to_drop,
            );

            (to_admit, to_drop, woke)
        };

        for (mut entry, reason) in to_drop {
            if let Some(tx) = entry.completion_tx.take() {
                let _ = tx.send(());
            }
            self.emit_dropped(entry.id, &reason, None).await;
        }
        // Unblock admitted user waiters; their queued→running move is in-memory
        // (no persisted event), so refresh the panel explicitly.
        let woke_any = !woke.is_empty();
        for tx in woke {
            let _ = tx.send(());
        }
        if woke_any {
            self.emit_changed().await;
        }
        for mut entry in to_admit {
            self.prepare_entry(&mut entry.request).await;
            self.emit_admitted(
                entry.id,
                entry.thread_id,
                entry.trigger_id.clone(),
                entry.request.depth(),
                None,
            )
            .await;
            self.spawn_execution(entry);
        }
    }

    /// Run the executor's admission hook at the request's chain depth.
    ///
    /// `prepare` emits: a sub-thread's eager `MessageReceived` goes out here.
    /// It runs on three different tasks, the submitter's, the drainer's and
    /// `run_now`'s, and only the first of those is inside the fire. Scoping the
    /// hook makes the emitted event carry the same depth whichever admitted it.
    async fn prepare_entry(&self, request: &mut ThreadQueueRequest) {
        let Some(executor) = self.executor.get() else {
            return;
        };
        let depth = request.depth();
        crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH
            .scope(depth, executor.prepare(request))
            .await;
    }

    /// Background admission scan (drain phases 1 & 3). Admits queued background
    /// entries oldest-first — respecting per-trigger pause/deletion + FIFO and
    /// the capacity caps — until background occupies `bg_target` slots or the
    /// backlog is exhausted. Admitted entries land in `to_admit`; entries whose
    /// trigger has vanished land in `to_drop`.
    fn drain_background_into(
        &self,
        state: &mut QueueState,
        running: &HashMap<Uuid, RunningThread>,
        bg_target: usize,
        to_admit: &mut Vec<QueueEntry>,
        to_drop: &mut Vec<(QueueEntry, String)>,
    ) {
        let mut skipped: HashSet<String> = HashSet::new();
        let mut i = 0;
        while i < state.queued.len() {
            if state.active.len() >= bg_target {
                break; // floor reached for this pass
            }
            let (kind, trigger_id) = {
                let e = &state.queued[i];
                (e.kind, e.trigger_id.clone())
            };
            if self.trigger_admission_held(kind) {
                i += 1;
                continue;
            }
            if let Some(ref tid) = trigger_id {
                if skipped.contains(tid) {
                    i += 1;
                    continue;
                }
                // A registry we could not READ is unknown, never "the trigger
                // is gone": that arm drops the fire. `.ok()` collapsed the two,
                // so one poisoning panic discarded every queued fire in the
                // workspace on the next drain.
                let paused = match self.trigger_configs.read() {
                    Ok(configs) => configs.get(tid).map(|c| c.paused),
                    Err(e) => {
                        log!(
                            "[ThreadQueue] Trigger registry unreadable ({}); leaving {}'s queued fires in place",
                            e,
                            tid
                        );
                        skipped.insert(tid.clone());
                        i += 1;
                        continue;
                    }
                };
                match paused {
                    None => {
                        // Trigger no longer exists — its queued fires are
                        // undeliverable.
                        if let Some(e) = state.queued.remove(i) {
                            to_drop.push((e, "trigger no longer exists".to_string()));
                        }
                        continue;
                    }
                    Some(true) => {
                        // Paused — entries wait for resume.
                        skipped.insert(tid.clone());
                        i += 1;
                        continue;
                    }
                    Some(false) => {}
                }
            }
            let counts = state.counts_for(kind, trigger_id.as_deref(), state.user_active(running));
            match state
                .policy
                .decide_drain(counts, kind, trigger_id.is_some())
            {
                AdmissionDecision::Admit => {
                    if let Some(mut e) = state.queued.remove(i) {
                        state.active.insert(
                            e.id,
                            ActiveSlot {
                                kind: e.kind,
                                trigger_id: e.trigger_id.clone(),
                                completion_tx: e.completion_tx.take(),
                                chain_thread_id: e.thread_id,
                                chain_depth: e.request.depth(),
                            },
                        );
                        to_admit.push(e);
                    }
                }
                _ => {
                    if let Some(tid) = trigger_id {
                        skipped.insert(tid);
                    }
                    i += 1;
                }
            }
        }
    }

    /// User admission scan (drain phase 2). Admits user-initiated waiters
    /// FIFO while the pool has a free slot. Returns the wake senders to fire
    /// after the lock is released (each unblocks its waiting chat task).
    async fn drain_users(
        &self,
        state: &mut QueueState,
        running: &HashMap<Uuid, RunningThread>,
    ) -> Vec<oneshot::Sender<()>> {
        let mut woke = Vec::new();
        while let Some(thread_id) = state.user_queued.front().map(|w| w.thread_id) {
            let besides = state.user_active_besides(running, thread_id);
            if !state.policy.user_can_admit(state.active.len() + besides) {
                break;
            }
            let waiter = state.user_queued.pop_front().expect("front checked above");
            self.reserve_user_slot(state, running, waiter.entry_id, thread_id, waiter.summary)
                .await;
            woke.push(waiter.wake);
        }
        woke
    }

    /// Force-admit a queued entry, ignoring every cap. User intent ("Run
    /// now" in the Thread Queue panel), and `actor` stamps the admission.
    /// The shutdown hold still applies: it is not a cap.
    pub async fn run_now(
        self: &Arc<Self>,
        entry_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), String> {
        let mut entry = {
            let mut state = self.state.lock().await;
            let idx = state
                .queued
                .iter()
                .position(|e| e.id == entry_id)
                .ok_or_else(|| "entry is not queued (already running or gone)".to_string())?;
            if self.trigger_admission_held(state.queued[idx].kind) {
                return Err(
                    "trigger admission is held during engine startup or shutdown".to_string(),
                );
            }
            let mut entry = state
                .queued
                .remove(idx)
                .expect("index verified by position()");
            state.active.insert(
                entry.id,
                ActiveSlot {
                    kind: entry.kind,
                    trigger_id: entry.trigger_id.clone(),
                    completion_tx: entry.completion_tx.take(),
                    chain_thread_id: entry.thread_id,
                    chain_depth: entry.request.depth(),
                },
            );
            entry
        };
        self.prepare_entry(&mut entry.request).await;
        self.emit_admitted(
            entry.id,
            entry.thread_id,
            entry.trigger_id.clone(),
            entry.request.depth(),
            actor,
        )
        .await;
        self.spawn_execution(entry);
        Ok(())
    }

    /// Drop a queued entry without running it. User intent from the panel,
    /// or internal cleanup (unparseable request at boot).
    pub async fn drop_entry(
        self: &Arc<Self>,
        entry_id: Uuid,
        reason: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<(), String> {
        let mut entry = {
            let mut state = self.state.lock().await;
            let idx = state
                .queued
                .iter()
                .position(|e| e.id == entry_id)
                .ok_or_else(|| "entry is not queued (already running or gone)".to_string())?;
            state
                .queued
                .remove(idx)
                .expect("index verified by position()")
        };
        if let Some(tx) = entry.completion_tx.take() {
            let _ = tx.send(());
        }
        self.emit_dropped(entry_id, reason, actor).await;
        Ok(())
    }

    /// Release an admitted entry's capacity slot — its work finished (any
    /// outcome). Resolves the submitter's completion handle and drains.
    pub async fn complete(self: &Arc<Self>, entry_id: Uuid) {
        self.finish_entry(entry_id, true).await;
    }

    async fn finish_entry(self: &Arc<Self>, entry_id: Uuid, completed: bool) {
        let slot = {
            let mut state = self.state.lock().await;
            state.active.remove(&entry_id)
        };
        let Some(mut slot) = slot else {
            return; // double-complete or unknown id — nothing to release
        };
        // The chain depth lasts as long as the work, not as long as the thread:
        // a user who Continues this thread later starts a fresh chain at 0.
        // Only THIS entry's binding goes, so a sibling entry on the same thread
        // keeps its own.
        if let Some(tid) = slot.chain_thread_id {
            crate::scheduler::user_tasks::forget_chain_depth(tid, entry_id);
        }
        if let Some(tx) = slot.completion_tx.take() {
            let _ = tx.send(());
        }
        // At the entry's depth, for the same reason it states its trigger: this
        // frame closes that fire, and it goes out on the sibling task that
        // joined the work rather than on the fire's own.
        if completed {
            self.at_chain_depth(
                slot.chain_depth,
                self.bus.emit_or_log_as_trigger(
                    BusEvent::System(SystemEvent::ThreadQueueCompleted { entry_id }),
                    "[ThreadQueue] ThreadQueueCompleted",
                    slot.trigger_id.clone(),
                ),
            )
            .await;
        }
        self.drain().await;
    }

    /// Bind the thread a trigger fire just created to its admitted entry.
    ///
    /// A cron / event-trigger entry has no thread at submit time: the fire
    /// creates one only while it executes, so its `ThreadQueueAdmitted` went
    /// out with a `None` thread id and the row stayed unbound. An unbound row
    /// is what left [`Self::recover_persisted_entries`] unable to tell a fire
    /// that already started from one that never did. It re-ran the whole
    /// trigger after a restart. See
    /// `docs/plans/2026-08-25-thread-queue-boot-handoff-for-trigger-fires.md`.
    ///
    /// Re-emitting `ThreadQueueAdmitted` is the carrier: the projection already
    /// COALESCEs the id onto the row, holds the status at `admitted`, and keeps
    /// the original `admitted_at`. Best-effort like every other queue emit, so
    /// an entry already completed or dropped simply matches no row.
    pub async fn record_entry_thread(&self, entry_id: Uuid, thread_id: Uuid) {
        // A trigger fire's thread appears only now, so this is also where its
        // chain depth gets bound. Everything the fire's thread emits from a
        // task the executor's scope cannot reach resolves through that binding.
        let (trigger_id, chain_depth) = {
            let mut state = self.state.lock().await;
            match state.active.get_mut(&entry_id) {
                Some(slot) => {
                    slot.chain_thread_id = Some(thread_id);
                    crate::scheduler::user_tasks::register_chain_depth(
                        thread_id,
                        entry_id,
                        slot.chain_depth,
                    );
                    (slot.trigger_id.clone(), slot.chain_depth)
                }
                None => (None, 0),
            }
        };
        self.emit_admitted(entry_id, Some(thread_id), trigger_id, chain_depth, None)
            .await;
    }

    /// Bind a thread's events to this work's chain depth, for the emits the
    /// executor's task-local scope cannot reach (see
    /// `scheduler::user_tasks::register_chain_depth`).
    fn register_chain_thread(&self, entry_id: Uuid, thread_id: Option<Uuid>, depth: u32) {
        if let Some(tid) = thread_id {
            crate::scheduler::user_tasks::register_chain_depth(tid, entry_id, depth);
        }
    }

    /// Run an emit at `depth`, for a queue frame that goes out on a task the
    /// fire does not own.
    ///
    /// The same reasoning that makes those frames state their trigger owner
    /// (`docs/adr/0137-a-trigger-never-wakes-itself.md`) applies to the depth.
    /// `ThreadQueueCompleted` comes from the sibling task that joins the work,
    /// so the ambient scope reads 0 there. Two triggers subscribed to it would
    /// then wake each other at depth 1 forever, with nothing to end it.
    async fn at_chain_depth<F: std::future::Future>(&self, depth: u32, fut: F) -> F::Output {
        crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH
            .scope(depth, fut)
            .await
    }

    // ---- User-initiated work (preempting, prioritized, counted) ----

    /// Reserve a pool slot for a NEW user-initiated response (chat / user-typed
    /// coding-agent thread). This is the **back-pressure gate**: user work is
    /// **prioritized but not exempt** — it admits immediately when the pool has
    /// a free slot, otherwise it joins the priority line and this call *awaits*
    /// until a slot frees, so a person briefly waits only at true pool-max
    /// (ADR 0008). In-memory only — never persisted.
    ///
    /// The admission leaves a reservation until the thread starts running. From
    /// then on `thread_summaries` counts the thread (ADR 0443). The returned
    /// guard's drop removes a reservation the thread never reached.
    ///
    /// **The guard is built before the first `.await` after the reservation.**
    /// The reservation lands in `user_queued` / `user_reservations` under the
    /// lock, and everything after that is cancellable. An aborted caller drops
    /// this future at `emit_changed` or at the wake. A reservation with no guard
    /// behind it has nobody left to release it: the drainer admits the dead
    /// waiter, its wake send fails into `let _ =`, and the slot is held for the
    /// life of the process. Owning it from the moment it exists releases it.
    pub async fn acquire_user_slot(
        self: &Arc<Self>,
        thread_id: Uuid,
        summary: String,
    ) -> UserSlotGuard {
        let entry_id = Uuid::new_v4();
        let wait = {
            let mut state = self.state.lock().await;
            let running = self.read_user_occupancy(&mut state, Some(thread_id)).await;
            let besides = state.user_active_besides(&running, thread_id);
            if state.policy.user_can_admit(state.active.len() + besides) {
                self.reserve_user_slot(&mut state, &running, entry_id, thread_id, summary)
                    .await;
                None
            } else {
                let (tx, rx) = oneshot::channel();
                state.user_queued.push_back(UserWaiter {
                    entry_id,
                    thread_id,
                    summary,
                    queued_at: Utc::now(),
                    wake: tx,
                });
                Some(rx)
            }
        };
        // Own the reservation before the next await, so cancelling anything
        // below releases it instead of stranding it. See the doc comment.
        let guard = UserSlotGuard {
            queue: self.clone(),
            entry_id,
        };
        // Panel: a new running (admitted) or waiting (queued) user entry.
        self.emit_changed().await;
        if let Some(rx) = wait {
            // Block until the drainer admits us. A dropped sender (engine
            // teardown) resolves Err — proceed; the slot dies with the process.
            let _ = rx.await;
        }
        guard
    }

    /// Record an admission. On a thread already `running` the reservation is
    /// born seen: the thread's status holds the slot. Any other reservation
    /// counts until its thread starts a turn. The await reads the thread's
    /// high-water mark and inserts nothing, so a cancel there leaves no trace.
    async fn reserve_user_slot(
        &self,
        state: &mut QueueState,
        running: &HashMap<Uuid, RunningThread>,
        entry_id: Uuid,
        thread_id: Uuid,
        summary: String,
    ) {
        let seen = running.contains_key(&thread_id);
        let after_seq = if seen {
            0
        } else {
            self.thread_high_water(thread_id).await
        };
        state.user_reservations.insert(
            entry_id,
            UserReservation {
                thread_id,
                summary,
                admitted_at: Utc::now(),
                after_seq,
                seen,
            },
        );
    }

    /// The thread's highest event sequence. A failed read returns 0, so the
    /// reservation counts as seen by the thread's next status read: an unknown
    /// admits rather than holding a slot nobody can release.
    async fn thread_high_water(&self, thread_id: Uuid) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(sequence), 0) FROM events \
             WHERE aggregate = 'thread' AND aggregate_id = $1",
        )
        .bind(thread_id.to_string())
        .fetch_one(&self.pool)
        .await
        .unwrap_or_else(|e| {
            log!(
                "[ThreadQueue] high-water read failed for {}: {}",
                thread_id,
                e
            );
            0
        })
    }

    /// Read user occupancy for an admission, under the state lock (ADR 0443).
    /// Returns the running threads that occupy a user slot: every user-started
    /// one, plus any a live admission, a waiter or `candidate` is for, whoever
    /// started it. Marks each reservation seen once its thread is running or
    /// has started a turn since admission.
    ///
    /// A seen reservation stays until its guard drops. It keeps a follow-up's
    /// thread in the read above, so an agent-started thread the user resumed
    /// counts while it runs.
    ///
    /// The started-a-turn check is what keeps a parked thread at zero. A
    /// thread that starts and parks between two admissions is never observed
    /// running, but its turn start is in the log either way.
    ///
    /// A failed read counts no running threads and marks nothing seen: an
    /// unknown admits work rather than blocking every chat on the pool.
    async fn read_user_occupancy(
        &self,
        state: &mut QueueState,
        candidate: Option<Uuid>,
    ) -> HashMap<Uuid, RunningThread> {
        let admitted: Vec<Uuid> = state
            .user_reservations
            .values()
            .map(|r| r.thread_id)
            .chain(state.user_queued.iter().map(|w| w.thread_id))
            .chain(candidate)
            .collect();
        let running: HashMap<Uuid, RunningThread> =
            match sqlx::query_as::<_, (Uuid, String, DateTime<Utc>)>(&format!(
                "SELECT thread_id, \
                    COALESCE(NULLIF(title, ''), NULLIF(first_message, ''), ''), \
                    COALESCE(last_revived_at, created_at) \
                 FROM thread_summaries \
                 WHERE status = {} AND (initiator = 'user' OR thread_id = ANY($1))",
                ThreadStatus::Running.sql_literal()
            ))
            .bind(&admitted)
            .fetch_all(&self.pool)
            .await
            {
                Ok(rows) => rows
                    .into_iter()
                    .map(|(tid, summary, revived_at)| {
                        let summary = truncate_summary(summary.trim());
                        (
                            tid,
                            RunningThread {
                                summary,
                                revived_at,
                            },
                        )
                    })
                    .collect(),
                Err(e) => {
                    log!("[ThreadQueue] user occupancy read failed: {}", e);
                    return HashMap::new();
                }
            };
        for r in state.user_reservations.values_mut() {
            if running.contains_key(&r.thread_id) {
                r.seen = true;
            }
        }

        let (ids, threads, after): (Vec<Uuid>, Vec<String>, Vec<i64>) = state
            .user_reservations
            .iter()
            .filter(|(_, r)| !r.seen)
            .map(|(id, r)| (*id, r.thread_id.to_string(), r.after_seq))
            .fold(
                (Vec::new(), Vec::new(), Vec::new()),
                |(mut ids, mut threads, mut after), (id, tid, seq)| {
                    ids.push(id);
                    threads.push(tid);
                    after.push(seq);
                    (ids, threads, after)
                },
            );
        if ids.is_empty() {
            return running;
        }
        match sqlx::query_scalar::<_, Uuid>(
            "SELECT r.entry_id \
             FROM unnest($1::uuid[], $2::text[], $3::bigint[]) AS r(entry_id, thread, after_seq) \
             WHERE EXISTS ( \
                 SELECT 1 FROM events e \
                 WHERE e.aggregate = 'thread' AND e.aggregate_id = r.thread \
                   AND e.sequence > r.after_seq AND e.event_type = ANY($4) \
             )",
        )
        .bind(&ids)
        .bind(&threads)
        .bind(&after)
        .bind(TURN_START_EVENT_TYPES)
        .fetch_all(&self.pool)
        .await
        {
            Ok(seen) => {
                for id in seen {
                    if let Some(r) = state.user_reservations.get_mut(&id) {
                        r.seen = true;
                    }
                }
            }
            Err(e) => log!("[ThreadQueue] reservation read failed: {}", e),
        }
        running
    }

    /// Release the gate's reservation or waiter, keyed by the guard's
    /// `entry_id`. Fires from [`UserSlotGuard`]'s drop when the chat task ends.
    /// An unseen reservation frees its slot here, and so does a seen one whose
    /// thread was counted only because of it. A still-**queued** waiter whose
    /// task gave up is dropped from the line. Refreshes the panel and drains.
    async fn release_user_slot(self: &Arc<Self>, entry_id: Uuid) {
        let removed = {
            let mut state = self.state.lock().await;
            if state.user_reservations.remove(&entry_id).is_some() {
                true
            } else {
                let before = state.user_queued.len();
                state.user_queued.retain(|w| w.entry_id != entry_id);
                state.user_queued.len() != before
            }
        };
        if removed {
            self.emit_changed().await;
            self.drain().await;
        }
    }

    /// Every user-initiated occupant, for the panel API to merge with the
    /// persisted background rows: occupying running threads and unseen
    /// reservations (admitted = Running), then waiters (queued = Queued).
    pub async fn user_entries(&self) -> Vec<UserQueueEntry> {
        let mut state = self.state.lock().await;
        let running = self.read_user_occupancy(&mut state, None).await;
        let mut out = Vec::with_capacity(
            running.len() + state.user_reservations.len() + state.user_queued.len(),
        );
        for (tid, thread) in running {
            out.push(UserQueueEntry {
                id: tid,
                thread_id: tid,
                summary: thread.summary,
                status: "admitted",
                queued_at: thread.revived_at,
                admitted_at: Some(thread.revived_at),
            });
        }
        for (id, r) in state.user_reservations.iter().filter(|(_, r)| !r.seen) {
            out.push(UserQueueEntry {
                id: *id,
                thread_id: r.thread_id,
                summary: r.summary.clone(),
                status: "admitted",
                queued_at: r.admitted_at,
                admitted_at: Some(r.admitted_at),
            });
        }
        for w in &state.user_queued {
            out.push(UserQueueEntry {
                id: w.entry_id,
                thread_id: w.thread_id,
                summary: w.summary.clone(),
                status: "queued",
                queued_at: w.queued_at,
                admitted_at: None,
            });
        }
        out
    }

    /// The merged Thread Queue view — persisted background rows (FIFO by
    /// `sequence`) followed by the user-initiated occupants
    /// (`kind: "user-chat"`) — plus the active capacity policy. The SINGLE
    /// source of truth shared by `GET /api/v1/thread-queue` and the
    /// `list_thread_queue` LLM tool, so the panel and the tool can never
    /// disagree about who occupies the pool.
    pub async fn snapshot(&self) -> Result<ThreadQueueSnapshot, sqlx::Error> {
        let mut entries: Vec<ThreadQueueEntryView> = sqlx::query_as(
            "SELECT id, kind, trigger_id, trigger_name, thread_id, summary, status, queued_at, admitted_at \
             FROM thread_queue ORDER BY sequence",
        )
        .fetch_all(&self.pool)
        .await?;
        for u in self.user_entries().await {
            entries.push(ThreadQueueEntryView {
                id: u.id,
                kind: "user-chat".to_string(),
                trigger_id: None,
                trigger_name: None,
                thread_id: Some(u.thread_id),
                summary: u.summary,
                status: u.status.to_string(),
                queued_at: u.queued_at,
                admitted_at: u.admitted_at,
            });
        }
        let policy = self.policy().await;
        Ok(ThreadQueueSnapshot { entries, policy })
    }

    /// Drain, and refresh the panel, whenever user occupancy may have moved.
    /// Spawned once at boot. A hint only: admission reads occupancy itself, so
    /// a lag loses nothing and just triggers a drain (ADR 0443).
    pub fn spawn_drain_hint_subscriber(self: &Arc<Self>) {
        let rx = self.bus.subscribe();
        tokio::spawn(self.clone().run_drain_hints(rx));
    }

    async fn run_drain_hints(
        self: Arc<Self>,
        mut rx: tokio::sync::broadcast::Receiver<crate::engine::event_bus::EmittedEvent>,
    ) {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match rx.recv().await {
                Ok(emitted) => {
                    if self.is_occupancy_hint(&emitted).await {
                        self.occupancy_moved().await;
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    log!(
                        "[ThreadQueue] drain hint subscriber lagged by {} events; draining",
                        n
                    );
                    self.occupancy_moved().await;
                }
                Err(RecvError::Closed) => {
                    log!("[ThreadQueue] drain hint channel closed — exiting");
                    break;
                }
            }
        }
    }

    /// A committed status move on a thread that may hold a user slot: a user
    /// thread, or one a live admission is for. A missing aggregate counts as
    /// user, since a spare drain costs nothing.
    async fn is_occupancy_hint(&self, emitted: &crate::engine::event_bus::EmittedEvent) -> bool {
        let BusEvent::Thread {
            thread_id, event, ..
        } = &emitted.typed
        else {
            return false;
        };
        if emitted.seq.is_none() || !moves_user_occupancy(event) {
            return false;
        }
        let user_started = emitted
            .aggregate
            .as_ref()
            .is_none_or(|a| a.initiator == crate::core::store::LegacyInitiator::User);
        user_started
            || self
                .state
                .lock()
                .await
                .user_reservations
                .values()
                .any(|r| r.thread_id == *thread_id)
    }

    async fn occupancy_moved(self: &Arc<Self>) {
        self.emit_changed().await;
        self.drain().await;
    }

    /// Transient panel refresh after an in-memory-only change (a user slot
    /// admitted / queued / released). Background changes already emit persisted
    /// `ThreadQueue*` events; user slots don't, so this nudges the panel to
    /// refetch and merge the in-memory user entries.
    async fn emit_changed(&self) {
        self.bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ThreadQueueChanged {}),
                "[ThreadQueue] ThreadQueueChanged",
            )
            .await;
    }

    /// Spawn admitted work and release its slot after execution, even on panic.
    /// A failed retry deadline check retains the admitted entry for recovery.
    ///
    /// **The spawn is where the chain used to end.** `EVENT_TRIGGER_DEPTH`
    /// follows an await chain and not a `tokio::spawn`, so execution read 0
    /// while the `prepare` beside it read the fire's depth. Re-establishing the
    /// scope here, from the value the request carries, is what lets
    /// the depth cap end a loop that passes through spawned work.
    /// Work that spawns AGAIN inside itself is covered by the per-thread
    /// registration below, not by this scope.
    fn spawn_execution(self: &Arc<Self>, entry: QueueEntry) {
        let entry_id = entry.id;
        let Some(executor) = self.executor.get().cloned() else {
            // No executor wired (engine still booting) — leave the slot
            // admitted; the boot requeue sweep recovers it on next start.
            log!(
                "[ThreadQueue] No executor installed — entry {} stays admitted unexecuted",
                entry_id
            );
            return;
        };
        let depth = entry.request.depth();
        self.register_chain_thread(entry.id, entry.thread_id, depth);
        let executable = ExecutableEntry {
            id: entry.id,
            request: entry.request,
            cancel: entry.cancel,
        };
        let mgr = self.clone();
        let dispatch = self.clone();
        let work = tokio::spawn(crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH.scope(
            depth,
            async move {
                let expired = dispatch
                    .drop_retry_if_expired(executable.id, &executable.request)
                    .await?;
                if !expired {
                    executor.execute(executable).await;
                }
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(!expired)
            },
        ));
        // Lives in the joiner, which outlasts the work whether it returns,
        // fails or panics. A script trigger has no chat turn of its own.
        let awake = crate::core::keep_awake::hold(
            crate::core::keep_awake::Work::QueueEntry,
            entry_id.to_string(),
        );
        tokio::spawn(async move {
            let _awake = awake;
            let outcome = work.await;
            match outcome {
                Ok(Ok(completed)) => mgr.finish_entry(entry_id, completed).await,
                Ok(Err(error)) => {
                    // Keep the admitted row and slot for recovery. A failed
                    // time query must never authorize replaying an old fire.
                    log!(
                        "[ThreadQueue] Retry deadline check failed for {}: {}",
                        entry_id,
                        error
                    );
                    mgr.notify(
                        "An interrupted run could not be retried".into(),
                        "Lucidos could not check the retry deadline. The entry remains in the Thread Queue for recovery.".into(),
                    ).await;
                }
                Err(join_err) => {
                    if join_err.is_panic() {
                        log!(
                            "[ThreadQueue] Entry {} execution panicked: {:?}",
                            entry_id,
                            join_err
                        );
                    }
                    mgr.complete(entry_id).await;
                }
            }
        });
    }

    // ---- Startup recovery ----

    /// Rebuild in-memory state from the `thread_queue` projection. Called at
    /// boot BEFORE any submission path is live (scheduler not started), so
    /// per-trigger FIFO holds across the restart:
    ///
    /// - `queued` rows load back into the in-memory queue (no re-emit).
    /// - `admitted` rows are work the previous process had already started.
    ///   One rule covers all four kinds: an entry whose `thread_id` names a
    ///   live `thread_summaries` row completes here, and thread-level recovery
    ///   (CC auto-resume / chat settle) owns it from there. An entry with no
    ///   thread re-queues. A trigger entry can re-queue only once: scripts
    ///   create no thread even when side effects have already happened.
    ///
    /// Draining starts separately via [`Self::start_draining`] once trigger
    /// configs are loaded.
    pub async fn recover_persisted_entries(self: &Arc<Self>) {
        #[derive(sqlx::FromRow)]
        struct Row {
            id: Uuid,
            kind: String,
            trigger_id: Option<String>,
            trigger_name: Option<String>,
            thread_id: Option<Uuid>,
            summary: String,
            request: serde_json::Value,
            status: String,
            queued_at: DateTime<Utc>,
            requeued: bool,
        }
        let rows: Vec<Row> = match sqlx::query_as(
            "SELECT q.id, q.kind, q.trigger_id, q.trigger_name, q.thread_id, q.summary, \
             q.request, q.status, q.queued_at, COALESCE((SELECT \
               (e.payload->'data'->>'requeued')::boolean FROM events e \
               WHERE e.event_type = 'ThreadQueued' AND e.payload->'data'->>'entry_id' = q.id::text \
               ORDER BY e.sequence DESC LIMIT 1), false) AS requeued \
             FROM thread_queue q ORDER BY q.sequence",
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                log!(
                    "[ThreadQueue] recover_persisted_entries query failed: {}",
                    e
                );
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        log!(
            "[ThreadQueue] Recovering {} persisted entr(ies) from thread_queue",
            rows.len()
        );

        // Cron coalescing on recovery: rows arrive oldest-first (ORDER BY
        // sequence). The first cron row of a trigger to go BACK IN THE QUEUE is
        // kept. Any later one is a duplicate scheduled fire, so drop it and
        // reboots stay cron-idempotent.
        //
        // A trigger enters this set only where an entry is actually pushed
        // below, never on the handoff path. A handed-off fire already ran, so
        // letting it consume the trigger's slot would drop a sibling row that
        // never ran and still owes a run.
        let mut seen_cron: HashSet<String> = HashSet::new();

        for row in rows {
            let request: ThreadQueueRequest = match serde_json::from_value(row.request) {
                Ok(r) => r,
                Err(e) => {
                    log!(
                        "[ThreadQueue] Entry {} has unparseable request ({}) — dropping",
                        row.id,
                        e
                    );
                    self.emit_dropped(row.id, "unparseable request after restart", None)
                        .await;
                    continue;
                }
            };
            let kind = request.kind();
            debug_assert_eq!(kind.as_str(), row.kind);

            // Deliberately NOT re-registering the chain depth here. A row that
            // hands off below completes without an in-memory slot, so nothing
            // would ever clear the binding. It would outlive the work, and a
            // later user Continue on that thread would inherit a chain it has
            // nothing to do with. A row that RE-QUEUES needs no help: it
            // registers when it is admitted. What survives the restart is the
            // depth on the request, which is the half that has to.
            //
            // So work handed to thread recovery resumes at depth 0. That is
            // safe because the resume is gated on cause (CLAUDE.md § Engine
            // Statelessness): a crash leaves the manual Continue button, so a
            // loop cannot restart itself into a fresh budget.
            if kind == ThreadQueueKind::Cron {
                if let Some(tid) = row.trigger_id.as_deref() {
                    if seen_cron.contains(tid) {
                        // A fire for this cron trigger is already back in the
                        // queue, so this row is redundant. Emit
                        // ThreadQueueDropped to clear its projection row; don't
                        // reload it into memory.
                        self.emit_dropped(
                            row.id,
                            "coalesced on recovery — duplicate scheduled fire",
                            None,
                        )
                        .await;
                        continue;
                    }
                }
            }

            let requeue = if row.status == "queued" {
                false // already queued — load silently, keep original queued_at
            } else {
                // An `admitted` row is work the dead process had already
                // started. Whether re-running it would DUPLICATE that work
                // turns on one question, the same for every kind: did a thread
                // materialize? A spawn kind binds its thread at submit time,
                // and a trigger fire binds one as soon as it mints one (see
                // `record_entry_thread`). So the column answers for all four.
                let thread_exists = match row.thread_id {
                    Some(tid) => {
                        match sqlx::query_scalar::<_, bool>(
                            "SELECT EXISTS(SELECT 1 FROM thread_summaries WHERE thread_id = $1)",
                        )
                        .bind(tid)
                        .fetch_one(&self.pool)
                        .await
                        {
                            Ok(exists) => exists,
                            // A probe that could not run is UNKNOWN, so name the
                            // side we fall to. Re-queuing risks a duplicate run
                            // the user can see and stop. Handing off on a
                            // transient read error abandons the fire in silence,
                            // with nothing in the inbox to act on.
                            Err(e) => {
                                log!(
                                    "[ThreadQueue] thread_summaries probe failed for entry {} (thread {}): {}, re-queuing",
                                    row.id,
                                    tid,
                                    e
                                );
                                false
                            }
                        }
                    }
                    None => false,
                };
                if thread_exists {
                    // The work happened, so thread-level recovery owns it. This
                    // is the ONLY thing standing between a trigger fire that
                    // parked on a question and a second identical run of it:
                    // parking emits no terminal event, so nothing ever
                    // completed the entry and the row is still `admitted`.
                    log!(
                        "[ThreadQueue] Entry {} ({}) already materialized thread {:?}, handing off to thread recovery",
                        row.id,
                        kind.as_str(),
                        row.thread_id
                    );
                    self.bus
                        .emit_or_log_as_trigger(
                            BusEvent::System(SystemEvent::ThreadQueueCompleted {
                                entry_id: row.id,
                            }),
                            "[ThreadQueue] ThreadQueueCompleted (boot handoff)",
                            row.trigger_id.clone(),
                        )
                        .await;
                    continue;
                }
                // A crash after the deadline notice but before the drop must
                // finish that same drop, even on an entry already re-queued.
                match self.drop_retry_if_expired(row.id, &request).await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        log!(
                            "[ThreadQueue] Failed to check recovered retry deadline for {}: {}",
                            row.id,
                            error
                        );
                        continue;
                    }
                }
                // Scripts create no thread, so a second cut cannot hand off.
                // Bound trigger re-admission, including a cut before an intent
                // fire creates its thread. Spawn kinds keep their own recovery.
                if let Some(trigger_id) = row.trigger_id.as_deref().filter(|_| row.requeued) {
                    if let Err(error) = self.drop_interrupted_entry(
                        row.id,
                        trigger_id,
                        None,
                        "requeue-cut-again",
                        "An engine restart interrupted this run twice. Lucidos will not run it again until its next fire.",
                    ).await {
                        log!("[ThreadQueue] Failed to report repeated interruption for {}: {}", row.id, error);
                    }
                    continue;
                }
                true
            };

            let entry = QueueEntry {
                id: row.id,
                kind,
                trigger_id: row.trigger_id,
                trigger_name: row.trigger_name,
                thread_id: row.thread_id,
                summary: row.summary,
                request,
                queued_at: if requeue { Utc::now() } else { row.queued_at },
                cancel: None,
                completion_tx: None,
            };
            if requeue {
                if let Err(error) = self.try_emit_queued(&entry, true, None).await {
                    log!(
                        "[ThreadQueue] Failed to persist boot requeue for {}: {}",
                        entry.id,
                        error
                    );
                    continue;
                }
            }
            if kind == ThreadQueueKind::Cron {
                if let Some(tid) = entry.trigger_id.clone() {
                    seen_cron.insert(tid);
                }
            }
            self.state.lock().await.queued.push_back(entry);
        }
    }

    /// Kick off the drain loop: an immediate drain (queued backlog from the
    /// previous process), then the periodic safety-net drain + delay
    /// notification check. Call AFTER the scheduler has replayed trigger
    /// configs — drain consults them for pause/deletion.
    pub fn start_draining(self: &Arc<Self>) {
        self.boot_recovery
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let mgr = self.clone();
        tokio::spawn(async move {
            loop {
                mgr.drain().await;
                let due = {
                    let mut state = mgr.state.lock().await;
                    mgr.delay_notifications_due(&mut state, Utc::now())
                };
                for (title, message) in due {
                    mgr.notify(title, message).await;
                }
                tokio::time::sleep(DRAIN_INTERVAL).await;
            }
        });
    }

    /// One "significantly delayed" notification per trigger whose oldest
    /// queued fire has waited [`DELAY_ALERT_WAIT`] or longer, subject to the
    /// per-trigger cooldown. A paused trigger's fires wait for resume on
    /// purpose, so it never alerts. Driven by the periodic drain loop.
    fn delay_notifications_due(
        &self,
        state: &mut QueueState,
        now: DateTime<Utc>,
    ) -> Vec<(String, String)> {
        let mut oldest: HashMap<String, (DateTime<Utc>, usize)> = HashMap::new();
        for e in &state.queued {
            if let Some(ref tid) = e.trigger_id {
                let slot = oldest.entry(tid.clone()).or_insert((e.queued_at, 0));
                slot.0 = slot.0.min(e.queued_at);
                slot.1 += 1;
            }
        }
        let mut due = Vec::new();
        for (tid, (oldest_at, count)) in oldest {
            let waited = (now - oldest_at).to_std().unwrap_or_default();
            if waited < DELAY_ALERT_WAIT
                || self.trigger_paused(&tid)
                || !state.delay_cooldown_elapsed(&tid)
            {
                continue;
            }
            let label = self.trigger_name(&tid).unwrap_or_else(|| tid.clone());
            due.push((
                format!("{label} is significantly delayed"),
                format!(
                    "Trigger \"{label}\"'s oldest fire has waited {} min in the Thread \
                     Queue ({count} waiting).",
                    waited.as_secs() / 60
                ),
            ));
        }
        due
    }

    // ---- Event emission helpers ----

    // An entry's three lifecycle frames state whose fire they belong to, taken
    // from the entry itself. None of them can read it off the ambient scope.
    // `ThreadQueued` runs before the fire, and `ThreadQueueCompleted` on the
    // sibling task that joins it. A sub-thread the fire submits is submitted
    // inline, so it would inherit a marker it must not have. See
    // `EventBus::emit_as_trigger`.
    //
    // `ThreadQueueDropped` passes `None` on purpose. A dropped entry never
    // fired, so it emitted nothing and there is no self-wake to suppress. The
    // trigger should still hear that a fire of its was coalesced away.

    async fn emit_queued(&self, entry: &QueueEntry, requeued: bool, actor: Option<MessageOrigin>) {
        if let Err(error) = self.try_emit_queued(entry, requeued, actor).await {
            log!("[ThreadQueue] Cannot persist queued entry: {}", error);
        }
    }

    async fn try_emit_queued(
        &self,
        entry: &QueueEntry,
        requeued: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let request_json = serde_json::to_value(&entry.request)?;
        let depth = entry.request.depth();
        self.at_chain_depth(
            depth,
            self.bus.emit_as_trigger(
                BusEvent::System(SystemEvent::ThreadQueued {
                    entry_id: entry.id,
                    kind: entry.kind,
                    trigger_id: entry.trigger_id.clone(),
                    trigger_name: entry.trigger_name.clone(),
                    thread_id: entry.thread_id,
                    summary: entry.summary.clone(),
                    request: request_json,
                    requeued,
                    actor,
                }),
                entry.trigger_id.clone(),
            ),
        )
        .await?;
        Ok(())
    }

    async fn emit_admitted(
        &self,
        entry_id: Uuid,
        thread_id: Option<Uuid>,
        trigger_id: Option<String>,
        depth: u32,
        actor: Option<MessageOrigin>,
    ) {
        self.at_chain_depth(
            depth,
            self.bus.emit_or_log_as_trigger(
                BusEvent::System(SystemEvent::ThreadQueueAdmitted {
                    entry_id,
                    thread_id,
                    actor,
                }),
                "[ThreadQueue] ThreadQueueAdmitted",
                trigger_id,
            ),
        )
        .await;
    }

    async fn emit_dropped(&self, entry_id: Uuid, reason: &str, actor: Option<MessageOrigin>) {
        self.bus
            .emit_or_log_as_trigger(
                BusEvent::System(SystemEvent::ThreadQueueDropped {
                    entry_id,
                    reason: reason.to_string(),
                    actor,
                }),
                "[ThreadQueue] ThreadQueueDropped",
                None,
            )
            .await;
    }

    /// Persist the notice before removing the entry. A deterministic notice
    /// id makes a crash between those writes safe to replay at the next boot.
    async fn drop_interrupted_entry(
        &self,
        entry_id: Uuid,
        trigger_id: &str,
        cut_thread: Option<Uuid>,
        reason: &str,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};
        use crate::scheduler::user_tasks::{emit_trigger_notification, TriggerNotification};

        let notice_id = Uuid::new_v5(&entry_id, reason.as_bytes());
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM events WHERE event_type = 'NotificationCreated' \
             AND payload->'data'->>'id' = $1)",
        )
        .bind(notice_id.to_string())
        .fetch_one(&self.pool)
        .await?;
        if !exists {
            let configured_name = self
                .trigger_configs
                .read()
                .unwrap()
                .get(trigger_id)
                .map(|config| config.name.clone());
            // The registry is still empty during queue recovery. Its stored
            // queue label survives deleted triggers and that boot ordering.
            let name = match configured_name {
                Some(name) => name,
                None => sqlx::query_scalar::<_, Option<String>>(
                    "SELECT trigger_name FROM thread_queue WHERE id = $1",
                )
                .bind(entry_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten()
                .unwrap_or_else(|| "Trigger".into()),
            };
            let notice = TriggerNotification::with_id(
                notice_id,
                &format!("{name}: run interrupted"),
                message,
            );
            let tap = cut_thread.map_or(Tap::Modal, |thread| Tap::Navigate {
                to: Box::new(NavigateUi {
                    target: NavigateTarget::Thread,
                    id: Some(thread.to_string()),
                    ..Default::default()
                }),
            });
            emit_trigger_notification(&self.bus, trigger_id, &notice, cut_thread, tap).await?;
            if let Some(engine) = self.engine.get().and_then(Weak::upgrade) {
                notice.send_push(&engine);
            }
        }
        self.bus
            .emit(BusEvent::System(SystemEvent::ThreadQueueDropped {
                entry_id,
                reason: reason.into(),
                actor: None,
            }))
            .await?;
        Ok(())
    }

    /// Inbox notification + push fan-out for queue-health events. Push is
    /// best-effort: in test fixtures no engine is attached, and the inbox
    /// row (emitted through the bus) is the durable record either way.
    async fn notify(&self, title: String, message: String) {
        let id = Uuid::new_v4();
        // Every Thread Queue notification is about queue state. The inbox row
        // and the push both open the Thread Queue panel, where the backlog is.
        let tap = crate::scheduler::notifications::Tap::Navigate {
            to: Box::new(crate::scheduler::notifications::NavigateUi {
                target: crate::scheduler::notifications::NavigateTarget::ThreadQueue,
                ..Default::default()
            }),
        };
        self.bus
            .emit_or_log(
                BusEvent::System(SystemEvent::NotificationCreated {
                    id: id.to_string(),
                    title: title.clone(),
                    message: message.clone(),
                    task_id: None,
                    app_id: None,
                    thread_id: None,
                    event_id: None,
                    tap: tap.clone(),
                    actor: None,
                }),
                "[ThreadQueue] NotificationCreated",
            )
            .await;
        if let Some(engine) = self.engine.get().and_then(Weak::upgrade) {
            crate::scheduler::push::send_push_to_all_with_app(
                &engine,
                &title,
                &message,
                Some(id),
                None,
                None,
                None,
                tap,
            );
        }
    }
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod manager_tests;
