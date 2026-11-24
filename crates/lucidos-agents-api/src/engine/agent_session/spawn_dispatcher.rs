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

use crate::engine::continuation_queries::{
    continuation_superseded_events_sql, CC_LIFECYCLE_EVENTS_SQL,
};

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
                log_legacy!(
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
        log_legacy!("[SpawnDispatcher] starting");
        while let Some(request) = requests.recv().await {
            let SpawnRequest::Continue { event_id, .. } = request;
            if self.already_spawned(event_id).await {
                continue;
            }
            self.dispatch_spawn(request).await;
        }
        log_legacy!("[SpawnDispatcher] request channel closed");
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
                log_legacy!(
                    "[SpawnDispatcher] already_spawned DB lookup failed for {}: {}",
                    event_id,
                    e
                );
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
            log_legacy!(
                "[SpawnDispatcher] failed to send Continue spawn request for thread={} event={}: {}",
                thread_id, event_id, e
            );
        } else {
            log_legacy!(
                "[SpawnDispatcher] dispatched Continue spawn thread={} event={}",
                thread_id,
                event_id
            );
        }
    }

    pub(crate) async fn redispatch_orphaned_continuations_on_startup(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
        log_legacy!(
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
