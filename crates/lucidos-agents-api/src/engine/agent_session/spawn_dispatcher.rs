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

pub struct SpawnDispatcher {
    pool: PgPool,
    spawn_tx: UnboundedSender<SpawnRequest>,
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

    pub(crate) async fn already_spawned(&self, event_id: Uuid) -> bool {
        if self.spawned_for_event.lock().await.contains(&event_id) {
            return true;
        }
        match has_cc_event_after(&self.pool, event_id).await {
            Ok(true) => {
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

    pub(crate) async fn dispatch_spawn(&self, request: SpawnRequest) {
        let SpawnRequest::Continue {
            thread_id,
            event_id,
        } = request;
        if !self.spawned_for_event.lock().await.insert(event_id) {
            return;
        }
        if let Err(e) = self.spawn_tx.send(request) {
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
