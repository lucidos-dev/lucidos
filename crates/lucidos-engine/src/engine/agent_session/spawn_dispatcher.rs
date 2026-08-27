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
//! table for triggers already followed by a CC lifecycle event, and
//! `dispatch_spawn` gates the send on atomically claiming the id).
//!
//! Delivery is guaranteed two ways:
//!
//! - **Live path** — [`SpawnDispatcher::spawn`] opens the broadcast
//!   subscription SYNCHRONOUSLY, before the startup backfill runs and before
//!   it returns. A broadcast `Receiver` buffers events from the moment of
//!   `subscribe()`, so a trigger emitted while the backfill query runs (e.g.
//!   `resume_pending_switches()`'s `ContinuationRequested`, which `main.rs`
//!   emits moments after `spawn()` returns) is delivered when `run()` drains
//!   the receiver — never broadcast to zero subscribers and lost.
//! - **Durable floor** — `redispatch_orphaned_continuations_on_startup` scans
//!   the events table for any `ContinuationRequested` that was emitted but
//!   never actuated (no subsequent CC lifecycle or terminal event, thread
//!   still `running`) and re-drives it onto the same durable mpsc channel, so
//!   a request lost by an older engine build — a zombie thread stuck
//!   "running" with no subprocess — heals on the next boot.
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

use crate::engine::event_bus::{BusEvent, EmittedEvent, EventBus};
use crate::engine::thread_events::{EventChannel, ThreadEvent};

/// SQL fragment listing the CC lifecycle events whose presence AFTER a trigger
/// event implies the trigger was already picked up (CC clearly ran in response).
/// Shared by [`has_cc_event_after`], the startup backfill, and the orphaned-
/// continuation scan so the three predicates can't drift apart.
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
/// thread was settled after the request — e.g. by
/// `settle_orphaned_running_coding_agent_threads` — and must stay settled with
/// its manual Continue affordance). Shared by the orphan scan and
/// [`thread_has_unactuated_continuation`].
fn continuation_superseded_events_sql() -> String {
    format!(
        "{CC_LIFECYCLE_EVENTS_SQL}, 'ContinuationRequested', 'ResponseAborted', \
         'ResponseCanceled', 'ResponseFailed', 'ResponseGenerated'"
    )
}

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
    /// A user message arrived on a coding-agent-channel thread. Equivalent of
    /// today's HTTP chat handler entering the `use_coding_agent == Some(true)`
    /// branch.
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
    pub(crate) fn new(pool: PgPool, spawn_tx: UnboundedSender<SpawnRequest>) -> Self {
        Self {
            pool,
            spawn_tx,
            spawned_for_event: Arc::new(Mutex::new(HashSet::new())),
            dispatch_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Build dispatcher and start the run loop on a tokio task. Returns the
    /// receiver end of the `SpawnRequest` channel so the caller can wire it
    /// to the engine-side actuator.
    ///
    /// The broadcast subscription is acquired HERE, synchronously, before this
    /// function returns — NOT lazily inside `run()`. The receiver buffers
    /// events from the moment of `subscribe()`, so a trigger emitted right
    /// after `spawn()` returns (while the seconds-long startup backfill query
    /// is still running) is delivered when `run()` drains the receiver.
    /// Subscribing after the backfill lost exactly that emit — the EventBus
    /// has no history replay, so `resume_pending_switches()`'s
    /// `ContinuationRequested` was broadcast to zero subscribers and the
    /// thread stayed a "running" zombie until a manual Continue.
    pub fn spawn(
        pool: PgPool,
        bus: Arc<EventBus>,
    ) -> (
        tokio::task::JoinHandle<()>,
        tokio::sync::mpsc::UnboundedReceiver<SpawnRequest>,
    ) {
        let (spawn_tx, spawn_rx) = tokio::sync::mpsc::unbounded_channel();
        let rx = bus.subscribe();
        let dispatcher = Self::new(pool, spawn_tx);
        let handle = tokio::spawn(async move {
            // Best-effort backfill — log and continue if it fails so a
            // partial DB state can't permanently disable the dispatcher.
            if let Err(e) = dispatcher.backfill_pending_triggers_on_startup().await {
                log!(
                    "[SpawnDispatcher] backfill_pending_triggers_on_startup failed: {}",
                    e
                );
            }
            // Durable safety net — re-drive any ContinuationRequested that was
            // emitted but never actuated (see the method docs). Best-effort
            // for the same reason as the backfill; the subscription above is
            // the primary delivery path.
            if let Err(e) = dispatcher
                .redispatch_orphaned_continuations_on_startup()
                .await
            {
                log!(
                    "[SpawnDispatcher] redispatch_orphaned_continuations_on_startup failed: {}",
                    e
                );
            }
            dispatcher.run(rx).await;
        });
        (handle, spawn_rx)
    }

    /// Dispatch spawns for each classified trigger arriving on `rx` — the
    /// broadcast subscription [`Self::spawn`] opened before the startup
    /// backfill (so nothing emitted since then is lost). Lives for the
    /// duration of the engine process.
    pub(crate) async fn run(self, rx: tokio::sync::broadcast::Receiver<EmittedEvent>) {
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
            ThreadEvent::ContinuationRequested { .. } => {
                Some(SpawnTrigger::ContinuationRequested {
                    thread_id: *thread_id,
                    event_id,
                })
            }
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
    ///
    /// Atomic mark-and-send: the insert into the idempotency set IS the claim.
    /// A trigger observed by two paths — the startup orphan re-dispatch and
    /// the buffered broadcast drain both see a `ContinuationRequested`
    /// persisted mid-startup — produces at most ONE `SpawnRequest`, whichever
    /// path claims the id first.
    pub(crate) async fn dispatch_spawn(&self, trigger: SpawnTrigger) {
        if !self
            .spawned_for_event
            .lock()
            .await
            .insert(trigger.event_id())
        {
            return;
        }
        self.dispatch_count.fetch_add(1, Ordering::SeqCst);

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
        let rows: Vec<(Uuid,)> = sqlx::query_as(&format!(
            "SELECT e.id FROM events e \
             JOIN thread_summaries t ON t.thread_id = e.aggregate_id::uuid \
             WHERE e.event_type = 'MessageReceived' \
               AND e.aggregate = 'thread' \
               AND t.source = 'claude_code' \
               AND EXISTS ( \
                   SELECT 1 FROM events e2 \
                   WHERE e2.aggregate_id = e.aggregate_id \
                     AND e2.sequence > e.sequence \
                     AND e2.event_type IN ({CC_LIFECYCLE_EVENTS_SQL}) \
               ) \
             ORDER BY e.sequence DESC LIMIT $1"
        ))
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

    /// Startup safety net (durable): re-dispatch every `ContinuationRequested`
    /// that was emitted but never actuated — per thread, the NEWEST request
    /// with no subsequent CC lifecycle event, no later `ContinuationRequested`
    /// (a newer request supersedes older ones), no terminal response event
    /// (a thread something settled after the request must stay settled), and
    /// the projection still showing `status = 'running'` (the zombie
    /// fingerprint: the request's projection arm flips status to running, so
    /// an unactuated request leaves it there).
    ///
    /// Why it exists: the broadcast bus has no history replay, so a request
    /// whose live delivery was lost (emitted before the dispatcher subscribed,
    /// on an engine build predating the synchronous subscribe in
    /// [`Self::spawn`]) left the thread a permanent zombie — `running` in the
    /// projection, no CC subprocess, and nothing ever rechecking it. This scan
    /// is the durable floor under the subscription: each still-live request is
    /// pushed onto the SAME durable mpsc channel as the live path, through the
    /// same [`Self::already_spawned`] guard.
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
               AND t.status = 'running' \
               AND NOT EXISTS ( \
                   SELECT 1 FROM events e2 \
                   WHERE e2.aggregate_id = e.aggregate_id \
                     AND e2.sequence > e.sequence \
                     AND e2.event_type IN ({}) \
               ) \
             ORDER BY e.sequence ASC",
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
        // claim that dedupes against the buffered broadcast drain.
        for (event_id, thread_id) in rows {
            self.dispatch_spawn(SpawnTrigger::ContinuationRequested {
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
               AND t.status = 'running' \
               AND NOT EXISTS ( \
                   SELECT 1 FROM events e2 \
                   WHERE e2.aggregate_id = e.aggregate_id \
                     AND e2.sequence > e.sequence \
                     AND e2.event_type IN ({}) \
               ) \
         )",
        continuation_superseded_events_sql()
    ))
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap_or(false)
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
