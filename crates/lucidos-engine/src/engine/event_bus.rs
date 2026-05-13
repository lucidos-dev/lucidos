//! EventBus — single emission point for all domain events.
//!
//! Producers call typed methods (emit_thread, emit_notification, etc.).
//! The bus persists the event, updates projections, and broadcasts to consumers.
//! Consumers (SSE, memory indexer, etc.) subscribe independently.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use crate::engine::thread_events::{ChildCompletionStatus, EventMeta, ThreadEvent};
use crate::engine::thread_lifecycle::{self, ArchiveState, ThreadType};

/// DB row from thread_summaries for child-to-parent fan-out:
/// (parent_thread_id, is_cc, title, first_message, parent_callback_sent).
type ChildSummaryRow = (Option<Uuid>, bool, Option<String>, Option<String>, bool);

/// Status expression used by every "response/session done" projection: the
/// thread goes 'waiting' iff the CC session left pending changes to review,
/// otherwise 'idle'. CodingAgentIdled binds the value as $2 (it's also being
/// written in the same query); the rest read the stored cc_has_changes.
pub(super) const STATUS_FROM_CC_HAS_CHANGES: &str =
    "CASE WHEN cc_has_changes THEN 'waiting' ELSE 'idle' END";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// What flows through the broadcast channel. Consumers match on `typed`.
#[derive(Clone, Debug)]
pub struct EmittedEvent {
    /// Event UUID — always present. For persisted events this is the DB primary key;
    /// for transient events a fresh UUID is generated for SSE correlation.
    pub event_id: Uuid,
    /// DB sequence number (None for transient events).
    pub seq: Option<i64>,
    /// When the event was created.
    pub created: DateTime<Utc>,
    /// Typed event — consumers match on the variant.
    pub typed: BusEvent,
    /// Post-event projection snapshot. Set for persisted Thread events
    /// (fetched in-tx after the projection update). `None` for transient
    /// Thread events, System events, and child-count broadcasts — those
    /// don't represent a state delta the frontend needs to apply.
    pub aggregate: Option<crate::core::store::ThreadAggregate>,
}

/// Typed union of all aggregate events.
// `Thread` carries a fat `ThreadEvent` payload; boxing would touch every emit
// site for a marginal cache win on a struct that's already cheap to clone.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum BusEvent {
    /// Thread-scoped event (persisted or transient, determined by event.is_persisted()).
    Thread {
        thread_id: Uuid,
        event: ThreadEvent,
        meta: EventMeta,
    },
    /// System/global event (aggregate identity on the event itself).
    System(SystemEvent),
}

#[path = "event_bus_system_event.rs"]
mod system_event;
pub use system_event::SystemEvent;

impl EmittedEvent {
    /// Convert to SSE-compatible JSON string.
    /// Thread events use `{ "type": "ThreadEvent", "data": { thread_id, seq?, event } }`.
    /// System events serialize directly via serde `#[serde(tag = "type", content = "data")]`.
    pub fn to_sse_json(&self) -> String {
        let json = match &self.typed {
            BusEvent::Thread {
                thread_id,
                event,
                meta,
            } => {
                let mut event_json = serde_json::to_value(event).unwrap_or_default();
                // Merge EventMeta fields (channel, request_event_id, etc.) into the
                // event JSON so SSE consumers see the same shape as DB-loaded events.
                if let Some(obj) = event_json.as_object_mut() {
                    meta.apply(obj);
                }
                let mut data = serde_json::json!({
                    "thread_id": thread_id.to_string(),
                    "event": event_json,
                    "created": self.created.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                });
                if let Some(seq) = self.seq {
                    data["seq"] = serde_json::json!(seq);
                }
                data["event_id"] = serde_json::json!(self.event_id.to_string());
                if let Some(agg) = &self.aggregate {
                    if let Ok(agg_json) = serde_json::to_value(agg) {
                        data["aggregate"] = agg_json;
                    }
                }
                serde_json::json!({ "type": "ThreadEvent", "data": data })
            }
            BusEvent::System(SystemEvent::DomainEvent {
                event_type,
                payload,
                ..
            }) => {
                // Domain events are user-defined at runtime, so they live inside a
                // wrapper variant in Rust. On the wire we unwrap to the inner type so
                // the frontend can dispatch by the actual event name (e.g. the SDK's
                // `lucidos.sse.on('SlidePresenterState', ...)` matches the producer's
                // `emit_event('SlidePresenterState', payload)`).
                serde_json::json!({ "type": event_type, "data": payload })
            }
            BusEvent::System(event) => serde_json::to_value(event).unwrap_or_default(),
        };
        json.to_string()
    }
}

// ---------------------------------------------------------------------------
// EventBus
// ---------------------------------------------------------------------------

/// Result of emitting a persisted event.
pub struct EmitResult {
    /// UUID of the persisted event row.
    pub event_id: Uuid,
    /// Auto-assigned DB sequence number.
    pub seq: i64,
}

/// Wake-up signal the child→parent fan-in fires when a child thread reaches
/// a terminal event. The typed `ChildThreadCompleted` row on the parent's
/// history holds all semantic content; this struct carries only the
/// identifiers needed to drive the parent's run-loop entry and stamp the
/// resulting response panel's `request_event_id`.
#[derive(Debug)]
pub struct ParentCallback {
    pub parent_thread_id: Uuid,
    pub child_thread_id: Uuid,
    pub child_completed_event_id: Uuid,
}

/// Trait for emitting domain events. Extracted from `EventBus` to allow
/// mock implementations in tests.
#[async_trait]
pub trait EventBusEmitter: Send + Sync {
    async fn emit(
        &self,
        event: BusEvent,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>>;
}

/// Channel capacity for the event broadcast.
const BUS_CAPACITY: usize = 4096;

#[derive(Clone)]
pub struct EventBus {
    pool: PgPool,
    event_tx: broadcast::Sender<EmittedEvent>,
    parent_callback_tx: mpsc::UnboundedSender<ParentCallback>,
    changes_projection: crate::core::changes_projection::ChangesProjection,
}

impl EventBus {
    pub fn new(pool: PgPool) -> (Self, mpsc::UnboundedReceiver<ParentCallback>) {
        let (event_tx, _) = broadcast::channel(BUS_CAPACITY);
        let (parent_callback_tx, parent_callback_rx) = mpsc::unbounded_channel();
        let changes_projection =
            crate::core::changes_projection::ChangesProjection::new(pool.clone());
        (
            Self {
                pool,
                event_tx,
                parent_callback_tx,
                changes_projection,
            },
            parent_callback_rx,
        )
    }

    pub fn changes_projection(&self) -> &crate::core::changes_projection::ChangesProjection {
        &self.changes_projection
    }

    /// Subscribe to all events. Returns a receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<EmittedEvent> {
        self.event_tx.subscribe()
    }

    /// Get a clone of the sender (for passing to consumers that need to check capacity, etc.)
    pub fn sender(&self) -> broadcast::Sender<EmittedEvent> {
        self.event_tx.clone()
    }

    // ---- Shared persistence ----

    /// Persist an event to the events table. Returns (event_id, sequence).
    async fn persist(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event_id: Uuid,
        aggregate: &str,
        aggregate_id: &str,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> Result<i64, sqlx::Error> {
        let seq: i64 = sqlx::query_scalar(
            r#"INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id)
               VALUES ($1, $2, $3, $4, $5, NOW(),
                       CASE WHEN $2 = 'thread' THEN $3::uuid ELSE NULL END)
               RETURNING sequence"#,
        )
        .bind(event_id)
        .bind(aggregate)
        .bind(aggregate_id)
        .bind(event_type)
        .bind(payload)
        .fetch_one(&mut **tx)
        .await?;

        Ok(seq)
    }

    // ---- Unified emit ----

    /// Emit an event and return only the persisted event_id, swallowing emit
    /// errors. Use when callers want the id (e.g. to record which `ToolCalled`
    /// triggered a spawn) but don't want to short-circuit on a bus failure.
    pub async fn emit_for_id(&self, event: BusEvent) -> Option<Uuid> {
        self.emit(event).await.ok().flatten().map(|r| r.event_id)
    }

    /// Emit and log on failure. Use when the caller wants observability for
    /// emit failures but cannot meaningfully recover (e.g. background
    /// broadcasts, projection updates after the primary work has succeeded).
    /// `ctx` should identify the call site, e.g. `"[ChangeOps] ChangeApplied"`.
    pub async fn emit_or_log(&self, event: BusEvent, ctx: &str) {
        if let Err(e) = self.emit(event).await {
            log!("[EventBus] {} emit failed: {}", ctx, e);
        }
    }

    /// Single entry point for all events.
    /// Persistence is determined by the event's `is_persisted()` method.
    pub async fn emit(
        &self,
        event: BusEvent,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
        match &event {
            BusEvent::Thread {
                thread_id,
                event: te,
                meta,
            } => {
                if te.is_persisted() {
                    let event_id = meta.event_id.unwrap_or_else(Uuid::new_v4);
                    let mut tx = self.pool.begin().await?;

                    // Validate request_event_id exists in the DB. Orphaned references
                    // cause stuck threads when the frontend can't group events into
                    // exchanges. Log loudly so callers fix their origin_id handling.
                    if let Some(ref req_id) = meta.request_event_id {
                        let exists: bool =
                            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE id = $1)")
                                .bind(req_id)
                                .fetch_one(&mut *tx)
                                .await
                                .unwrap_or(false);
                        if !exists {
                            crate::log!(
                                "[EventBus] WARNING: request_event_id {} does not exist in events table \
                                 (event_type={}, thread_id={}). This causes orphaned event references.",
                                req_id, te.event_type(), thread_id
                            );
                        }
                    }

                    let seq = self
                        .persist(
                            &mut tx,
                            event_id,
                            "thread",
                            &thread_id.to_string(),
                            te.event_type(),
                            &te.to_payload(meta),
                        )
                        .await?;
                    let side_effects = self
                        .update_thread_projection(&mut tx, *thread_id, te, meta)
                        .await?;
                    // Read-your-write within the same tx so the snapshot reflects
                    // exactly this event's post-state — no race with a concurrent
                    // emit for the same thread committing between the projection
                    // update and a post-commit fetch. A failed fetch logs and
                    // broadcasts without aggregate (frontend tolerates absence
                    // with a warning, but it indicates a backend bug).
                    let aggregate =
                        match crate::core::store::fetch_thread_aggregate(&mut *tx, *thread_id)
                            .await
                        {
                            Ok(agg) => agg,
                            Err(e) => {
                                crate::log!(
                                    "[EventBus] Failed to fetch ThreadAggregate for {}: {}",
                                    thread_id,
                                    e
                                );
                                None
                            }
                        };
                    tx.commit().await?;
                    let broadcast_created = Utc::now();

                    // Capture what notify_parent_if_child needs before event is moved
                    let notify_thread_id = *thread_id;
                    let notify_event = te.clone();

                    let _ = self.event_tx.send(EmittedEvent {
                        event_id,
                        seq: Some(seq),
                        created: broadcast_created,
                        typed: event,
                        aggregate,
                    });
                    // Run after broadcast so a panic here can't skip SSE delivery
                    self.notify_parent_if_child(notify_thread_id, &notify_event)
                        .await;
                    // If a child was just created, notify the parent with updated counts
                    if let ThreadEvent::MessageReceived {
                        parent_thread_id: Some(pid),
                        ..
                    } = &notify_event
                    {
                        self.broadcast_children_count(*pid).await;
                    }
                    // Side-effect events run in their own transactions, after the
                    // main commit. Section changes are NOT among them — the
                    // per-event aggregate already carries the post-projection
                    // section to subscribers, no follow-up broadcast required.
                    for effect in side_effects {
                        if let Err(e) = Box::pin(self.emit(effect)).await {
                            crate::log!("[EventBus] Side-effect emit failed: {}", e);
                        }
                    }
                    Ok(Some(EmitResult { event_id, seq }))
                } else {
                    let _ = self.event_tx.send(EmittedEvent {
                        event_id: Uuid::new_v4(),
                        seq: None,
                        created: Utc::now(),
                        typed: event,
                        aggregate: None,
                    });
                    Ok(None)
                }
            }
            BusEvent::System(se) => {
                if se.is_persisted() {
                    let event_id = Uuid::new_v4();
                    let stored_event_type = match &se {
                        SystemEvent::DomainEvent { event_type, .. } => event_type.as_str(),
                        _ => se.event_type(),
                    };
                    let mut tx = self.pool.begin().await?;
                    let seq = self
                        .persist(
                            &mut tx,
                            event_id,
                            se.aggregate(),
                            &se.aggregate_id(),
                            stored_event_type,
                            &se.to_payload(),
                        )
                        .await?;
                    self.update_system_projection(&mut tx, event_id, se).await?;
                    tx.commit().await?;

                    let _ = self.event_tx.send(EmittedEvent {
                        event_id,
                        seq: Some(seq),
                        created: Utc::now(),
                        typed: event,
                        aggregate: None,
                    });
                    Ok(Some(EmitResult { event_id, seq }))
                } else {
                    // Transient system events still drive projections (e.g.
                    // thread_presence). The events table is intentionally
                    // skipped — these are high-churn and not interesting to
                    // replay. Skip the SSE broadcast when the projection
                    // reports no real change (e.g. ThreadFocused heartbeats
                    // every 30s from the frontend).
                    let should_broadcast = self.update_transient_system_projection(se).await?;
                    if should_broadcast {
                        let _ = self.event_tx.send(EmittedEvent {
                            event_id: Uuid::new_v4(),
                            seq: None,
                            created: Utc::now(),
                            typed: event,
                            aggregate: None,
                        });
                    }
                    Ok(None)
                }
            }
        }
    }

    /// Project transient system events that maintain external state (without
    /// being persisted to the events table). Returns `true` when callers
    /// should still broadcast the event to SSE consumers, `false` when the
    /// projection update was a no-op (heartbeat refresh, redundant unfocus).
    /// Variants without a projection always broadcast.
    async fn update_transient_system_projection(
        &self,
        event: &SystemEvent,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        match event {
            SystemEvent::ThreadFocused {
                thread_id,
                device_id,
            } => {
                crate::core::ThreadPresenceStore::record_focused(&self.pool, device_id, *thread_id)
                    .await
            }
            SystemEvent::ThreadUnfocused {
                thread_id,
                device_id,
            } => {
                crate::core::ThreadPresenceStore::record_unfocused(
                    &self.pool, device_id, *thread_id,
                )
                .await
            }
            _ => Ok(true),
        }
    }

    // ---- Parent callback ----

    /// Send a ChildrenCountChanged transient event to the parent thread's SSE channel.
    /// `aggregate` carries any other projection changes (e.g. archive_state) the
    /// caller made before emitting — the frontend overlays it onto thread.meta.
    fn send_children_count_event(
        &self,
        parent_id: Uuid,
        active: i64,
        total: i64,
        aggregate: Option<crate::core::store::ThreadAggregate>,
    ) {
        let _ = self.event_tx.send(EmittedEvent {
            event_id: Uuid::new_v4(),
            seq: None,
            created: Utc::now(),
            typed: BusEvent::Thread {
                thread_id: parent_id,
                event: ThreadEvent::ChildrenCountChanged { active, total },
                meta: EventMeta::default(),
            },
            aggregate,
        });
    }

    /// Query children counts from DB and broadcast to the parent thread's SSE channel.
    async fn broadcast_children_count(&self, parent_id: Uuid) {
        let counts: Option<(i64, i64)> = match sqlx::query_as(
            "SELECT active_children_count::bigint, total_children_count::bigint FROM thread_summaries WHERE thread_id = $1"
        )
        .bind(parent_id)
        .fetch_optional(&self.pool)
        .await {
            Ok(row) => row,
            Err(e) => {
                crate::log!("[EventBus] Failed to query children counts for {}: {}", parent_id, e);
                return;
            }
        };
        if let Some((active, total)) = counts {
            self.send_children_count_event(parent_id, active, total, None);
        }
    }

    /// Handle parent notification when a child thread emits a terminal event.
    /// Decrements the parent's `active_children_count` and, for completion events,
    /// sends a callback message with results.
    async fn notify_parent_if_child(&self, child_thread_id: Uuid, event: &ThreadEvent) {
        // Cancel = user-driven, terminal. Abort splits on `AbortCause::is_transient`:
        // EngineShutdown / RecoveryAfterRestart are mid-retry (no decrement, no
        // callback — the resumed child's eventual idle would be orphaned);
        // SafetyNet / ProcessKilled / Unknown are terminal (decrement so the
        // parent doesn't display as Active forever, but no card — the user
        // already sees the child's error state). Same `is_transient` shape as
        // `SessionEnded { reason }` below.
        let is_terminal = match event {
            ThreadEvent::CodingAgentIdled { .. }
            | ThreadEvent::ResponseGenerated { .. }
            | ThreadEvent::ResponseFailed { .. }
            | ThreadEvent::ResponseCanceled { .. } => true,
            ThreadEvent::ResponseAborted { cause, .. } => !cause.is_transient(),
            ThreadEvent::SessionEnded { reason } => !reason.is_transient(),
            _ => false,
        };
        if !is_terminal {
            return;
        }

        // Look up parent, child info, CC status, and whether callback was already sent
        let row: Option<ChildSummaryRow> = match sqlx::query_as::<_, ChildSummaryRow>(
            "SELECT parent_thread_id, is_cc, title, first_message, parent_callback_sent FROM thread_summaries WHERE thread_id = $1"
        )
        .bind(child_thread_id)
        .fetch_optional(&self.pool)
        .await {
            Ok(Some(row)) => Some(row),
            Ok(None) => return,
            Err(e) => {
                crate::log!("[FanOut] Failed to look up parent for child {}: {}", child_thread_id, e);
                return;
            }
        };

        let Some((Some(parent_id), is_cc, title, first_msg, callback_already_sent)) = row else {
            return;
        };

        // CC threads can emit CodingAgentIdled multiple times (initial work,
        // auto-harden, background agents). Only process the first one —
        // subsequent idles should not decrement the counter again or send
        // duplicate callbacks to the parent.
        if is_cc && callback_already_sent && matches!(event, ThreadEvent::CodingAgentIdled { .. }) {
            return;
        }

        // CC sessions can terminate without ever emitting CodingAgentIdled or
        // SessionEnded — e.g. the user cancels and the session sits archived,
        // leaving only ResponseCanceled (or a terminal-cause ResponseAborted
        // from a SafetyNet / ProcessKilled crash) as the signal. The
        // `!callback_already_sent` guard collapses multiple terminal events
        // for the same child to a single decrement. Transient aborts (engine
        // shutdown, recovery) already early-returned via `is_terminal`.
        let should_decrement = if is_cc {
            matches!(event, ThreadEvent::CodingAgentIdled { .. })
                || (!callback_already_sent
                    && matches!(
                        event,
                        ThreadEvent::SessionEnded { .. }
                            | ThreadEvent::ResponseCanceled { .. }
                            | ThreadEvent::ResponseAborted { .. }
                    ))
        } else {
            matches!(
                event,
                ThreadEvent::ResponseGenerated { .. }
                    | ThreadEvent::ResponseFailed { .. }
                    | ThreadEvent::ResponseCanceled { .. }
                    | ThreadEvent::ResponseAborted { .. }
                    | ThreadEvent::SessionEnded { .. }
            )
        };
        // Completion events trigger a callback to the parent (typed
        // ChildThreadCompleted on the parent thread) and surface the parent
        // to inbox. ResponseCanceled is included so the parent sees a
        // "Canceled" card and the LLM learns the child was stopped.
        // ResponseAborted is NOT — the user already sees the child's error
        // state (SafetyNet/ProcessKilled), and engine-shutdown aborts are
        // transient (and were filtered out above). For CC children,
        // SessionEnded also counts when no prior callback was sent — handles
        // CC sessions that end without ever idling.
        let should_callback = matches!(
            (is_cc, event),
            (true, ThreadEvent::CodingAgentIdled { .. })
                | (false, ThreadEvent::ResponseGenerated { .. })
                | (_, ThreadEvent::ResponseFailed { .. })
                | (_, ThreadEvent::ResponseCanceled { .. })
        ) || (is_cc
            && !callback_already_sent
            && matches!(event, ThreadEvent::SessionEnded { .. }));

        // Decrement-only paths must still mark the child or a follow-up event
        // (CodingAgentIdled, SessionEnded) re-decrements via the
        // `!callback_already_sent` gate above. The should_callback path marks
        // after the typed-event emit succeeds; abort never emits, so mark here.
        // Non-CC chat children emit exactly one terminator per request (the
        // agentic loop's `has_terminator` guard), so they need no marker; CC
        // children can have multiple terminal events for the same turn.
        let mark_callback_for_terminal_abort = should_decrement
            && is_cc
            && matches!(event, ThreadEvent::ResponseAborted { .. });

        if should_decrement || should_callback {
            self.update_parent_after_child_terminal(
                parent_id,
                should_decrement,
                should_callback,
            )
            .await;
        }

        if mark_callback_for_terminal_abort {
            self.mark_parent_callback_sent(child_thread_id).await;
        }

        if !should_callback {
            return;
        }

        let label = title
            .or_else(|| first_msg.map(|m| m.chars().take(80).collect()))
            .unwrap_or_else(|| "unknown task".into());

        // Fetch the child thread's last response text — becomes the
        // `summary` field on the typed event (and the failure-error path
        // overrides it below). 2000-char cap mirrors the previous prose
        // path's truncation.
        let last_response: Option<String> = sqlx::query_scalar(
            "SELECT payload->>'text' FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseGenerated' \
             AND payload->>'text' IS NOT NULL AND payload->>'text' != '' \
             ORDER BY created DESC LIMIT 1",
        )
        .bind(child_thread_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .unwrap_or_else(|e| {
            crate::log!(
                "[FanOut] Failed to fetch child response for {}: {}",
                child_thread_id,
                e
            );
            None
        });

        // Per-status caps differ deliberately: success / no-changes / canceled
        // summaries come from a real ResponseGenerated.text (or partial text)
        // the orchestrator may want most of (2000 chars). Failure summaries
        // come from ResponseFailed.error which is often a panic / stack trace
        // and should never dominate the parent's context — re-cap to 200 chars
        // (matching the pre-Phase-4 prose path) before truncation.
        const SUCCESS_SUMMARY_CAP: usize = 2000;
        const FAILURE_SUMMARY_CAP: usize = 200;
        let (status, summary, cap) = match event {
            ThreadEvent::CodingAgentIdled {
                has_changes: true, ..
            } => (
                ChildCompletionStatus::Success,
                last_response.clone().unwrap_or_default(),
                SUCCESS_SUMMARY_CAP,
            ),
            ThreadEvent::CodingAgentIdled {
                has_changes: false, ..
            } => (
                ChildCompletionStatus::NoChanges,
                last_response.clone().unwrap_or_default(),
                SUCCESS_SUMMARY_CAP,
            ),
            ThreadEvent::ResponseGenerated { .. } => (
                ChildCompletionStatus::Success,
                last_response.clone().unwrap_or_default(),
                SUCCESS_SUMMARY_CAP,
            ),
            ThreadEvent::ResponseFailed { error } => (
                ChildCompletionStatus::Failure,
                error.clone(),
                FAILURE_SUMMARY_CAP,
            ),
            // User-driven cancel — surface to parent so the LLM learns the
            // child was stopped (and the UI shows a Canceled card). Pull the
            // partial-stream text from ResponseCanceled itself; falling back
            // to last_response would surface a *prior* turn's text and read
            // as if the canceled turn had completed.
            ThreadEvent::ResponseCanceled { text, .. } => (
                ChildCompletionStatus::Canceled,
                text.clone(),
                SUCCESS_SUMMARY_CAP,
            ),
            // Defensive: any other terminal event slipping through still
            // produces a typed completion so the parent isn't left waiting.
            _ => (
                ChildCompletionStatus::Success,
                last_response.clone().unwrap_or_default(),
                SUCCESS_SUMMARY_CAP,
            ),
        };

        let summary = {
            if summary.len() > cap {
                let cut = summary.floor_char_boundary(cap);
                let mut s = summary[..cut].to_string();
                s.push_str("… (truncated)");
                s
            } else {
                summary
            }
        };

        // Look up which proposed changes the child left in `pending` state.
        // CC chats that ended with `has_changes: true` typically have one;
        // chat children and `no_changes` idles have zero. Failures are
        // reported with `pending_change_ids = []` regardless because the
        // worktree state isn't reviewable.
        let pending_change_ids: Vec<String> = if matches!(status, ChildCompletionStatus::Failure) {
            Vec::new()
        } else {
            self.changes_projection
                .pending_for_thread(child_thread_id)
                .await
                .into_iter()
                .map(|c| c.id.to_string())
                .collect()
        };

        // Emit the typed source-of-truth event onto the parent thread BEFORE
        // marking the callback sent — that ordering means a crash in between
        // re-fires this whole path on next visit instead of leaving the parent
        // permanently silent. `EventMeta::NONE` because this fan-in is engine
        // orchestration, not user/agent actor.
        let typed_event = ThreadEvent::ChildThreadCompleted {
            child_thread_id,
            child_thread_title: Some(label.clone()),
            status,
            summary,
            pending_change_ids,
        };
        // Box::pin here because `emit_or_log → emit → notify_parent_if_child`
        // is the recursion the compiler flags — the emitted ChildThreadCompleted
        // for the parent will itself walk back through `notify_parent_if_child`
        // (which immediately exits because ChildThreadCompleted isn't a
        // terminal event), but the type-level cycle still needs indirection.
        //
        // If the typed-event emit failed, we MUST NOT mark the callback as
        // sent or fire the wake-up kick — telling the parent's resume path
        // to attribute its response to a non-persisted event would have the
        // parent's UI displaying a phantom card. Returning here means the
        // next terminal event (or recovery sweep) gets another shot at the
        // typed emit; for chat children whose ResponseGenerated fires
        // exactly once that means a permanent miss, accepted as the lesser
        // evil vs. the phantom-event case.
        let emit_result = match Box::pin(self.emit(BusEvent::Thread {
            thread_id: parent_id,
            event: typed_event,
            meta: EventMeta::NONE,
        }))
        .await
        {
            Ok(Some(r)) => r,
            Ok(None) => {
                crate::log!(
                    "[FanOut] ChildThreadCompleted emit returned None for parent {}; \
                     skipping wake-up kick — see notify_parent_if_child error path.",
                    parent_id
                );
                return;
            }
            Err(e) => {
                crate::log!(
                    "[FanOut] Failed to emit ChildThreadCompleted for parent {}: {}; \
                     skipping wake-up kick — driving the parent against a non-persisted \
                     event would attribute its response to a phantom card. Next terminal \
                     event (if any) retries; for chat children with one-shot \
                     ResponseGenerated this means a permanent miss.",
                    parent_id,
                    e
                );
                return;
            }
        };

        self.mark_parent_callback_sent(child_thread_id).await;
        self.send_parent_callback(parent_id, child_thread_id, emit_result.event_id);
    }

    /// Combined into one round-trip + one broadcast so subscribers see the
    /// count change and the section change in the same envelope — replaces the
    /// old separate `ThreadMarkedUnread` side-effect that raced with the
    /// children-count broadcast.
    async fn update_parent_after_child_terminal(
        &self,
        parent_id: Uuid,
        decrement: bool,
        surface_to_inbox: bool,
    ) {
        let dec = if decrement { 1_i64 } else { 0 };
        let new_archive = if surface_to_inbox {
            Some(ArchiveState::Inbox.as_str())
        } else {
            None
        };
        let row: Option<(i64, i64)> = match sqlx::query_as(
            "UPDATE thread_summaries SET \
             active_children_count = GREATEST(0, active_children_count - $2), \
             archive_state = COALESCE($3, archive_state) \
             WHERE thread_id = $1 \
             RETURNING active_children_count::bigint, total_children_count::bigint",
        )
        .bind(parent_id)
        .bind(dec)
        .bind(new_archive)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(opt) => opt,
            Err(e) => {
                crate::log!(
                    "[FanOut] Failed to update parent {} after child terminal: {}",
                    parent_id,
                    e
                );
                return;
            }
        };
        let Some((active, total)) = row else { return };
        let aggregate = match crate::core::store::fetch_thread_aggregate(&self.pool, parent_id)
            .await
        {
            Ok(agg) => agg,
            Err(e) => {
                crate::log!(
                    "[FanOut] Failed to fetch aggregate for parent {}: {}",
                    parent_id,
                    e
                );
                None
            }
        };
        self.send_children_count_event(parent_id, active, total, aggregate);
    }

    async fn mark_parent_callback_sent(&self, child_thread_id: Uuid) {
        if let Err(e) = sqlx::query(
            "UPDATE thread_summaries SET parent_callback_sent = TRUE WHERE thread_id = $1",
        )
        .bind(child_thread_id)
        .execute(&self.pool)
        .await
        {
            crate::log!(
                "[FanOut] Failed to mark callback sent for child {}: {}",
                child_thread_id,
                e
            );
        }
    }

    fn send_parent_callback(
        &self,
        parent_thread_id: Uuid,
        child_thread_id: Uuid,
        child_completed_event_id: Uuid,
    ) {
        if let Err(e) = self.parent_callback_tx.send(ParentCallback {
            parent_thread_id,
            child_thread_id,
            child_completed_event_id,
        }) {
            crate::log!(
                "[FanOut] Failed to send parent callback for child {}: {}",
                child_thread_id,
                e
            );
        }
    }

    // ---- Contract helpers ----

    /// Get the thread type from thread_summaries.
    async fn get_thread_type(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: &Uuid,
    ) -> ThreadType {
        let source: Option<String> =
            sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&mut **tx)
                .await
                .unwrap_or(None);
        if source.as_deref() == Some("claude_code") {
            ThreadType::CodingAgent
        } else {
            ThreadType::Chat
        }
    }

    /// Get the current stored section from thread_summaries.
    async fn get_current_section(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: &Uuid,
    ) -> ArchiveState {
        let section: Option<String> =
            sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&mut **tx)
                .await
                .unwrap_or(None);
        section
            .map(|s| ArchiveState::parse(&s))
            .unwrap_or(ArchiveState::Archived)
    }

    /// Apply a contract transition result to the database. Only effect is the
    /// section update — the per-event aggregate snapshot then carries the new
    /// state to subscribers, so no follow-up section-changing event is emitted.
    async fn apply_transition(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: Uuid,
        result: &thread_lifecycle::TransitionResult,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(new_section) = result.new_section {
            sqlx::query("UPDATE thread_summaries SET archive_state = $1 WHERE thread_id = $2")
                .bind(new_section.as_str())
                .bind(thread_id)
                .execute(&mut **tx)
                .await?;
        }
        Ok(())
    }

}

#[async_trait]
impl EventBusEmitter for EventBus {
    async fn emit(
        &self,
        event: BusEvent,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
        EventBus::emit(self, event).await
    }
}

/// Test-only mock that records emitted events without touching a database.
#[cfg(test)]
pub struct MockEventBus {
    emitted: std::sync::Mutex<Vec<BusEvent>>,
    /// When set, `emit` returns this error instead of `Ok(None)`.
    pub fail_with: std::sync::Mutex<Option<String>>,
}

#[cfg(test)]
impl Default for MockEventBus {
    fn default() -> Self {
        Self {
            emitted: std::sync::Mutex::new(Vec::new()),
            fail_with: std::sync::Mutex::new(None),
        }
    }
}

#[cfg(test)]
impl MockEventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn emitted_events(&self) -> Vec<BusEvent> {
        self.emitted.lock().unwrap().clone()
    }
}

#[cfg(test)]
#[async_trait]
impl EventBusEmitter for MockEventBus {
    async fn emit(
        &self,
        event: BusEvent,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(msg) = self.fail_with.lock().unwrap().as_ref() {
            return Err(msg.clone().into());
        }
        self.emitted.lock().unwrap().push(event);
        Ok(None)
    }
}

#[cfg(test)]
#[path = "event_bus_tests.rs"]
mod tests;

#[path = "event_bus_projection.rs"]
mod projection;
