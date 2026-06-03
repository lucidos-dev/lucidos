//! Event-driven CC spawn dispatcher (Phase 5).
//!
//! Subscribes to the EventBus and decides when a Claude Code spawn should be
//! initiated. Two trigger types are recognized today:
//!
//! - `UserMessage` — a `MessageReceived` event on a CC-channel thread. The
//!   chat HTTP handler still owns the spawn for this trigger (it has to,
//!   because the surrounding post-processing — auto-apply, ChangesUpdated
//!   SSE, orphan re-submission, ResponseFailed — is not yet event-driven).
//!   The dispatcher therefore stays in **shadow mode** for this trigger:
//!   it logs that it would have dispatched and bumps an observability
//!   counter, but does NOT call into `run_direct_agent`. Migrating the
//!   chat handler is the work that belongs to the next phase; see
//!   `docs/plans/2026-04-24-cc-resume-spike-q7.md` for the deferral
//!   rationale.
//!
//! - `ContinuationRequested` — emitted by the recovery path (Phase 5.3) when a
//!   mid-turn-interrupted Claude Code session needs to resume without a new user
//!   message. Production-active: the dispatcher pushes a [`SpawnRequest`]
//!   onto an mpsc channel that a long-running task on `LucidosEngine`
//!   consumes, calling `run_direct_agent` with empty input so CC re-enters
//!   `--resume` against its prior session and continues from where it left
//!   off.
//!
//! Idempotency is keyed on the trigger event id: the same trigger never
//! produces two spawns, even across engine restarts (the in-memory set is
//! seeded by `backfill_pending_triggers_on_startup`, which scans the events
//! table for triggers already followed by a CC lifecycle event).
//!
//! Phase 3 (`PermissionResolved`) is still blocked by the spike outcome
//! documented in `docs/plans/2026-04-24-cc-resume-spike-q7.md`. Until then
//! the [`SpawnTrigger`] enum carries only `UserMessage` and `ContinuationRequested`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, ThreadEvent};

/// Concrete spawn instruction emitted by the dispatcher and consumed by
/// the engine-side receiver task. Variants stay parallel to
/// [`SpawnTrigger`] so adding a new trigger means adding the matching
/// `SpawnRequest` and a receiver branch — never a string discriminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnRequest {
    /// Re-enter a Claude Code session that was interrupted mid-turn. The receiver
    /// invokes `run_direct_agent` with empty input so CC reconnects via
    /// `--resume` against its existing session id and continues without a
    /// new user message.
    Continue {
        thread_id: Uuid,
        /// The originating `ContinuationRequested` event id — used downstream for
        /// idempotency and tracing.
        event_id: Uuid,
    },
    // A `UserMessage` variant is intentionally absent today. The chat
    // HTTP handler still owns spawning for that trigger; see module docs.
}

/// What kinds of events trigger a CC spawn. Phase 3 will add a
/// `PermissionAnswer` variant once the deferred-`tool_result` path is unblocked.
///
/// `UserMessage.text` is unused outside tests today — the chat handler still
/// owns spawning for that trigger so the dispatcher only logs/counts. The
/// field is kept in the taxonomy because the next cutover phase consumes it
/// when actuating the spawn directly.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum SpawnTrigger {
    /// A user message arrived on a CC-channel thread. Equivalent of today's
    /// HTTP chat handler entering the `use_claude_code == Some(true)` branch.
    /// Currently observed in shadow mode — the chat handler still spawns CC.
    UserMessage {
        thread_id: Uuid,
        event_id: Uuid,
        text: String,
    },
    /// User clicked "continue" after a mid-turn engine restart left the thread
    /// in an interrupted state. The spawn re-enters CC with the prior context
    /// and no new prompt. Production-active: the dispatcher pushes a
    /// [`SpawnRequest::Continue`] onto its outbound channel.
    ContinuationRequested { thread_id: Uuid, event_id: Uuid },
}

impl SpawnTrigger {
    pub(crate) fn event_id(&self) -> Uuid {
        match self {
            Self::UserMessage { event_id, .. } | Self::ContinuationRequested { event_id, .. } => {
                *event_id
            }
        }
    }
}

/// Subscribes to the EventBus and dispatches CC spawns based on classified
/// triggers. Production-active for `ContinuationRequested`; shadow-only for
/// `UserMessage` (see module-level docs).
pub struct SpawnDispatcher {
    pool: PgPool,
    bus: Arc<EventBus>,
    /// Outbound channel — the dispatcher sends [`SpawnRequest`]s here and a
    /// receiver task on `LucidosEngine` calls `run_direct_agent` for each.
    /// `Unbounded` because dropping a continuation request would leave a
    /// thread stuck "Working" forever; the volume of `ContinuationRequested`s is
    /// inherently bounded by the number of mid-turn-interrupted threads on
    /// startup, not by ambient traffic.
    spawn_tx: UnboundedSender<SpawnRequest>,
    /// Trigger event ids the dispatcher has already acted on (or knows are
    /// already covered by existing CC events). Survives the lifetime of the
    /// dispatcher; restored on startup by `backfill_pending_triggers_on_startup`.
    spawned_for_event: Arc<Mutex<HashSet<Uuid>>>,
    /// Number of times `dispatch_spawn` was invoked (whether actuated or
    /// shadow). Used by tests + observability.
    pub(crate) dispatch_count: Arc<AtomicUsize>,
}

impl SpawnDispatcher {
    pub(crate) fn new(
        pool: PgPool,
        bus: Arc<EventBus>,
        spawn_tx: UnboundedSender<SpawnRequest>,
    ) -> Self {
        Self {
            pool,
            bus,
            spawn_tx,
            spawned_for_event: Arc::new(Mutex::new(HashSet::new())),
            dispatch_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Test/observability accessor — number of dispatch attempts (gated or not).
    #[allow(dead_code)]
    pub(crate) fn dispatch_count(&self) -> usize {
        self.dispatch_count.load(Ordering::SeqCst)
    }

    /// Build dispatcher and start the run loop on a tokio task. Returns the
    /// receiver end of the `SpawnRequest` channel so the caller can wire it
    /// to the engine-side actuator.
    pub fn spawn(
        pool: PgPool,
        bus: Arc<EventBus>,
    ) -> (
        tokio::task::JoinHandle<()>,
        tokio::sync::mpsc::UnboundedReceiver<SpawnRequest>,
    ) {
        let (spawn_tx, spawn_rx) = tokio::sync::mpsc::unbounded_channel();
        let dispatcher = Self::new(pool, bus, spawn_tx);
        let handle = tokio::spawn(async move {
            // Best-effort backfill — log and continue if it fails so a
            // partial DB state can't permanently disable the dispatcher.
            if let Err(e) = dispatcher.backfill_pending_triggers_on_startup().await {
                log!(
                    "[SpawnDispatcher] backfill_pending_triggers_on_startup failed: {}",
                    e
                );
            }
            dispatcher.run().await;
        });
        (handle, spawn_rx)
    }

    /// Subscribe to the bus and dispatch spawns for each classified trigger.
    /// Lives for the duration of the engine process.
    pub(crate) async fn run(self) {
        let rx = self.bus.subscribe();
        log!("[SpawnDispatcher] starting (Continue=actuating, UserMessage=shadow)");
        let stream = BroadcastStream::new(rx);
        tokio::pin!(stream);
        while let Some(item) = stream.next().await {
            let emitted = match item {
                Ok(e) => e,
                Err(e) => {
                    // Lagged consumer — log and continue. The dispatcher's
                    // idempotency set + DB backfill protect against missed
                    // triggers on the next iteration.
                    log!("[SpawnDispatcher] broadcast lag: {}", e);
                    continue;
                }
            };
            let Some(trigger) = self.classify_trigger(&emitted.typed, emitted.event_id) else {
                continue;
            };
            if self.already_spawned(trigger.event_id()).await {
                continue;
            }
            self.dispatch_spawn(trigger).await;
        }
        log!("[SpawnDispatcher] stream ended");
    }

    /// Recognize the events that should produce a CC spawn. Returns `None` for
    /// every other event so the consumer loop stays cheap.
    pub(crate) fn classify_trigger(
        &self,
        event: &BusEvent,
        event_id: Uuid,
    ) -> Option<SpawnTrigger> {
        let BusEvent::Thread {
            thread_id,
            event,
            meta,
        } = event
        else {
            return None;
        };

        match event {
            ThreadEvent::MessageReceived { text, .. } => {
                // Only CC-channel messages drive the CC dispatcher. Chat-channel
                // messages reach the regular LLM loop and are out of scope.
                let is_cc_channel = meta.channel.as_ref() == Some(&EventChannel::ClaudeCode);
                if !is_cc_channel {
                    return None;
                }
                Some(SpawnTrigger::UserMessage {
                    thread_id: *thread_id,
                    event_id,
                    text: text.clone(),
                })
            }
            ThreadEvent::ContinuationRequested { .. } => Some(SpawnTrigger::ContinuationRequested {
                thread_id: *thread_id,
                event_id,
            }),
            // PermissionAnswer is blocked by Phase 3 (see module docs).
            _ => None,
        }
    }

    /// Returns true when a spawn for this trigger has already happened — either
    /// dispatched by this process (in-memory set) or implied by a subsequent
    /// CC lifecycle event in the DB. The DB check protects against
    /// double-dispatch on engine restart: if the prior process already spawned
    /// CC and the session reached idle, replaying the trigger event from the
    /// bus must not produce a second spawn.
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
                // Cache so subsequent broadcasts skip the DB hit.
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

    /// Mark a trigger as handled and route it according to its kind.
    ///
    /// - `ContinuationRequested`: send a [`SpawnRequest::Continue`] on the outbound
    ///   channel for the engine-side receiver to actuate.
    /// - `UserMessage`: shadow only — log + counter bump. The chat HTTP
    ///   handler still owns spawning for this trigger.
    pub(crate) async fn dispatch_spawn(&self, trigger: SpawnTrigger) {
        self.dispatch_count.fetch_add(1, Ordering::SeqCst);
        self.spawned_for_event
            .lock()
            .await
            .insert(trigger.event_id());

        match trigger {
            SpawnTrigger::ContinuationRequested {
                thread_id,
                event_id,
            } => {
                if let Err(e) = self.spawn_tx.send(SpawnRequest::Continue {
                    thread_id,
                    event_id,
                }) {
                    // Receiver gone → engine is shutting down, or the
                    // receiver task died. Log but don't panic; recovery on
                    // the next engine start will re-emit.
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
            SpawnTrigger::UserMessage {
                thread_id,
                event_id,
                ..
            } => {
                // Shadow: chat handler currently owns this path. Migration
                // out of chat/process.rs is deferred — see module docs and
                // docs/plans/2026-04-24-cc-resume-spike-q7.md.
                log!(
                    "[SpawnDispatcher] shadow-observed UserMessage thread={} event={}; chat handler owns spawning",
                    thread_id,
                    event_id
                );
            }
        }
    }

    /// On startup, prime the in-memory idempotency set with every recent
    /// trigger event whose Claude Code session has already produced a follow-up event.
    /// Without this, the broadcast replay (or events arriving immediately
    /// after subscribe) would re-dispatch every historical user message.
    ///
    /// Today the broadcast channel does NOT replay history — but the dispatcher
    /// can still observe a trigger that was emitted moments before it
    /// subscribed (race window). This backfill is the durable safety net.
    pub(crate) async fn backfill_pending_triggers_on_startup(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Look back over a bounded window — enough to catch anything that
        // could plausibly still be in flight, small enough that a busy
        // workspace doesn't slurp megabytes of history.
        const LOOKBACK_LIMIT: i64 = 1000;

        // For every recent MessageReceived on a CC thread that has any
        // subsequent CC lifecycle event, mark it spawned. The "any subsequent
        // event" predicate is generous: we want to *avoid* re-dispatching;
        // the live event stream will surface anything truly unhandled.
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT e.id FROM events e \
             JOIN thread_summaries t ON t.thread_id = e.aggregate_id::uuid \
             WHERE e.event_type = 'MessageReceived' \
               AND e.aggregate = 'thread' \
               AND t.source = 'claude_code' \
               AND EXISTS ( \
                   SELECT 1 FROM events e2 \
                   WHERE e2.aggregate_id = e.aggregate_id \
                     AND e2.sequence > e.sequence \
                     AND e2.event_type IN ('SessionStarted', 'CodingAgentIdled', \
                                            'SessionEnded', 'CodingAgentToolCalled', \
                                            'CodingAgentTextStreamed', \
                                            'CodingAgentUserMessageSent') \
               ) \
             ORDER BY e.sequence DESC LIMIT $1",
        )
        .bind(LOOKBACK_LIMIT)
        .fetch_all(&self.pool)
        .await?;

        let count = rows.len();
        let mut guard = self.spawned_for_event.lock().await;
        for (id,) in rows {
            guard.insert(id);
        }
        log!(
            "[SpawnDispatcher] backfilled {} already-handled trigger event(s)",
            count
        );
        Ok(())
    }
}

/// Returns true when the events table contains a CC lifecycle event whose
/// sequence is greater than the trigger event's sequence (same thread).
/// Implies the trigger has already been picked up by an earlier dispatcher
/// or the legacy direct-call path.
async fn has_cc_event_after(
    pool: &PgPool,
    trigger_event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(bool,)> = sqlx::query_as(
        "SELECT EXISTS ( \
             SELECT 1 FROM events e2 \
             JOIN events trig ON trig.id = $1 \
             WHERE e2.aggregate_id = trig.aggregate_id \
               AND e2.sequence > trig.sequence \
               AND e2.event_type IN ('SessionStarted', 'CodingAgentIdled', \
                                      'SessionEnded', 'CodingAgentToolCalled', \
                                      'CodingAgentTextStreamed', \
                                      'CodingAgentUserMessageSent') \
         )",
    )
    .bind(trigger_event_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(b,)| b).unwrap_or(false))
}

#[cfg(test)]
#[path = "spawn_dispatcher_tests.rs"]
mod tests;
