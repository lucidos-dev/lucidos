//! The *compactor*: the background job that writes summary tree nodes.
//!
//! **Work is a queue of scopes.** A scope is *dirty* when its log may hold
//! entries no node reflects. A worker *drains* one scope at a time: it reads
//! the log, realigns the stored nodes with it, and builds every node it can.
//! Live events go to the front of the queue and the backfill to the back. The
//! backfill takes the workspace first, then threads by last activity.
//!
//! **Model calls run in lanes** ([`super::limit`]): each provider route allows as
//! many calls at once as it has shown it can take. Live calls go first.
//!
//! **A build is a function of the log and the answers.** A drain applies
//! finished nodes in the order it launched them. So what a node reads never
//! depends on which call returned first, or on which scope was queued first.
//!
//! **Progress is durable (I8).** A node is stored the moment it is built, and
//! a drained scope records the newest event it reflects. A restart re-reads
//! that and resumes where the last process stopped.
//!
//! **A failure never wedges the queue.** A failed node puts its scope back
//! after [`RETRY`] and frees the worker for another scope. Only the first
//! failure of each node is logged.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::stream::{FuturesOrdered, StreamExt};
use sqlx::PgPool;
use tokio::sync::{Notify, Semaphore};
use uuid::Uuid;

use super::fold;
use super::limit::{Lanes, MAX_LIMIT};
use super::log::{self, EntryKind, LogEntry};
use super::shape::LeafOrder;
use super::store::{self, StoredNode};
use super::workspace_log::{WorkspaceLeaf, WorkspaceLog};
use super::{prompt, shape, NodeAddr, SummaryScope, CONTEXT_BYTES, HORIZON_SECS, LIVE_DRAINS};
use super::{BackfillProgress, NODE_BYTES, READS, READY_DAYS, RETRY, TRIES};
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::{AuxCapture, ContextPurpose};
use crate::llm::provider::{LlmProvider, Message, MessageContent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// What the compactor needs from the engine. A trait so the tests can run it
/// on a scripted model and a fixed set of artifacts.
#[async_trait::async_trait]
pub(crate) trait CompactorDeps: Send + Sync {
    /// The model one node is written with. Resolved per node, so a changed
    /// preference applies to the next one. `Err` when none is configured.
    async fn model(&self) -> Result<CompactionModel, String>;

    /// An artifact's text at a commit, or `None`, logged, when it cannot be
    /// read as text.
    fn read_artifact(&self, path: &str, commit: &str) -> Option<String>;
}

pub(crate) struct CompactionModel {
    /// Routes `model` to whichever configured backend serves it.
    pub(crate) provider: Arc<dyn LlmProvider>,
    /// The model every call names, so the capture records what actually ran.
    pub(crate) model: String,
    pub(crate) effort: Option<String>,
    /// Covers every round of one node.
    pub(crate) deadline: Duration,
    /// The provider route the calls take, which names their lane.
    pub(crate) lane: String,
}

/// How one drain ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drained {
    /// Every node the log allows is built.
    Complete,
    /// The compactor paused mid-drain; resume re-seeds the scope.
    Paused,
    /// A node failed; the scope goes back after [`RETRY`].
    Failed,
    /// No model is configured, so nothing that needs one can be built.
    ModelUnavailable,
}

enum NodeError {
    ModelUnavailable(String),
    Failed(String),
}

impl From<BoxError> for NodeError {
    fn from(e: BoxError) -> Self {
        Self::Failed(e.to_string())
    }
}

impl From<sqlx::Error> for NodeError {
    fn from(e: sqlx::Error) -> Self {
        Self::Failed(e.to_string())
    }
}

/// What a leaf compresses.
struct LeafInput {
    message: String,
    context: Vec<String>,
    source_event_id: Uuid,
    source_thread_id: Option<Uuid>,
}

enum NodeInput {
    Leaf(LeafInput),
    Merge {
        a: String,
        b: String,
        context: Vec<String>,
    },
}

#[derive(Default)]
struct Queue {
    order: VecDeque<SummaryScope>,
    queued: HashSet<SummaryScope>,
    /// Queued scopes a live event marked. They sit at the front of `order`.
    urgent: HashSet<SummaryScope>,
    /// Scopes being drained, and whether one was marked dirty meanwhile.
    draining: HashMap<SummaryScope, bool>,
    backfill: Option<Backfill>,
}

/// A running backfill.
struct Backfill {
    /// The scopes it still owes.
    owed: HashSet<SummaryScope>,
    /// The owed scopes the ready flag waits on: the workspace and every
    /// thread active within [`READY_DAYS`]. Empty once the flag is set.
    gate: HashSet<SummaryScope>,
    /// Every scope a tree exists for, owed or not, so a restart halfway
    /// reports the same count rather than starting again at zero.
    total: usize,
    /// The ready flag is set, so what is still owed is older threads.
    ready: bool,
    /// The owed scopes that have started draining, with their node counts.
    nodes: HashMap<SummaryScope, NodeCount>,
    /// The owed scopes whose last drain failed, until one completes.
    failing: HashSet<SummaryScope>,
}

/// One owed scope's nodes: built so far, and needed in all.
#[derive(Clone, Copy, Debug, Default)]
struct NodeCount {
    done: u64,
    total: u64,
}

struct WorkspaceState {
    log: WorkspaceLog,
    nodes: HashMap<NodeAddr, StoredNode>,
}

pub(crate) struct Compactor {
    pool: PgPool,
    bus: EventBus,
    deps: Arc<dyn CompactorDeps>,
    lanes: Lanes,
    reads: Semaphore,
    queue: Mutex<Queue>,
    wake: Notify,
    /// The workspace log and nodes, kept between drains. Only the drain of
    /// the workspace scope touches it, and two drains of one scope never run
    /// at once.
    workspace: tokio::sync::Mutex<Option<WorkspaceState>>,
    /// Set when the cache may disagree with the database, so the next
    /// workspace drain reloads it whole.
    workspace_stale: AtomicBool,
    /// Threads deleted since the last workspace drain, whose leaves it drops.
    deleted_threads: Mutex<HashSet<Uuid>>,
    reported: Mutex<HashSet<(SummaryScope, NodeAddr)>>,
    /// Set while the workspace is off the Tree module. Workers take no scope,
    /// and a running drain launches no further node, so only the calls
    /// already in flight finish after a pause.
    paused: AtomicBool,
    resumed: Notify,
    /// Set when a drain ends for want of a model, cleared by the next model
    /// call that answers.
    waiting_for_model: AtomicBool,
    /// Held from reading the backfill count to emitting it, so two workers
    /// never announce counts out of order.
    announcing: tokio::sync::Mutex<()>,
}

impl Compactor {
    pub(crate) fn new(pool: PgPool, bus: EventBus, deps: Arc<dyn CompactorDeps>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            bus,
            deps,
            lanes: Lanes::default(),
            reads: Semaphore::new(READS),
            queue: Mutex::default(),
            wake: Notify::new(),
            workspace: tokio::sync::Mutex::new(None),
            workspace_stale: AtomicBool::new(false),
            deleted_threads: Mutex::default(),
            reported: Mutex::default(),
            paused: AtomicBool::new(false),
            resumed: Notify::new(),
            waiting_for_model: AtomicBool::new(false),
            announcing: tokio::sync::Mutex::new(()),
        })
    }

    /// The running backfill's count, or `None` when none runs.
    pub(crate) fn backfill_progress(&self) -> Option<BackfillProgress> {
        let q = self.queue.lock().unwrap();
        let backfill = q.backfill.as_ref()?;
        let done = backfill.total - backfill.owed.len();
        let (nodes_done, nodes_total) = backfill
            .nodes
            .values()
            .fold((0, 0), |(done, total), n| (done + n.done, total + n.total));
        // Each scope under way adds its own share, so a scope starting adds
        // nothing and a scope finishing trades its full share for one `done`.
        let under_way: u64 = backfill
            .nodes
            .values()
            .filter(|n| n.total > 0)
            .map(|n| n.done.min(n.total) * 1000 / n.total)
            .sum();
        Some(BackfillProgress {
            done,
            total: backfill.total,
            done_milli: done as u64 * 1000 + under_way,
            nodes_done,
            nodes_total,
            waiting_for_model: self.waiting_for_model.load(Ordering::SeqCst),
            retrying: !backfill.failing.is_empty(),
            ready: backfill.ready,
        })
    }

    /// Start counting the nodes of `scope`, when the backfill owes it, and
    /// say whether it does. A scope draining again after a failure or a pause
    /// keeps what it built, so its count never goes back.
    fn count_nodes(&self, scope: SummaryScope, remaining: u64) -> bool {
        let mut q = self.queue.lock().unwrap();
        let Some(backfill) = q.backfill.as_mut().filter(|b| b.owed.contains(&scope)) else {
            return false;
        };
        let count = backfill.nodes.entry(scope).or_default();
        count.total = count.done + remaining;
        true
    }

    /// Count one built node of `scope`, and say whether the backfill counts
    /// that scope.
    fn node_built(&self, scope: SummaryScope) -> bool {
        let mut q = self.queue.lock().unwrap();
        let Some(count) = q.backfill.as_mut().and_then(|b| b.nodes.get_mut(&scope)) else {
            return false;
        };
        count.done += 1;
        true
    }

    /// Tell the UI where the backfill has got, when one runs. The frame that
    /// counts the last scope ends the backfill.
    async fn announce_progress(&self) {
        let _order = self.announcing.lock().await;
        let Some(progress) = self.backfill_progress() else {
            return;
        };
        self.bus
            .emit_or_log(
                BusEvent::System(SystemEvent::TreeBackfillProgressed { progress }),
                "[SummaryTree] TreeBackfillProgressed",
            )
            .await;
        let mut q = self.queue.lock().unwrap();
        // A re-seed during the emit installs a new backfill, which stays.
        if q.backfill
            .as_ref()
            .is_some_and(|b| b.ready && b.owed.is_empty())
        {
            log!("[SummaryTree] Backfill complete; every tree is built");
            q.backfill = None;
        }
    }

    /// Record whether the compactor waits on a model, and announce a change.
    async fn set_waiting_for_model(&self, waiting: bool) {
        if self.waiting_for_model.swap(waiting, Ordering::SeqCst) != waiting {
            self.announce_progress().await;
        }
    }

    /// Stop taking work. Progress is durable, so [`Self::resume`] picks up
    /// where this left off (I8).
    pub(crate) fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// Take work again, re-reading what changed while paused.
    pub(crate) async fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.resumed.notify_waiters();
        if let Err(e) = self.seed().await {
            log!("[SummaryTree] Could not re-seed the compactor: {}", e);
        }
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Seed the queue, then run the workers for the engine's lifetime: one per
    /// call a lane can hold, plus [`LIVE_DRAINS`] that take only live work.
    pub(crate) fn start(self: &Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(e) = this.seed().await {
                log!("[SummaryTree] Could not seed the compactor: {}", e);
            }
            let workers = (0..MAX_LIMIT)
                .map(|_| Take::Any)
                .chain((0..LIVE_DRAINS).map(|_| Take::UrgentOnly));
            for take in workers {
                let worker = this.clone();
                tokio::spawn(async move { worker.work(take).await });
            }
        });
    }

    /// Queue every scope with unreflected events: the workspace first, then
    /// threads by last activity, newest first. Until the workspace is ready,
    /// those scopes are the backfill. After, the threads never drained to the
    /// end still are.
    pub(crate) async fn seed(&self) -> Result<(), BoxError> {
        let swept = store::sweep_deleted_threads(&self.pool).await?;
        let threads = store::dirty_threads(&self.pool, READY_DAYS).await?;
        let ready = store::is_ready(&self.pool).await?;
        let trees = store::in_scope_thread_count(&self.pool).await? + 1;
        let mut scopes = vec![SummaryScope::Workspace];
        let mut owed: HashSet<SummaryScope> = HashSet::new();
        let mut gate: HashSet<SummaryScope> = HashSet::new();
        if !ready {
            owed.insert(SummaryScope::Workspace);
            gate.insert(SummaryScope::Workspace);
        }
        for thread in &threads {
            let scope = SummaryScope::Thread(thread.thread_id);
            scopes.push(scope);
            if !ready || thread.unreflected {
                owed.insert(scope);
            }
            if !ready && thread.ready_window {
                gate.insert(scope);
            }
        }
        log!(
            "[SummaryTree] Seeding {} scopes (ready: {}, {} owed, {} before ready, swept {} nodes of deleted threads)",
            scopes.len(),
            ready,
            owed.len(),
            gate.len(),
            swept
        );
        {
            let mut q = self.queue.lock().unwrap();
            // A drain running through the re-seed keeps counting its nodes,
            // and a failing scope keeps failing until it completes.
            let (nodes, failing) = match q.backfill.take() {
                Some(old) => (
                    old.nodes
                        .into_iter()
                        .filter(|(scope, _)| owed.contains(scope))
                        .collect(),
                    old.failing.intersection(&owed).copied().collect(),
                ),
                None => Default::default(),
            };
            q.backfill = (!owed.is_empty()).then(|| Backfill {
                // A thread created between the two reads is owed but uncounted.
                total: trees.max(scopes.len()),
                owed,
                gate,
                ready,
                nodes,
                failing,
            });
            for scope in scopes {
                mark_dirty_in(&mut q, scope, false);
            }
        }
        self.wake.notify_waiters();
        self.announce_progress().await;
        Ok(())
    }

    /// Mark `scope` dirty. `urgent` puts it at the front, for live events.
    /// Every idle worker wakes: a live-only one cannot take a backfill scope.
    pub(crate) fn mark_dirty(&self, scope: SummaryScope, urgent: bool) {
        mark_dirty_in(&mut self.queue.lock().unwrap(), scope, urgent);
        self.wake.notify_waiters();
    }

    /// The cache may have missed a change, so reload it whole.
    pub(crate) fn forget_workspace(&self) {
        self.workspace_stale.store(true, Ordering::SeqCst);
        self.mark_dirty(SummaryScope::Workspace, true);
    }

    /// A delete removed these threads' workspace leaves and moved the leaves
    /// after them. Their own scopes are re-marked so a drain sweeps any node
    /// written while the delete ran.
    pub(crate) fn forget_threads(&self, ids: &[Uuid]) {
        self.deleted_threads.lock().unwrap().extend(ids);
        self.mark_dirty(SummaryScope::Workspace, true);
        for id in ids {
            self.mark_dirty(SummaryScope::Thread(*id), true);
        }
    }

    async fn work(self: Arc<Self>, take: Take) {
        loop {
            if self.is_paused() {
                let resumed = self.resumed.notified();
                if self.is_paused() {
                    resumed.await;
                }
                continue;
            }
            let woken = self.wake.notified();
            let Some((scope, urgent)) = self.pop(take) else {
                tokio::select! {
                    _ = self.resumed.notified() => {}
                    _ = woken => {}
                }
                continue;
            };
            let outcome = match self.drain(scope, urgent).await {
                Ok(outcome) => outcome,
                Err(e) => {
                    log!("[SummaryTree] Drain of {} failed: {}", scope.as_db(), e);
                    Drained::Failed
                }
            };
            self.finish(scope, urgent, outcome).await;
            if outcome == Drained::ModelUnavailable {
                tokio::time::sleep(RETRY).await;
            }
        }
    }

    /// The next scope `take` allows, and whether a live event marked it.
    ///
    /// A live-only worker takes the first urgent scope, wherever it sits: an
    /// owed workspace drain marked by a live event can hold the front.
    fn pop(&self, take: Take) -> Option<(SummaryScope, bool)> {
        let mut q = self.queue.lock().unwrap();
        let at = match take {
            Take::Any => 0,
            Take::UrgentOnly if q.urgent.is_empty() => return None,
            Take::UrgentOnly => q.order.iter().position(|s| urgency(&q, *s))?,
        };
        let scope = q.order.remove(at)?;
        let urgent = urgency(&q, scope);
        q.queued.remove(&scope);
        q.urgent.remove(&scope);
        q.draining.insert(scope, false);
        Some((scope, urgent))
    }

    /// Drain everything queued, one scope at a time, on this task. Tests use
    /// it to run the compactor to rest without workers.
    #[cfg(test)]
    pub(crate) async fn drain_queue(self: &Arc<Self>) -> Vec<(SummaryScope, Drained)> {
        let mut outcomes = Vec::new();
        while let Some(outcome) = self.drain_next().await {
            outcomes.push(outcome);
        }
        outcomes
    }

    /// Drain the scope at the front of the queue, if any.
    #[cfg(test)]
    pub(crate) async fn drain_next(self: &Arc<Self>) -> Option<(SummaryScope, Drained)> {
        let (scope, urgent) = self.pop(Take::Any)?;
        let outcome = self.drain(scope, urgent).await.expect("drain");
        self.finish(scope, urgent, outcome).await;
        Some((scope, outcome))
    }

    /// The scope at the front of the queue, and whether it drains as urgent.
    #[cfg(test)]
    pub(crate) fn front(&self) -> Option<(SummaryScope, bool)> {
        let q = self.queue.lock().unwrap();
        let &front = q.order.front()?;
        Some((front, urgency(&q, front)))
    }

    /// What a live-only worker would take now.
    #[cfg(test)]
    pub(crate) fn pop_live(&self) -> Option<(SummaryScope, bool)> {
        self.pop(Take::UrgentOnly)
    }

    /// The scopes queued now, front first.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> Vec<SummaryScope> {
        self.queue.lock().unwrap().order.iter().copied().collect()
    }

    /// The lane `route`'s calls run in.
    #[cfg(test)]
    pub(crate) fn lane(&self, route: &str) -> Arc<super::limit::Lane> {
        self.lanes.lane(route)
    }

    /// Settle a drain. A scope that goes back on the queue keeps its urgency,
    /// so live work never lands behind the backfill.
    async fn finish(self: &Arc<Self>, scope: SummaryScope, urgent: bool, outcome: Drained) {
        let (redirty, progressed, gate_cleared, started_failing) = {
            let mut q = self.queue.lock().unwrap();
            let redirty = q.draining.remove(&scope).unwrap_or(false);
            let (mut progressed, mut gate_cleared, mut started_failing) = (false, None, false);
            if let Some(backfill) = q.backfill.as_mut() {
                match outcome {
                    Drained::Complete => {
                        progressed = backfill.owed.remove(&scope);
                        backfill.nodes.remove(&scope);
                        backfill.failing.remove(&scope);
                        if backfill.gate.remove(&scope) && backfill.gate.is_empty() {
                            gate_cleared = Some(backfill.total);
                        }
                    }
                    Drained::Failed if backfill.owed.contains(&scope) => {
                        started_failing = backfill.failing.insert(scope);
                    }
                    _ => {}
                }
            }
            (redirty, progressed, gate_cleared, started_failing)
        };
        if outcome == Drained::ModelUnavailable {
            self.set_waiting_for_model(true).await;
        }
        if let Some(total) = gate_cleared {
            self.record_ready(total).await;
        }
        if progressed || started_failing {
            self.announce_progress().await;
        }
        match outcome {
            Drained::Complete => {
                if redirty {
                    self.mark_dirty(scope, true);
                }
            }
            Drained::Paused => {
                if redirty {
                    self.mark_dirty(scope, false);
                }
            }
            Drained::Failed | Drained::ModelUnavailable => {
                self.mark_dirty_after(scope, urgent, RETRY)
            }
        }
    }

    /// Set the ready flag, then announce it. A failed write announces nothing:
    /// turns stay on Classic, and a re-seed after [`RETRY`] builds the gate
    /// again, which finds its trees built and sets the flag.
    async fn record_ready(self: &Arc<Self>, total: usize) {
        if let Err(e) = store::mark_ready(&self.pool).await {
            log!(
                "[SummaryTree] Could not record the ready flag, re-seeding in {:?}: {}",
                RETRY,
                e
            );
            let this = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(RETRY).await;
                if let Err(e) = this.seed().await {
                    log!("[SummaryTree] Could not re-seed the compactor: {}", e);
                }
            });
            return;
        }
        if let Some(backfill) = self.queue.lock().unwrap().backfill.as_mut() {
            backfill.ready = true;
        }
        log!("[SummaryTree] The workspace tree and the recent threads are built; the workspace is ready");
        self.bus
            .emit_or_log(
                BusEvent::System(SystemEvent::TreeBackfillCompleted { total }),
                "[SummaryTree] TreeBackfillCompleted",
            )
            .await;
    }

    fn mark_dirty_after(self: &Arc<Self>, scope: SummaryScope, urgent: bool, delay: Duration) {
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            this.mark_dirty(scope, urgent);
        });
    }

    pub(crate) async fn drain(
        self: &Arc<Self>,
        scope: SummaryScope,
        urgent: bool,
    ) -> Result<Drained, BoxError> {
        match scope {
            SummaryScope::Thread(id) => self.drain_thread(id, urgent).await,
            SummaryScope::Workspace => self.drain_workspace(urgent).await,
        }
    }

    async fn drain_thread(self: &Arc<Self>, id: Uuid, urgent: bool) -> Result<Drained, BoxError> {
        let scope = SummaryScope::Thread(id);
        let reading = self.reads.acquire().await.expect("never closed");
        let Some(info) = store::thread_in_scope(&self.pool, id).await? else {
            // Deleted, or a trigger run nobody has talked to: no tree.
            store::realign(&self.pool, scope, Some(0), &[], &[]).await?;
            return Ok(Drained::Complete);
        };
        let loaded = store::load_thread_log(&self.pool, id, info.kind).await?;
        let leaf_ids: Vec<Uuid> = loaded.log.entries.iter().map(|e| e.event_id).collect();
        let mut nodes = store::load_nodes(&self.pool, scope).await?;
        self.realign(scope, &leaf_ids, &mut nodes).await?;
        drop(reading);

        let source = ThreadSource {
            thread_id: id,
            entries: &loaded.log.entries,
        };
        let outcome = self.build(scope, urgent, &source, &mut nodes).await?;
        if matches!(outcome, Drained::Complete) {
            store::reflect(&self.pool, scope, loaded.through.unwrap_or(0)).await?;
            if loaded.has_newer {
                self.mark_dirty_after(scope, urgent, Duration::from_secs(HORIZON_SECS as u64));
            }
        }
        Ok(outcome)
    }

    /// A failed drain may have changed the cache without the database, so the
    /// next one starts from the database.
    async fn drain_workspace(self: &Arc<Self>, urgent: bool) -> Result<Drained, BoxError> {
        let result = self.drain_workspace_cached(urgent).await;
        if result.is_err() {
            self.workspace_stale.store(true, Ordering::SeqCst);
        }
        result
    }

    async fn drain_workspace_cached(self: &Arc<Self>, urgent: bool) -> Result<Drained, BoxError> {
        let scope = SummaryScope::Workspace;
        let mut guard = self.workspace.lock().await;
        let reading = self.reads.acquire().await.expect("never closed");
        if self.workspace_stale.swap(false, Ordering::SeqCst) {
            *guard = None;
        }
        let deleted = std::mem::take(&mut *self.deleted_threads.lock().unwrap());
        match guard.as_mut() {
            None => {
                *guard = Some(WorkspaceState {
                    log: WorkspaceLog::default(),
                    nodes: store::load_nodes(&self.pool, scope).await?,
                });
            }
            // The delete already purged their nodes; only the log is cached.
            Some(state) if !deleted.is_empty() => {
                state
                    .log
                    .leaves
                    .retain(|l| !l.thread_id().is_some_and(|t| deleted.contains(&t)));
                for leaf in &mut state.log.leaves {
                    leaf.forget_writer_in(&deleted);
                }
                state.nodes = store::load_nodes(&self.pool, scope).await?;
            }
            Some(_) => {}
        }
        let state = guard.as_mut().expect("loaded above");
        let has_newer = state.log.extend(&self.pool).await?;
        let leaf_ids: Vec<Uuid> = state.log.leaves.iter().map(|l| l.event_id()).collect();
        self.realign(scope, &leaf_ids, &mut state.nodes).await?;
        drop(reading);

        let source = WorkspaceSource::new(self, &state.log.leaves);
        let outcome = self.build(scope, urgent, &source, &mut state.nodes).await?;
        if matches!(outcome, Drained::Complete) {
            store::reflect(&self.pool, scope, state.log.through).await?;
        }
        if has_newer {
            self.mark_dirty_after(scope, urgent, Duration::from_secs(HORIZON_SECS as u64));
        }
        Ok(outcome)
    }

    /// Bring stored nodes in line with the log, and drop what no longer fits.
    ///
    /// A leaf whose source moved is re-keyed to its new position with its
    /// text. That happens after a delete, or when a slow write committed late.
    /// Realigning starts at the lowest position a leaf left or moved to. A
    /// merge reaching past it, or missing a child, is dropped. So no stored
    /// node ever disagrees with what a rebuild would write (I1).
    async fn realign(
        &self,
        scope: SummaryScope,
        leaf_ids: &[Uuid],
        nodes: &mut HashMap<NodeAddr, StoredNode>,
    ) -> Result<(), sqlx::Error> {
        let len = leaf_ids.len() as u64;
        let position: HashMap<Uuid, u64> = leaf_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i as u64))
            .collect();
        let from = nodes
            .iter()
            .filter(|(a, n)| {
                a.is_leaf()
                    && (a.start >= len || n.source_event_id != Some(leaf_ids[a.start as usize]))
            })
            .map(|(a, n)| {
                let moved_to = n.source_event_id.and_then(|s| position.get(&s)).copied();
                moved_to.map_or(a.start, |to| to.min(a.start))
            })
            .min();

        let mut moved = Vec::new();
        if let Some(from) = from {
            let stale: Vec<NodeAddr> = nodes
                .keys()
                .filter(|a| {
                    if a.is_leaf() {
                        a.start >= from
                    } else {
                        a.end() > from
                    }
                })
                .copied()
                .collect();
            for addr in stale {
                let node = nodes.remove(&addr).expect("listed above");
                let new_start = node.source_event_id.and_then(|s| position.get(&s)).copied();
                if let (true, Some(start)) = (addr.is_leaf(), new_start) {
                    moved.push((NodeAddr::leaf(start), node));
                }
            }
            for (addr, node) in &moved {
                nodes.insert(*addr, node.clone());
            }
        }

        let mut merges: Vec<NodeAddr> = nodes.keys().filter(|a| !a.is_leaf()).copied().collect();
        merges.sort_by_key(|a| a.span);
        let mut orphans = Vec::new();
        for addr in merges {
            let (a, b) = addr.children().expect("a merge has children");
            if !nodes.contains_key(&a) || !nodes.contains_key(&b) {
                nodes.remove(&addr);
                orphans.push(addr);
            }
        }

        if from.is_some() || !orphans.is_empty() {
            log!(
                "[SummaryTree] Realigned {} from entry {:?}: {} leaves moved, {} orphan merges dropped",
                scope.as_db(),
                from,
                moved.len(),
                orphans.len()
            );
            store::realign(&self.pool, scope, from, &moved, &orphans).await?;
        }
        Ok(())
    }

    /// Build every node the pump allows, at most [`MAX_LIMIT`] in flight.
    ///
    /// Finished nodes are applied in launch order. So which lines exist when
    /// the next node launches, and with them its context, follow from the log
    /// alone, however long each call took. Each node runs on its own task, so
    /// a finished one releases its connection while the next leaf loads.
    async fn build(
        self: &Arc<Self>,
        scope: SummaryScope,
        urgent: bool,
        source: &(dyn TreeSource + '_),
        nodes: &mut HashMap<NodeAddr, StoredNode>,
    ) -> Result<Drained, BoxError> {
        let len = source.len();
        let mut texts: HashMap<NodeAddr, String> =
            nodes.iter().map(|(a, n)| (*a, n.text.clone())).collect();
        let mut pending = shape::Pending::new(len, &texts);
        if self.count_nodes(scope, pending.remaining()) {
            self.announce_progress().await;
        }
        let mut busy = HashSet::new();
        let mut inflight = FuturesOrdered::new();
        let (mut failed, mut unavailable) = (false, false);

        loop {
            if !failed && !self.is_paused() {
                let room = MAX_LIMIT - inflight.len();
                for addr in pending.buildable(&texts, &busy, source.leaf_order(), room) {
                    let input = if addr.is_leaf() {
                        // A failed read ends the drain only once every
                        // launched node has landed.
                        match source.leaf(addr.start, &texts).await {
                            Ok(leaf) => NodeInput::Leaf(leaf),
                            Err(e) => {
                                self.report_once(scope, addr, &e.to_string());
                                failed = true;
                                break;
                            }
                        }
                    } else {
                        let (a, b) = addr.children().expect("a merge has children");
                        NodeInput::Merge {
                            a: texts[&a].clone(),
                            b: texts[&b].clone(),
                            context: tiling_lines(addr.end(), &texts),
                        }
                    };
                    busy.insert(addr);
                    let this = self.clone();
                    let capture_thread = source.capture_thread(addr);
                    let node = tokio::spawn(async move {
                        this.build_node(scope, addr, input, capture_thread, urgent)
                            .await
                    });
                    inflight.push_back(async move {
                        let result = node.await.unwrap_or_else(|e| {
                            Err(NodeError::Failed(format!("the node's task ended: {e}")))
                        });
                        (addr, result)
                    });
                }
            }
            let Some((addr, result)) = inflight.next().await else {
                break;
            };
            busy.remove(&addr);
            match result {
                Ok(node) => {
                    pending.mark_built(addr);
                    texts.insert(addr, node.text.clone());
                    nodes.insert(addr, node);
                    if self.node_built(scope) {
                        self.announce_progress().await;
                    }
                }
                Err(NodeError::ModelUnavailable(e)) => {
                    self.report_once(scope, addr, &e);
                    (failed, unavailable) = (true, true);
                }
                Err(NodeError::Failed(e)) => {
                    self.report_once(scope, addr, &e);
                    failed = true;
                }
            }
        }

        Ok(if unavailable {
            Drained::ModelUnavailable
        } else if failed {
            Drained::Failed
        } else if !pending.is_done() {
            // Only a pause stops the pump with nodes left to build.
            Drained::Paused
        } else {
            Drained::Complete
        })
    }

    fn report_once(&self, scope: SummaryScope, addr: NodeAddr, error: &str) {
        if self.reported.lock().unwrap().insert((scope, addr)) {
            log!(
                "[SummaryTree] Node {} of {} failed, retrying every {:?}: {}",
                addr,
                scope.as_db(),
                RETRY,
                error
            );
        }
    }

    /// Write one node and store it. A source that already fits is the node
    /// verbatim, with no model call.
    async fn build_node(
        &self,
        scope: SummaryScope,
        addr: NodeAddr,
        input: NodeInput,
        capture_thread: Option<Uuid>,
        urgent: bool,
    ) -> Result<StoredNode, NodeError> {
        let (verbatim, step, context, source_event_id, source_thread_id) = match input {
            NodeInput::Leaf(leaf) => (
                leaf.message.clone(),
                prompt::compress_step(&leaf.message),
                leaf.context,
                Some(leaf.source_event_id),
                leaf.source_thread_id,
            ),
            NodeInput::Merge { a, b, context } => (
                format!("{a}\n{b}"),
                prompt::merge_step(&a, &b),
                context,
                None,
                None,
            ),
        };
        let (text, model) = if verbatim.len() <= NODE_BYTES {
            (verbatim, None)
        } else {
            let (text, model) = self
                .write_line(&context, &step, capture_thread, urgent)
                .await?;
            (text, Some(model))
        };
        let node = StoredNode {
            text,
            model,
            source_event_id,
            source_thread_id,
        };
        store::insert_node(&self.pool, scope, addr, &node).await?;
        Ok(node)
    }

    /// One model-written line, under the purpose's whole-node deadline, in
    /// the lane of the route it takes.
    async fn write_line(
        &self,
        context: &[String],
        step: &str,
        capture_thread: Option<Uuid>,
        urgent: bool,
    ) -> Result<(String, String), NodeError> {
        let model = self
            .deps
            .model()
            .await
            .map_err(NodeError::ModelUnavailable)?;
        let permit = self.lanes.lane(&model.lane).acquire(urgent).await;
        let capture = AuxCapture::for_thread_or_home(
            &self.bus,
            capture_thread,
            ContextPurpose::SummaryCompaction,
        );
        let request = prompt::request(context, step);
        match fit_line(&model, &request, &capture).await {
            Ok(line) => {
                permit.succeeded();
                self.set_waiting_for_model(false).await;
                Ok((line, model.model))
            }
            Err(NodeError::Failed(e)) => {
                if crate::llm::is_capacity_error(&e) {
                    permit.congested();
                }
                Err(NodeError::Failed(e))
            }
            Err(e) => Err(e),
        }
    }
}

/// Which scopes a worker takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Take {
    Any,
    /// Only a scope a live event marked.
    UrgentOnly,
}

/// Whether queued `scope` drains as urgent. The workspace drain the backfill
/// still owes never does, though a live event marked it: it holds up to
/// [`MAX_LIMIT`] calls of history, and urgent ones would starve every other
/// call in the lane.
fn urgency(q: &Queue, scope: SummaryScope) -> bool {
    let backfilling = scope == SummaryScope::Workspace
        && q.backfill.as_ref().is_some_and(|b| b.owed.contains(&scope));
    q.urgent.contains(&scope) && !backfilling
}

/// A scope being drained is only flagged, so its drain runs again once it
/// ends: two drains of one scope must never overlap.
fn mark_dirty_in(q: &mut Queue, scope: SummaryScope, urgent: bool) {
    match q.draining.get_mut(&scope) {
        Some(redirty) => *redirty = true,
        None => enqueue(q, scope, urgent),
    }
}

fn enqueue(q: &mut Queue, scope: SummaryScope, urgent: bool) {
    if urgent {
        q.urgent.insert(scope);
    }
    if !q.queued.insert(scope) {
        if urgent {
            q.order.retain(|s| *s != scope);
            q.order.push_front(scope);
        }
        return;
    }
    if urgent {
        q.order.push_front(scope);
    } else {
        q.order.push_back(scope);
    }
}

/// Ask for a line until it fits, in one conversation (OptChat §4.3). After
/// [`TRIES`] answers the shortest wins, a few bytes over at worst: the size is
/// a target nothing downstream relies on.
///
/// The purpose's deadline covers every round. It bounds each provider call,
/// never its record, so a round that answered in time is never lost.
async fn fit_line(
    model: &CompactionModel,
    request: &str,
    capture: &AuxCapture,
) -> Result<String, NodeError> {
    let mut messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text(request.to_string()),
    }];
    let mut tries: Vec<String> = Vec::new();
    let capture = capture.until(tokio::time::Instant::now() + model.deadline);
    loop {
        let call = capture.chat(
            model.provider.as_ref(),
            messages.clone(),
            vec![],
            crate::llm::ModelSelection::model(&model.model).with_effort(model.effort.as_deref()),
            Some(prompt::COMPACT),
            None,
        );
        let response = call.await.map_err(|e| {
            if e.is::<tokio::time::error::Elapsed>() {
                NodeError::Failed(format!("timed out after {:?}", model.deadline))
            } else {
                NodeError::from(e)
            }
        })?;
        let line = response.content.unwrap_or_default().trim().to_string();
        if line.is_empty() {
            return Err(NodeError::Failed("the model returned an empty line".into()));
        }
        tries.push(line.clone());
        if line.len() <= NODE_BYTES || tries.len() >= TRIES {
            break;
        }
        messages.push(Message {
            role: "assistant".to_string(),
            content: MessageContent::Text(line.clone()),
        });
        messages.push(Message {
            role: "user".to_string(),
            content: MessageContent::Text(prompt::too_long(&line)),
        });
    }
    Ok(tries
        .into_iter()
        .min_by_key(String::len)
        .expect("at least one try"))
}

/// The context lines for a node ending at `end`: the prefix's context tiling.
fn tiling_lines(end: u64, texts: &HashMap<NodeAddr, String>) -> Vec<String> {
    fold::context_tiling(end, |a| texts.get(&a).map(|t| t.len() + 1), CONTEXT_BYTES)
        .iter()
        .filter_map(|a| texts.get(a).cloned())
        .collect()
}

/// The raw lines of a thread's entries before `end`, newest first until
/// [`CONTEXT_BYTES`], returned oldest first.
fn raw_context(entries: &[LogEntry], end: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut bytes = 0;
    for entry in entries[..end].iter().rev() {
        let line = entry.raw_line();
        bytes += line.len() + 1;
        if bytes > CONTEXT_BYTES {
            break;
        }
        lines.push(line);
    }
    lines.reverse();
    lines
}

/// A workspace turn leaf's message: the thread's title, then the turn's lines.
fn turn_message(title: &str, lines: Option<&[String]>) -> String {
    let body = match lines {
        Some(lines) if !lines.is_empty() => lines.join("\n"),
        _ => "(no messages)".to_string(),
    };
    format!("{}: [{title}]\n{body}", EntryKind::Turn.as_str())
}

/// One tree's log, as the build loop sees it.
#[async_trait::async_trait]
trait TreeSource: Send + Sync {
    fn len(&self) -> u64;

    /// Whether a leaf reads the leaves before it, and so waits on them.
    fn leaf_order(&self) -> LeafOrder;

    /// What leaf `index` compresses. `texts` is this tree's built lines.
    async fn leaf(
        &self,
        index: u64,
        texts: &HashMap<NodeAddr, String>,
    ) -> Result<LeafInput, BoxError>;

    /// The thread a node's model call is recorded on, for token accounting.
    /// `None` records it on the home thread.
    fn capture_thread(&self, addr: NodeAddr) -> Option<Uuid>;
}

struct ThreadSource<'a> {
    thread_id: Uuid,
    entries: &'a [LogEntry],
}

#[async_trait::async_trait]
impl TreeSource for ThreadSource<'_> {
    fn len(&self) -> u64 {
        self.entries.len() as u64
    }

    fn leaf_order(&self) -> LeafOrder {
        LeafOrder::InOrder
    }

    async fn leaf(
        &self,
        index: u64,
        texts: &HashMap<NodeAddr, String>,
    ) -> Result<LeafInput, BoxError> {
        let entry = &self.entries[index as usize];
        Ok(LeafInput {
            message: entry.message(),
            context: tiling_lines(index, texts),
            source_event_id: entry.event_id,
            source_thread_id: None,
        })
    }

    fn capture_thread(&self, _addr: NodeAddr) -> Option<Uuid> {
        Some(self.thread_id)
    }
}

struct WorkspaceSource<'a> {
    compactor: &'a Compactor,
    leaves: &'a [WorkspaceLeaf],
    /// Threads read during this drain. Consecutive turn leaves are often one
    /// conversation, and each would otherwise reload it. `None` caches a
    /// thread that is out of scope or gone.
    threads: Mutex<HashMap<Uuid, Option<Arc<ThreadSnapshot>>>>,
}

/// One thread's log, as a workspace drain read it.
struct ThreadSnapshot {
    title: String,
    log: log::ThreadLog,
}

/// Threads a workspace drain keeps at once.
const THREAD_CACHE: usize = 64;

impl<'a> WorkspaceSource<'a> {
    fn new(compactor: &'a Compactor, leaves: &'a [WorkspaceLeaf]) -> Self {
        Self {
            compactor,
            leaves,
            threads: Mutex::default(),
        }
    }

    async fn thread(&self, id: Uuid) -> Result<Option<Arc<ThreadSnapshot>>, BoxError> {
        if let Some(hit) = self.threads.lock().unwrap().get(&id) {
            return Ok(hit.clone());
        }
        let pool = &self.compactor.pool;
        let snapshot = match store::thread_in_scope(pool, id).await? {
            None => None,
            Some(info) => {
                let loaded = store::load_thread_log(pool, id, info.kind).await?;
                Some(Arc::new(ThreadSnapshot {
                    title: info.title.unwrap_or_else(|| "Untitled thread".to_string()),
                    log: loaded.log,
                }))
            }
        };
        let mut cache = self.threads.lock().unwrap();
        if cache.len() >= THREAD_CACHE {
            cache.clear();
        }
        cache.insert(id, snapshot.clone());
        Ok(snapshot)
    }
}

#[async_trait::async_trait]
impl TreeSource for WorkspaceSource<'_> {
    fn len(&self) -> u64 {
        self.leaves.len() as u64
    }

    fn leaf_order(&self) -> LeafOrder {
        LeafOrder::Any
    }

    /// A turn leaf compresses that turn's entries as raw lines, and reads the
    /// thread's earlier entries the same way as context. It reads no summary
    /// line, so it never waits on a tree and lands the same whichever scope
    /// built first. It never reads another thread, so a delete leaves no line
    /// written from the deleted words.
    async fn leaf(
        &self,
        index: u64,
        _texts: &HashMap<NodeAddr, String>,
    ) -> Result<LeafInput, BoxError> {
        let (event_id, thread_id) = match &self.leaves[index as usize] {
            WorkspaceLeaf::Artifact {
                event_id,
                path,
                commit,
                verb,
                ..
            } => {
                let mut message = format!("{}: {path} ({verb})", EntryKind::Artifact.as_str());
                if let Some(content) = self.compactor.deps.read_artifact(path, commit) {
                    message.push('\n');
                    message.push_str(&log::cap(&content));
                }
                return Ok(LeafInput {
                    message,
                    context: Vec::new(),
                    source_event_id: *event_id,
                    source_thread_id: None,
                });
            }
            WorkspaceLeaf::Turn {
                event_id,
                thread_id,
            } => (*event_id, *thread_id),
        };
        let leaf = |message: String, context: Vec<String>| LeafInput {
            message,
            context,
            source_event_id: event_id,
            source_thread_id: Some(thread_id),
        };
        let Some(thread) = self.thread(thread_id).await? else {
            return Ok(leaf(
                turn_message("(a thread no longer here)", None),
                Vec::new(),
            ));
        };
        let Some(turn) = thread.log.turn(event_id) else {
            return Ok(leaf(turn_message(&thread.title, None), Vec::new()));
        };
        let entries = &thread.log.entries;
        let lines: Vec<String> = entries[turn.entries.clone()]
            .iter()
            .map(|e| prompt::flatten(&e.raw_line()))
            .collect();
        Ok(leaf(
            turn_message(&thread.title, Some(&lines)),
            raw_context(entries, turn.entries.start),
        ))
    }

    /// A node is recorded on the newest thread inside it: a turn's own, or an
    /// artifact's writer. A stretch with neither records on the home thread.
    fn capture_thread(&self, addr: NodeAddr) -> Option<Uuid> {
        self.leaves[addr.start as usize..addr.end() as usize]
            .iter()
            .rev()
            .find_map(WorkspaceLeaf::capture_thread)
    }
}
