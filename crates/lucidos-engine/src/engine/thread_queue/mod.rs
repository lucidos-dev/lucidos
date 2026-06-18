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
//!   (back-pressure at pool-max) and counts the slot. From there the slot's
//!   lifetime is owned by [`ThreadQueue::reconcile_user_slot`], driven by the
//!   settle subscriber off `thread_summaries.status` — the SINGLE place the
//!   user-half of the pool moves in and out, so it can never drift from real
//!   thread status (a thread that parks on a question, resumes, is continued,
//!   or auto-resumes after restart all converge correctly). These slots live
//!   in-memory only — ephemeral runtime (a dead response is gone on restart,
//!   never re-fired), so they are NOT persisted; the panel API merges them in
//!   and a transient `ThreadQueueChanged` refreshes the panel when only user
//!   state moves.
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
};
pub use request::ThreadQueueRequest;
pub(crate) use request::truncate_summary;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{MessageOrigin, ThreadEvent};
use crate::engine::LucidosEngine;
use crate::triggers::TriggerConfig;

/// Backlog size at which a per-trigger "significantly delayed" notification fires.
const BACKLOG_COUNT_THRESHOLD: usize = 10;
/// Oldest-waiting age at which a per-trigger "significantly delayed" notification fires.
const BACKLOG_AGE: Duration = Duration::from_secs(5 * 60);
/// Minimum spacing between notifications for the same trigger (and for the
/// global at-capacity notice) so a hot queue doesn't spam the inbox.
const NOTIFY_COOLDOWN: Duration = Duration::from_secs(10 * 60);
/// Safety-net drain interval — normal drains are event-driven (completion,
/// drop, policy change, trigger resume); the timer catches anything missed
/// and drives the backlog-age notification check.
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
}

/// One admitted user-initiated response occupying the pool. In-memory only —
/// never persisted (a dead response is gone on restart, never re-fired).
struct UserSlot {
    /// The thread running the response, for the panel's Running list.
    thread_id: Option<Uuid>,
    summary: String,
    admitted_at: DateTime<Utc>,
}

/// A user-initiated response waiting for a free slot (pool at max). Drains
/// with priority — ahead of background, behind the reserved-background floor.
struct UserWaiter {
    /// Stable id for this waiter / its eventual [`UserSlot`].
    entry_id: Uuid,
    thread_id: Option<Uuid>,
    summary: String,
    queued_at: DateTime<Utc>,
    /// Resolved by the drainer once admitted, unblocking the chat task.
    wake: oneshot::Sender<()>,
}

struct QueueState {
    policy: CapacityPolicy,
    queued: VecDeque<QueueEntry>,
    active: HashMap<Uuid, ActiveSlot>,
    /// Admitted user-initiated responses (in-memory only — see [`UserSlot`]).
    user_active: HashMap<Uuid, UserSlot>,
    /// User-initiated responses waiting for a slot (priority line, FIFO).
    user_queued: VecDeque<UserWaiter>,
    /// Per-trigger notification cooldowns (allowed-ephemeral — a restart
    /// resetting the cooldown at worst re-notifies once).
    backlog_notified: HashMap<String, Instant>,
    global_notified: Option<Instant>,
}

impl QueueState {
    /// Everything occupying the shared pool — background admits + user admits.
    fn total_active(&self) -> usize {
        self.active.len() + self.user_active.len()
    }

    fn counts_for(&self, kind: ThreadQueueKind, trigger_id: Option<&str>) -> AdmissionCounts {
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
            user_active: self.user_active.len(),
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
    /// Resolves when the entry's work finishes OR the entry is dropped.
    pub completion: oneshot::Receiver<()>,
}

/// RAII handle for a user-initiated pool slot from
/// [`ThreadQueue::acquire_user_slot`]. Releasing on drop (even on panic)
/// frees the slot and drains, so a crashed chat task can't leak capacity.
pub struct UserSlotGuard {
    queue: Arc<ThreadQueue>,
    entry_id: Uuid,
}

impl Drop for UserSlotGuard {
    fn drop(&mut self) {
        // release_user_slot is async; the drop runs in a sync context, so hand
        // the cleanup to a detached task. The pool count is in-memory, so a
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
    pub thread_id: Option<Uuid>,
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

/// Thread events on which the settle subscriber reconciles the user pool against
/// the (now-committed) `thread_summaries.status`, so the pool stays a faithful
/// mirror of real thread status — the SINGLE place the user-half of the pool
/// moves in and out. Reconcile reads the actual post-commit status, so it's
/// *direction-agnostic*; this predicate only needs to fire on the status
/// transitions that matter.
///
/// Two deliberate groups, and one deliberate exclusion:
///
/// - **→ running WITHOUT a back-pressure gate** — resume after a park,
///   continuation respawn, post-restart auto-resume, and engine-injected
///   prompts (hardening / conflict recovery). reconcile is the *sole adder*
///   for these, so they MUST be here — this is what fixes the reported bug and
///   its whole family.
/// - **→ not-running** — parked on the user, terminal, or back to idle.
///   reconcile removes; removal is idempotent so these can never double-count.
/// - **Excluded: gate-covered starts** (`MessageReceived`, `SessionStarted`,
///   `CodingAgentUserMessageSent`, `UserPromptInjected`). These are always
///   preceded by [`Self::acquire_user_slot`] inserting the slot (or are
///   mid-flight into an already-counted thread), so reconcile must NOT also add
///   on them — `acquire_user_slot` adds unconditionally, so a reconcile add
///   racing it would double-count. The gate owns the start; reconcile owns
///   everything after.
///
/// Per-token streaming and pure metadata/audit events never move status and are
/// excluded. A missed status variant degrades to "the pool lags status until
/// the next status event for that thread (or the gate guard's drop)", never a
/// hard break — mirror new status arms here.
fn affects_user_running(event: &ThreadEvent) -> bool {
    matches!(
        event,
        // → running with no gate — reconcile is the sole adder.
        ThreadEvent::ContinuationStarted { .. }
            | ThreadEvent::ContinuationRequested { .. }
            | ThreadEvent::UserQuestionAnswered { .. }
            | ThreadEvent::CodingAgentPermissionResolved { .. }
            | ThreadEvent::CommandPermissionResolved { .. }
            | ThreadEvent::CodingAgentPromptSent { .. }
            // → waiting_for_user_answer (parked on the user).
            | ThreadEvent::UserQuestionAsked { .. }
            | ThreadEvent::CodingAgentPermissionRequest { .. }
            | ThreadEvent::CommandPermissionRequested { .. }
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
    /// Installed via [`Self::attach_engine`] after `Arc::new(engine)` —
    /// executes admitted entries. Test fixtures install mocks.
    executor: OnceLock<Arc<dyn ThreadQueueExecutor>>,
    /// Weak engine handle, used only for push fan-out on queue notifications.
    engine: OnceLock<Weak<LucidosEngine>>,
    state: tokio::sync::Mutex<QueueState>,
}

impl ThreadQueue {
    pub fn new(
        pool: PgPool,
        bus: EventBus,
        trigger_configs: Arc<std::sync::RwLock<HashMap<String, TriggerConfig>>>,
        policy: CapacityPolicy,
    ) -> Self {
        Self {
            pool,
            bus,
            trigger_configs,
            executor: OnceLock::new(),
            engine: OnceLock::new(),
            state: tokio::sync::Mutex::new(QueueState {
                policy,
                queued: VecDeque::new(),
                active: HashMap::new(),
                user_active: HashMap::new(),
                user_queued: VecDeque::new(),
                backlog_notified: HashMap::new(),
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
            log!("[ThreadQueue] load_policy query failed: {} — using defaults", e);
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

    /// Submit a background spawn. Admits immediately when capacity allows
    /// (the executor's `prepare` hook runs inline, before this returns);
    /// otherwise the entry waits in the queue. Never blocks on execution.
    pub async fn submit(
        self: &Arc<Self>,
        request: ThreadQueueRequest,
        actor: Option<MessageOrigin>,
        cancel: Option<CancellationToken>,
    ) -> SubmitOutcome {
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

        let decision = {
            let mut state = self.state.lock().await;
            let counts = state.counts_for(kind, trigger_id.as_deref());
            let decision = state
                .policy
                .decide_submit(counts, kind, trigger_id.is_some());
            match decision {
                AdmissionDecision::Admit => {
                    state.active.insert(
                        entry_id,
                        ActiveSlot {
                            kind,
                            trigger_id: trigger_id.clone(),
                            completion_tx: entry.completion_tx.take(),
                        },
                    );
                }
                AdmissionDecision::Queue | AdmissionDecision::Overflow => {}
            }
            decision
        };

        match decision {
            AdmissionDecision::Admit => {
                if let Some(executor) = self.executor.get() {
                    executor.prepare(&mut entry.request).await;
                }
                self.emit_queued(&entry, false, actor).await;
                self.emit_admitted(entry_id, entry.thread_id, None).await;
                self.spawn_execution(entry);
                SubmitOutcome {
                    entry_id,
                    admitted: true,
                    position: 0,
                    completion: completion_rx,
                }
            }
            AdmissionDecision::Queue => {
                self.emit_queued(&entry, false, actor).await;
                let (position, notify) = {
                    let mut state = self.state.lock().await;
                    state.queued.push_back(entry);
                    let position = state.queued.len();
                    let notify = self.backlog_notifications_due(&mut state, trigger_id.as_deref());
                    (position, notify)
                };
                for (title, message) in notify {
                    self.notify(title, message).await;
                }
                SubmitOutcome {
                    entry_id,
                    admitted: false,
                    position,
                    completion: completion_rx,
                }
            }
            AdmissionDecision::Overflow => {
                self.handle_overflow(entry, actor).await;
                SubmitOutcome {
                    entry_id,
                    admitted: false,
                    position: 0,
                    completion: completion_rx,
                }
            }
        }
    }

    /// Apply the overflow policy: the trigger's queue is at its ceiling.
    async fn handle_overflow(self: &Arc<Self>, entry: QueueEntry, actor: Option<MessageOrigin>) {
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
        self.emit_queued(&entry, false, actor).await;
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
                    let reason = format!(
                        "per-trigger queue cap ({cap}) reached — oldest entry dropped"
                    );
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
                // Pause through the bus — the scheduler's trigger subscriber
                // flips the in-memory config, exactly as a user pause would.
                self.bus
                    .emit_or_log(
                        BusEvent::System(SystemEvent::TriggerDisabled {
                            trigger_id: trigger_id.clone(),
                            payload: serde_json::json!({
                                "reason": "thread-queue overflow",
                            }),
                            actor: None,
                        }),
                        "[ThreadQueue] TriggerDisabled (overflow)",
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

    /// Collect any due backlog notifications (call under the state lock,
    /// send after releasing it). Count-based check at enqueue time; the
    /// age-based check lives in the periodic drain loop.
    fn backlog_notifications_due(
        &self,
        state: &mut QueueState,
        trigger_id: Option<&str>,
    ) -> Vec<(String, String)> {
        let mut due = Vec::new();
        if let Some(tid) = trigger_id {
            let backlog = state
                .queued
                .iter()
                .filter(|e| e.trigger_id.as_deref() == Some(tid))
                .count();
            if backlog >= BACKLOG_COUNT_THRESHOLD && self.cooldown_elapsed_for(state, tid) {
                let label = self.trigger_name(tid).unwrap_or_else(|| tid.to_string());
                due.push((
                    format!("{label} is significantly delayed"),
                    format!("Trigger \"{label}\" has {backlog} fires waiting in the Thread Queue."),
                ));
            }
        }
        let total_queued = state.queued.len() + state.user_queued.len();
        if state.total_active() >= state.policy.max_concurrent_total
            && state
                .global_notified
                .is_none_or(|t| t.elapsed() >= NOTIFY_COOLDOWN)
        {
            state.global_notified = Some(Instant::now());
            due.push((
                "Lucidos is at capacity".to_string(),
                format!(
                    "All {} slots are busy; {total_queued} thread(s) are waiting \
                     in the Thread Queue.",
                    state.policy.max_concurrent_total
                ),
            ));
        }
        due
    }

    fn cooldown_elapsed_for(&self, state: &mut QueueState, trigger_id: &str) -> bool {
        let elapsed = state
            .backlog_notified
            .get(trigger_id)
            .is_none_or(|t| t.elapsed() >= NOTIFY_COOLDOWN);
        if elapsed {
            state
                .backlog_notified
                .insert(trigger_id.to_string(), Instant::now());
        }
        elapsed
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
            let mut to_admit: Vec<QueueEntry> = Vec::new();
            let mut to_drop: Vec<(QueueEntry, String)> = Vec::new();

            // Phase 1 — background reclaims up to its reserved floor first.
            let floor = state.policy.effective_reserved_background();
            self.drain_background_into(&mut state, floor, &mut to_admit, &mut to_drop);
            // Phase 2 — user-initiated waiters take priority for free slots.
            let woke = Self::drain_users(&mut state);
            // Phase 3 — background fills whatever capacity remains.
            self.drain_background_into(&mut state, usize::MAX, &mut to_admit, &mut to_drop);

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
            if let Some(executor) = self.executor.get() {
                executor.prepare(&mut entry.request).await;
            }
            self.emit_admitted(entry.id, entry.thread_id, None).await;
            self.spawn_execution(entry);
        }
    }

    /// Background admission scan (drain phases 1 & 3). Admits queued background
    /// entries oldest-first — respecting per-trigger pause/deletion + FIFO and
    /// the capacity caps — until background occupies `bg_target` slots or the
    /// backlog is exhausted. Admitted entries land in `to_admit`; entries whose
    /// trigger has vanished land in `to_drop`.
    fn drain_background_into(
        &self,
        state: &mut QueueState,
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
            if let Some(ref tid) = trigger_id {
                if skipped.contains(tid) {
                    i += 1;
                    continue;
                }
                let config = self
                    .trigger_configs
                    .read()
                    .ok()
                    .and_then(|c| c.get(tid).map(|c| c.paused));
                match config {
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
            let counts = state.counts_for(kind, trigger_id.as_deref());
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
    fn drain_users(state: &mut QueueState) -> Vec<oneshot::Sender<()>> {
        let mut woke = Vec::new();
        while state.policy.user_can_admit(state.total_active()) {
            let Some(waiter) = state.user_queued.pop_front() else {
                break;
            };
            state.user_active.insert(
                waiter.entry_id,
                UserSlot {
                    thread_id: waiter.thread_id,
                    summary: waiter.summary,
                    admitted_at: Utc::now(),
                },
            );
            woke.push(waiter.wake);
        }
        woke
    }

    /// Force-admit a queued entry, ignoring every cap. User intent ("Run
    /// now" in the Thread Queue panel) — `actor` stamps the admission.
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
                },
            );
            entry
        };
        if let Some(executor) = self.executor.get() {
            executor.prepare(&mut entry.request).await;
        }
        self.emit_admitted(entry.id, entry.thread_id, actor).await;
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
        let slot = {
            let mut state = self.state.lock().await;
            state.active.remove(&entry_id)
        };
        let Some(mut slot) = slot else {
            return; // double-complete or unknown id — nothing to release
        };
        if let Some(tx) = slot.completion_tx.take() {
            let _ = tx.send(());
        }
        self.bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ThreadQueueCompleted { entry_id }),
                "[ThreadQueue] ThreadQueueCompleted",
            )
            .await;
        self.drain().await;
    }

    // ---- User-initiated work (preempting, prioritized, counted) ----

    /// Reserve a pool slot for a NEW user-initiated response (chat / user-typed
    /// coding-agent thread). This is the **back-pressure gate**: user work is
    /// **prioritized but not exempt** — it admits immediately when the pool has
    /// a free slot, otherwise it joins the priority line and this call *awaits*
    /// until a slot frees, so a person briefly waits only at true pool-max
    /// (ADR 0008). In-memory only — never persisted.
    ///
    /// The gate seeds the slot; from there [`Self::reconcile_user_slot`] (driven
    /// by the settle subscriber off `thread_summaries.status`) owns the slot's
    /// lifetime — it removes it when the thread parks/idles/terminates and
    /// re-adds it on resume — so the panel always mirrors real thread status.
    /// The returned guard's drop is the BACKSTOP: it releases the gate's
    /// reservation if the task dies before reconcile cleared it (normally a
    /// no-op, since the terminal status event already reconciled the slot away).
    pub async fn acquire_user_slot(
        self: &Arc<Self>,
        thread_id: Option<Uuid>,
        summary: String,
    ) -> UserSlotGuard {
        let entry_id = Uuid::new_v4();
        let wait = {
            let mut state = self.state.lock().await;
            if state.policy.user_can_admit(state.total_active()) {
                state.user_active.insert(
                    entry_id,
                    UserSlot {
                        thread_id,
                        summary,
                        admitted_at: Utc::now(),
                    },
                );
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
        // Panel: a new running (admitted) or waiting (queued) user entry.
        self.emit_changed().await;
        if let Some(rx) = wait {
            // Block until the drainer admits us. A dropped sender (engine
            // teardown) resolves Err — proceed; the slot dies with the process.
            let _ = rx.await;
        }
        UserSlotGuard {
            queue: self.clone(),
            entry_id,
        }
    }

    /// Backstop release of the gate's reserved slot, keyed by the guard's
    /// `entry_id` — fires from [`UserSlotGuard`]'s drop when the chat task ends.
    /// Normally a no-op: [`Self::reconcile_user_slot`] already removed the slot
    /// (by thread id) on the terminal status event. It still matters when the
    /// task dies without a terminal status event (a reserved-but-never-started
    /// thread, or broadcast lag), or to clear a still-**queued** waiter whose
    /// task gave up before admission. Refreshes the panel and drains so the
    /// freed slot admits waiting work.
    async fn release_user_slot(self: &Arc<Self>, entry_id: Uuid) {
        let removed = {
            let mut state = self.state.lock().await;
            if state.user_active.remove(&entry_id).is_some() {
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

    /// Snapshot of in-memory user-initiated occupants for the panel API to
    /// merge with the persisted background rows (admitted = Running, queued =
    /// Queued).
    pub async fn user_entries(&self) -> Vec<UserQueueEntry> {
        let state = self.state.lock().await;
        let mut out = Vec::with_capacity(state.user_active.len() + state.user_queued.len());
        for (id, slot) in &state.user_active {
            out.push(UserQueueEntry {
                id: *id,
                thread_id: slot.thread_id,
                summary: slot.summary.clone(),
                status: "admitted",
                queued_at: slot.admitted_at,
                admitted_at: Some(slot.admitted_at),
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
    /// `sequence`) followed by the in-memory user-initiated occupants
    /// (`kind: "user-chat"`) — plus the active capacity policy. The SINGLE
    /// source of truth shared by `GET /api/v1/thread-queue` and the
    /// `list_thread_queue` LLM tool, so the panel and the tool can never
    /// disagree about who occupies the pool (the divergence that let the tool
    /// report an empty pool while the panel showed phantom user-chat rows).
    pub async fn snapshot(&self) -> Result<ThreadQueueSnapshot, sqlx::Error> {
        let mut entries: Vec<ThreadQueueEntryView> = sqlx::query_as(
            "SELECT id, kind, trigger_id, trigger_name, thread_id, summary, status, queued_at, admitted_at \
             FROM thread_queue ORDER BY sequence",
        )
        .fetch_all(&self.pool)
        .await?;
        // Merge the in-memory user-initiated occupants (never persisted rows).
        for u in self.user_entries().await {
            entries.push(ThreadQueueEntryView {
                id: u.id,
                kind: "user-chat".to_string(),
                trigger_id: None,
                trigger_name: None,
                thread_id: u.thread_id,
                summary: u.summary,
                status: u.status.to_string(),
                queued_at: u.queued_at,
                admitted_at: u.admitted_at,
            });
        }
        let policy = self.policy().await;
        Ok(ThreadQueueSnapshot { entries, policy })
    }

    /// Converge the in-memory user-initiated pool for `thread_id` onto its
    /// authoritative `thread_summaries.status` — the SINGLE place the user-half
    /// of the pool moves in and out, so it can never drift from reality:
    ///
    /// - A user-initiated thread that is `running` occupies **exactly one**
    ///   user slot (added here if missing).
    /// - Anything else — idle, parked on the user (`waiting_for_user_answer`),
    ///   failed, terminal, or any non-user / background thread — occupies
    ///   **none** (removed here if present).
    ///
    /// Because it reads the real, just-committed status (the subscriber observes
    /// events post-`tx.commit()`, post-projection) it is *direction-agnostic*:
    /// it doesn't matter whether the triggering event was a park, a resume, or a
    /// termination — reconcile reads where the thread actually landed and makes
    /// the pool match. That is what fixes the whole family in one stroke: a
    /// thread that parks on a question then resumes, a continuation respawn, and
    /// a post-restart auto-resume all flip `status` back to `running`, so the
    /// slot reappears with no per-path re-acquire wiring.
    ///
    /// Adding here is unconditional (no capacity wait): a thread that's already
    /// `running` cannot be made to wait — back-pressure applies only to NEW work
    /// via [`Self::acquire_user_slot`], which still gates the start. For NEW
    /// work the gate has already inserted the slot, so the first reconcile is a
    /// no-op. Idempotent; a cheap PK lookup that no-ops for every background
    /// thread (`initiator != 'user'`) and every already-consistent thread.
    pub async fn reconcile_user_slot(self: &Arc<Self>, thread_id: Uuid) {
        let row: Option<(String, String, String)> = match sqlx::query_as(
            "SELECT status, initiator, \
                    COALESCE(NULLIF(title, ''), NULLIF(first_message, ''), '') \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(row) => row,
            // Leave the pool UNCHANGED on a transient query error — don't read a
            // failed status read as "not running" and yank a live thread's slot
            // (that would drop it from the panel until the next status event).
            // The next status event for the thread reconciles it correctly.
            Err(e) => {
                log!(
                    "[ThreadQueue] reconcile_user_slot query failed for {}: {} — leaving pool unchanged",
                    thread_id,
                    e
                );
                return;
            }
        };
        // Only user-initiated threads occupy a user slot; background spawns
        // (initiator 'system'/'unknown') are tracked via the `thread_queue`
        // projection, not here.
        let should_occupy = matches!(
            row.as_ref(),
            Some((status, initiator, _)) if status == "running" && initiator == "user"
        );

        let (added, removed) = {
            let mut state = self.state.lock().await;
            let has_slot = state
                .user_active
                .values()
                .any(|s| s.thread_id == Some(thread_id));
            match (should_occupy, has_slot) {
                (true, false) => {
                    let summary = row
                        .as_ref()
                        .map(|(_, _, s)| truncate_summary(s.trim()))
                        .unwrap_or_default();
                    state.user_active.insert(
                        Uuid::new_v4(),
                        UserSlot {
                            thread_id: Some(thread_id),
                            summary,
                            admitted_at: Utc::now(),
                        },
                    );
                    (true, false)
                }
                (false, true) => {
                    state
                        .user_active
                        .retain(|_, s| s.thread_id != Some(thread_id));
                    (false, true)
                }
                // Already consistent (running with a slot, or not-running with
                // none) — nothing to do.
                _ => (false, false),
            }
        };
        if added || removed {
            self.emit_changed().await;
        }
        // Removal frees a slot — drain so waiting work takes it. An add only
        // adds load (the thread is already running), so it needs no drain.
        if removed {
            self.drain().await;
        }
    }

    /// Subscribe to the bus and keep the user-half of the pool in lockstep with
    /// each thread's `thread_summaries.status` (see [`Self::reconcile_user_slot`]).
    /// Spawned once at boot. Survives broadcast lag — a dropped event at worst
    /// leaves a slot stale until the next status event for that thread (or the
    /// gate guard's drop), so the consumer must never exit on `Lagged`.
    pub fn spawn_settle_subscriber(self: &Arc<Self>) {
        let mgr = self.clone();
        let mut rx = self.bus.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(emitted) => {
                        // Only persisted thread events have a committed status to
                        // read; transient (seq == None) events never change it.
                        if emitted.seq.is_none() {
                            continue;
                        }
                        if let BusEvent::Thread {
                            thread_id, event, ..
                        } = &emitted.typed
                        {
                            if affects_user_running(event) {
                                mgr.reconcile_user_slot(*thread_id).await;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        log!(
                            "[ThreadQueue] settle subscriber lagged by {} events — \
                             user-slot reconcile skipped for those; continuing",
                            n
                        );
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        log!("[ThreadQueue] settle subscriber channel closed — exiting");
                        break;
                    }
                }
            }
        });
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

    /// Spawn an admitted entry's work and complete the slot when it resolves
    /// — even on panic (the JoinHandle surfaces it), so a crashed executor
    /// can never leak a capacity slot.
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
        let executable = ExecutableEntry {
            id: entry.id,
            request: entry.request,
            cancel: entry.cancel,
        };
        let mgr = self.clone();
        let work = tokio::spawn(async move { executor.execute(executable).await });
        tokio::spawn(async move {
            if let Err(join_err) = work.await {
                if join_err.is_panic() {
                    log!(
                        "[ThreadQueue] Entry {} execution panicked: {:?}",
                        entry_id,
                        join_err
                    );
                }
            }
            mgr.complete(entry_id).await;
        });
    }

    // ---- Startup recovery ----

    /// Rebuild in-memory state from the `thread_queue` projection. Called at
    /// boot BEFORE any submission path is live (scheduler not started), so
    /// per-trigger FIFO holds across the restart:
    ///
    /// - `queued` rows load back into the in-memory queue (no re-emit).
    /// - `admitted` rows are work that died with the previous process:
    ///   trigger kinds re-queue (re-fire semantics, same as missed-cron
    ///   catch-up); spawn kinds whose thread already materialized complete
    ///   instead — the thread-level recovery (CC auto-resume / chat settle)
    ///   owns them from here.
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
        }
        let rows: Vec<Row> = match sqlx::query_as(
            "SELECT id, kind, trigger_id, trigger_name, thread_id, summary, request, status, queued_at \
             FROM thread_queue ORDER BY sequence",
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                log!("[ThreadQueue] recover_persisted_entries query failed: {}", e);
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

            let requeue = if row.status == "queued" {
                false // already queued — load silently, keep original queued_at
            } else {
                match kind {
                    ThreadQueueKind::EventTrigger | ThreadQueueKind::Cron => true,
                    ThreadQueueKind::SubThread | ThreadQueueKind::CodingAgent => {
                        let thread_exists = match row.thread_id {
                            Some(tid) => sqlx::query_scalar::<_, bool>(
                                "SELECT EXISTS(SELECT 1 FROM thread_summaries WHERE thread_id = $1)",
                            )
                            .bind(tid)
                            .fetch_one(&self.pool)
                            .await
                            .unwrap_or(false),
                            None => false,
                        };
                        if thread_exists {
                            // The spawn happened — thread-level recovery owns it.
                            log!(
                                "[ThreadQueue] Entry {} ({}) already materialized thread {:?} — handing off to thread recovery",
                                row.id,
                                kind.as_str(),
                                row.thread_id
                            );
                            self.bus
                                .emit_or_log(
                                    BusEvent::System(SystemEvent::ThreadQueueCompleted {
                                        entry_id: row.id,
                                    }),
                                    "[ThreadQueue] ThreadQueueCompleted (boot handoff)",
                                )
                                .await;
                            continue;
                        }
                        true
                    }
                }
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
                self.emit_queued(&entry, true, None).await;
            }
            self.state.lock().await.queued.push_back(entry);
        }
    }

    /// Kick off the drain loop: an immediate drain (queued backlog from the
    /// previous process), then the periodic safety-net drain + backlog-age
    /// notification check. Call AFTER the scheduler has replayed trigger
    /// configs — drain consults them for pause/deletion.
    pub fn start_draining(self: &Arc<Self>) {
        let mgr = self.clone();
        tokio::spawn(async move {
            loop {
                mgr.drain().await;
                let due = {
                    let mut state = mgr.state.lock().await;
                    mgr.age_notifications_due(&mut state)
                };
                for (title, message) in due {
                    mgr.notify(title, message).await;
                }
                tokio::time::sleep(DRAIN_INTERVAL).await;
            }
        });
    }

    /// Per-trigger "oldest waiting too long" check, driven by the periodic
    /// drain loop.
    fn age_notifications_due(&self, state: &mut QueueState) -> Vec<(String, String)> {
        let mut oldest: HashMap<String, (DateTime<Utc>, usize)> = HashMap::new();
        for e in &state.queued {
            if let Some(ref tid) = e.trigger_id {
                let slot = oldest.entry(tid.clone()).or_insert((e.queued_at, 0));
                slot.0 = slot.0.min(e.queued_at);
                slot.1 += 1;
            }
        }
        let now = Utc::now();
        let mut due = Vec::new();
        for (tid, (oldest_at, count)) in oldest {
            let age = (now - oldest_at).to_std().unwrap_or_default();
            if age >= BACKLOG_AGE && self.cooldown_elapsed_for(state, &tid) {
                let label = self.trigger_name(&tid).unwrap_or_else(|| tid.clone());
                due.push((
                    format!("{label} is significantly delayed"),
                    format!(
                        "Trigger \"{label}\" has {count} fire(s) waiting in the Thread Queue; \
                         the oldest has waited {} min.",
                        age.as_secs() / 60
                    ),
                ));
            }
        }
        due
    }

    // ---- Event emission helpers ----

    async fn emit_queued(&self, entry: &QueueEntry, requeued: bool, actor: Option<MessageOrigin>) {
        let request_json = match serde_json::to_value(&entry.request) {
            Ok(v) => v,
            Err(e) => {
                log!("[ThreadQueue] request serialization failed: {}", e);
                return;
            }
        };
        self.bus
            .emit_or_log(
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
                "[ThreadQueue] ThreadQueued",
            )
            .await;
    }

    async fn emit_admitted(
        &self,
        entry_id: Uuid,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) {
        self.bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ThreadQueueAdmitted {
                    entry_id,
                    thread_id,
                    actor,
                }),
                "[ThreadQueue] ThreadQueueAdmitted",
            )
            .await;
    }

    async fn emit_dropped(&self, entry_id: Uuid, reason: &str, actor: Option<MessageOrigin>) {
        self.bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ThreadQueueDropped {
                    entry_id,
                    reason: reason.to_string(),
                    actor,
                }),
                "[ThreadQueue] ThreadQueueDropped",
            )
            .await;
    }

    /// Inbox notification + push fan-out for queue-health events. Push is
    /// best-effort: in test fixtures no engine is attached, and the inbox
    /// row (emitted through the bus) is the durable record either way.
    async fn notify(&self, title: String, message: String) {
        let id = Uuid::new_v4();
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
                    // Every Thread Queue notification (backlog, at-capacity,
                    // overflow-pause) is about queue state — tap lands on the
                    // Thread Queue panel so the user sees the backlog directly.
                    tap: crate::scheduler::notifications::Tap::Navigate {
                        to: crate::scheduler::notifications::NavigateUi {
                            target: crate::scheduler::notifications::NavigateTarget::ThreadQueue,
                            ..Default::default()
                        },
                    },
                    actor: None,
                }),
                "[ThreadQueue] NotificationCreated",
            )
            .await;
        if let Some(engine) = self.engine.get().and_then(Weak::upgrade) {
            crate::scheduler::push::send_push_to_all(&engine, &title, &message, Some(id)).await;
        }
    }
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod manager_tests;
