//! Coding-agent spawn dispatcher.
//!
//! Every committed `ContinuationRequested` reaches this dispatcher on a
//! directed channel the `EventBus` sends to after commit
//! ([`EventBus::take_spawn_requests`]). The dispatcher claims the request's
//! event id and forwards a [`SpawnRequest`] to the engine-side consumer, which
//! calls `run_direct_agent` with `--resume` (ADR 0272).
//!
//! The bus creates the unbounded channel itself. So a request emitted before
//! the dispatcher starts waits for it, and a broadcast lag never drops one.
//!
//! Idempotency is keyed on the trigger event id: [`SpawnDispatcher::dispatch_spawn`]
//! gates the send on atomically claiming it. Two paths can see one request,
//! the channel and the startup orphan scan, and the claim lets one through.
//!
//! The orphan scan (`redispatch_orphaned_continuations_on_startup`) is the
//! durable floor for a request a previous process persisted and never actuated,
//! on a thread still `running`.

use std::collections::HashSet;
use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::engine::event_bus::EventBus;
pub use crate::engine::event_bus::SpawnRequest;
use crate::engine::thread_lifecycle::ThreadStatus;

/// SQL fragment listing the CC lifecycle events whose presence AFTER a trigger
/// event implies the trigger was already picked up (CC clearly ran in response).
/// Shared by [`has_cc_event_after`] and the orphaned-continuation scan so the
/// predicates can't drift apart.
///
/// `ContinuationStarted` belongs here: the spawn consumer emits it BEFORE
/// invoking `run_direct_agent` on every recovery-shaped resume, so it is the
/// earliest actuation marker — and treating a request it follows as
/// unactuated would re-dispatch mid-cold-start resumes and defeat the crash
/// loop-breaker (`switch_was_user_initiated` counts it as a start for the
/// same reason).
const CC_LIFECYCLE_EVENTS_SQL: &str = "'SessionStarted', 'CodingAgentIdled', \
    'SessionEnded', 'CodingAgentToolCalled', 'CodingAgentTextStreamed', \
    'CodingAgentUserMessageSent', 'ContinuationStarted'";

/// SQL fragment for "this `ContinuationRequested` is no longer live": any CC
/// lifecycle event (actuated), a LATER `ContinuationRequested` (superseded —
/// this yields newest-per-thread for free), or a terminal response event (the
/// thread was settled after the request, for example by
/// `settle_orphaned_running_threads`, and must stay settled with
/// its manual Continue affordance). Shared by the orphan scan and
/// [`thread_has_unactuated_continuation`].
fn continuation_superseded_events_sql() -> String {
    format!(
        "{CC_LIFECYCLE_EVENTS_SQL}, 'ContinuationRequested', 'ResponseAborted', \
         'ResponseCanceled', 'ResponseFailed', 'ResponseGenerated'"
    )
}

/// Claims each spawn request once and forwards it to the engine-side consumer.
pub struct SpawnDispatcher {
    pool: PgPool,
    /// Outbound channel — the dispatcher sends [`SpawnRequest`]s here and a
    /// receiver task on `LucidosEngine` calls `run_direct_agent` for each.
    /// `Unbounded` because dropping a continuation request would leave a
    /// thread stuck "Working" forever.
    spawn_tx: UnboundedSender<SpawnRequest>,
    /// Trigger event ids the dispatcher has already acted on (or knows are
    /// already covered by existing CC events). Survives the lifetime of the
    /// dispatcher.
    spawned_for_event: Arc<Mutex<HashSet<Uuid>>>,
}

impl SpawnDispatcher {
    pub(crate) fn new(pool: PgPool, spawn_tx: UnboundedSender<SpawnRequest>) -> Self {
        Self {
            pool,
            spawn_tx,
            spawned_for_event: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Take the bus's spawn requests, run the startup orphan scan, then drain
    /// the requests on a tokio task. Returns the receiver end of the outbound
    /// channel so the caller can wire it to the engine-side actuator.
    ///
    /// Panics when the bus's spawn requests were already taken: a second
    /// dispatcher on one bus would split the requests between two claim sets.
    pub fn spawn(
        pool: PgPool,
        bus: Arc<EventBus>,
    ) -> (
        tokio::task::JoinHandle<()>,
        tokio::sync::mpsc::UnboundedReceiver<SpawnRequest>,
    ) {
        let requests = bus
            .take_spawn_requests()
            .expect("one spawn dispatcher per event bus");
        let (spawn_tx, spawn_rx) = tokio::sync::mpsc::unbounded_channel();
        let dispatcher = Self::new(pool, spawn_tx);
        let handle = tokio::spawn(async move {
            // Best-effort: a failed scan must not disable the live channel.
            if let Err(e) = dispatcher
                .redispatch_orphaned_continuations_on_startup()
                .await
            {
                log!(
                    "[SpawnDispatcher] redispatch_orphaned_continuations_on_startup failed: {}",
                    e
                );
            }
            dispatcher.run(requests).await;
        });
        (handle, spawn_rx)
    }

    /// Dispatch every request the bus sends. Lives for the duration of the
    /// engine process.
    pub(crate) async fn run(self, mut requests: UnboundedReceiver<SpawnRequest>) {
        log!("[SpawnDispatcher] starting");
        while let Some(request) = requests.recv().await {
            let SpawnRequest::Continue { event_id, .. } = request;
            if self.already_spawned(event_id).await {
                continue;
            }
            self.dispatch_spawn(request).await;
        }
        log!("[SpawnDispatcher] request channel closed");
    }

    /// Returns true when a spawn for this trigger has already happened — either
    /// dispatched by this process (in-memory set) or implied by a subsequent
    /// CC lifecycle event in the DB.
    pub(crate) async fn already_spawned(&self, event_id: Uuid) -> bool {
        // Fast path — in-memory cache.
        if self.spawned_for_event.lock().await.contains(&event_id) {
            return true;
        }
        // Slow path — does the events table show a CC lifecycle event after
        // this trigger? `CodingAgentIdled`/`SessionEnded`/`SessionStarted`
        // imply CC was spawned in response.
        match has_cc_event_after(&self.pool, event_id).await {
            Ok(true) => {
                // Cache so a second sighting skips the DB hit.
                self.spawned_for_event.lock().await.insert(event_id);
                true
            }
            Ok(false) => false,
            Err(e) => {
                log!(
                    "[SpawnDispatcher] already_spawned DB lookup failed for {}: {}",
                    event_id,
                    e
                );
                // Conservative default: assume not handled so the trigger is
                // dispatched. A transient DB failure is preferable to silently
                // dropping a user's continuation request.
                false
            }
        }
    }

    /// Claim the request's event id and send it to the engine-side consumer.
    ///
    /// The insert into the idempotency set IS the claim. A request seen by
    /// both the startup orphan scan and the live channel produces at most ONE
    /// send, whichever path claims the id first.
    pub(crate) async fn dispatch_spawn(&self, request: SpawnRequest) {
        let SpawnRequest::Continue {
            thread_id,
            event_id,
        } = request;
        if !self.spawned_for_event.lock().await.insert(event_id) {
            return;
        }
        if let Err(e) = self.spawn_tx.send(request) {
            // Receiver gone → engine is shutting down, or the receiver task
            // died. The next boot's recovery owns the thread.
            log!(
                "[SpawnDispatcher] failed to send Continue spawn request for thread={} event={}: {}",
                thread_id, event_id, e
            );
        } else {
            log!(
                "[SpawnDispatcher] dispatched Continue spawn thread={} event={}",
                thread_id,
                event_id
            );
        }
    }

    /// Startup safety net (durable): re-dispatch every `ContinuationRequested`
    /// that was emitted but never actuated — per thread, the NEWEST request
    /// with no subsequent CC lifecycle event, no later `ContinuationRequested`
    /// (a newer request supersedes older ones), no terminal response event
    /// (a thread something settled after the request must stay settled), and
    /// the projection still showing `status = 'running'` (the zombie
    /// fingerprint: the request's projection arm flips status to running, so
    /// an unactuated request leaves it there).
    ///
    /// The live channel covers every request this process emits. This scan
    /// covers one a previous process persisted and boot left `running`, such
    /// as a switch-resumed thread whose request postdates the teardown abort.
    ///
    /// Crash contract: a crash-interrupted thread never gets a
    /// `ContinuationRequested` (recovery emits `CodingAgentIdled` and keeps
    /// the manual Continue affordance — see `switch_was_user_initiated`), so
    /// this scan cannot auto-resume work that may have crashed the engine.
    pub(crate) async fn redispatch_orphaned_continuations_on_startup(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // No LIMIT on purpose: a silent cap would strand any orphan beyond it
        // for the lifetime of the process (this scan only runs at startup).
        // The predicate itself bounds the result — one row per zombie thread —
        // and both outer filters are index-backed (`idx_events_type_created`,
        // `idx_events_aggregate_id`).
        let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(&format!(
            "SELECT e.id, e.aggregate_id::uuid FROM events e \
             JOIN thread_summaries t ON t.thread_id = e.aggregate_id::uuid \
             WHERE e.event_type = 'ContinuationRequested' \
               AND e.aggregate = 'thread' \
               AND t.source = 'claude_code' \
               AND t.status = {} \
               AND NOT EXISTS ( \
                   SELECT 1 FROM events e2 \
                   WHERE e2.aggregate_id = e.aggregate_id \
                     AND e2.sequence > e.sequence \
                     AND e2.event_type IN ({}) \
               ) \
             ORDER BY e.sequence ASC",
            ThreadStatus::Running.sql_literal(),
            continuation_superseded_events_sql()
        ))
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(());
        }
        log!(
            "[SpawnDispatcher] re-dispatching {} orphaned ContinuationRequested event(s)",
            rows.len()
        );
        // No `already_spawned` pre-check: the scan's NOT-EXISTS set is a
        // superset of `has_cc_event_after`'s, so the DB fallback is provably
        // false for every row, and `dispatch_spawn`'s atomic insert is the
        // claim that dedupes against the live channel.
        for (event_id, thread_id) in rows {
            self.dispatch_spawn(SpawnRequest::Continue {
                thread_id,
                event_id,
            })
            .await;
        }
        Ok(())
    }
}

/// True when the thread's newest `ContinuationRequested` is still live — no
/// subsequent CC lifecycle event, later request, or terminal response, and the
/// thread still `running` — i.e. a resume was requested but never actuated.
/// `resume_pending_switches` consults this before emitting: the startup orphan
/// re-dispatch ([`SpawnDispatcher::redispatch_orphaned_continuations_on_startup`])
/// owns driving the existing request, and a second request event id for the
/// same thread would slip past the per-EVENT idempotency guard and
/// double-spawn.
///
/// Deliberately the SAME predicate (filters included) as the orphan scan, so
/// "skip the emit" implies "the scan will drive it" — a `true` here with the
/// scan excluding the row would strand the thread with no resume at all.
/// Errors default to `false` (emit anyway): a transient DB failure must not
/// silently drop a user's auto-resume.
pub(crate) async fn thread_has_unactuated_continuation(pool: &PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS ( \
             SELECT 1 FROM events e \
             JOIN thread_summaries t ON t.thread_id = e.aggregate_id::uuid \
             WHERE e.aggregate_id = $1 \
               AND e.event_type = 'ContinuationRequested' \
               AND e.aggregate = 'thread' \
               AND t.source = 'claude_code' \
               AND t.status = {} \
               AND NOT EXISTS ( \
                   SELECT 1 FROM events e2 \
                   WHERE e2.aggregate_id = e.aggregate_id \
                     AND e2.sequence > e.sequence \
                     AND e2.event_type IN ({}) \
               ) \
         )",
        ThreadStatus::Running.sql_literal(),
        continuation_superseded_events_sql()
    ))
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| {
        log!(
            "[SpawnDispatcher] unactuated-continuation probe failed for {}: {}; emitting anyway",
            thread_id,
            e
        );
        false
    })
}

/// Returns true when the events table contains a CC lifecycle event whose
/// sequence is greater than the trigger event's sequence (same thread).
/// Implies the trigger has already been picked up by an earlier dispatcher
/// or the legacy direct-call path.
async fn has_cc_event_after(pool: &PgPool, trigger_event_id: Uuid) -> Result<bool, sqlx::Error> {
    let row: Option<(bool,)> = sqlx::query_as(&format!(
        "SELECT EXISTS ( \
             SELECT 1 FROM events e2 \
             JOIN events trig ON trig.id = $1 \
             WHERE e2.aggregate_id = trig.aggregate_id \
               AND e2.sequence > trig.sequence \
               AND e2.event_type IN ({CC_LIFECYCLE_EVENTS_SQL}) \
         )"
    ))
    .bind(trigger_event_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(b,)| b).unwrap_or(false))
}

#[cfg(test)]
#[path = "spawn_dispatcher_tests.rs"]
mod tests;
