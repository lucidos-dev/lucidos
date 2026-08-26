//! EventBus — single emission point for all domain events.
//!
//! Producers call typed methods (`emit_thread`, `emit_notification`, etc.).
//! The bus drives an in-tx pipeline that persists the event, updates projections,
//! and captures a post-projection aggregate snapshot — then commits and broadcasts
//! the snapshot to consumers. Consumers (SSE, memory indexer, etc.) subscribe
//! independently and observe events only after `tx.commit()`.
//!
//! See the [`EventBus`] struct doc-comment for the full two-contract surface:
//! the five-phase in-emit pipeline (Validate → Persist → Project → CaptureAggregate
//! → PostCommit) and the post-commit subscriber contract.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::thread_lifecycle::{self, ArchiveState, ThreadType};
use crate::scheduler::user_tasks::{current_event_trigger_depth, current_trigger_id};

/// CC session-end status — always `'idle'`. A proposed change is a review
/// artifact, not a parked loop, so it does not block the parent's "is my
/// child still working?" rollup. Pending review surfaces via the
/// `coding_agent_proposed` column (and `is_blocking` clause 3) instead.
pub(super) const STATUS_FROM_PROPOSED_CHANGE: &str = "'idle'";

/// The `status` values that are a **verdict about how the turn ended**, rather
/// than a resting state the next event may freely overwrite. Both are written by
/// exactly one place each: `'failed'` by `ResponseFailed` and by
/// `AbortCause::status_sql()`'s fall-through arm, `'paused'` by that same
/// function's `promises_auto_resume` arm (the user's own *Switch to new version*
/// interrupted the turn, and the engine is bringing it back).
///
/// They share this list because they share the hazard: each is emitted at the
/// moment a coding-agent turn dies, and each is followed by that turn's trailing
/// events. See [`preserving_verdict`].
///
/// **These two are the whole set, and that is load-bearing.** A third value
/// escaping into an abort's status write is not a widening of the list, it is a
/// hole in it. `status_sql` used to answer `'waiting'` whenever a change was
/// pending, and the drain then overwrote it within milliseconds. Say the verdict
/// here, and let the reader rank it against a pending change.
pub(super) const PRESERVED_STATUS_VERDICTS: &[&str] = &["failed", "paused"];

/// [`PRESERVED_STATUS_VERDICTS`] as a SQL `IN (...)` body. Every name is a
/// compile-time literal, so there is nothing to parameterize and nothing to
/// escape. Built once: the projection interpolates it on every status write.
pub(super) static PRESERVED_STATUS_VERDICTS_SQL: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        PRESERVED_STATUS_VERDICTS
            .iter()
            .map(|v| format!("'{v}'"))
            .collect::<Vec<_>>()
            .join(",")
    });

/// Wrap a `status` target so a verdict already on the row survives.
///
/// A verdict (see [`PRESERVED_STATUS_VERDICTS`]) is a statement about how the
/// turn ended: it failed, or an engine restart interrupted it. Only a real start
/// event (`MessageReceived`, `CodingAgentUserMessageSent`, `UserPromptInjected`,
/// `CodingAgentPromptSent`, `ContinuationRequested`, …) may clear one, because
/// only those mean new work was actually requested. Every event that merely
/// *closes out* the ended turn has to leave the verdict alone, or the status dot
/// reverts to idle while the interruption or failure still stands.
///
/// This matters far more for coding agents than for the Lucidos Agent, which
/// is why the two channels visibly disagreed before this was applied
/// uniformly. A chat turn's loop emits nothing after its own terminator, so
/// its verdict is never touched. A coding-agent turn emits four more things:
/// the subprocess's trailing drain output, the `ChangeProposed` it commits on
/// its way out, `CodingAgentIdled`, and `SessionEnded`. Each used to write a
/// status and, between them, they walked an interrupted thread back to
/// `'idle'`. The drain is not suppressed by `external_terminal_emitted`, which
/// covers the duplicate terminal rather than the activity stream.
///
/// Not expressible in the coarse `StatusRule::ConditionalCc` model that
/// `thread_lifecycle::status_transitions()` publishes, so the contract table
/// stays as-is and this guard lives in the projection SQL — see the note on
/// `CodingAgentIdled` there. The `terminal_events_never_set_running`
/// invariant is unaffected.
pub(super) fn preserving_verdict(target: &str) -> String {
    format!(
        "CASE WHEN status IN ({verdicts}) THEN status ELSE {target} END",
        verdicts = &*PRESERVED_STATUS_VERDICTS_SQL,
    )
}

/// SET fragment that clears every CC-state column on `thread_summaries` —
/// used identically by `ChangeApplied`, `ChangeDiscarded`, and `ThreadArchived`
/// since the user-facing proposal lifecycle ends in all three. Add new
/// CC-state columns here so the next "terminal proposal event" handler
/// doesn't have to know about them.
pub(super) const CLEAR_CODING_AGENT_FLAGS: &str =
    "coding_agent_proposed = FALSE, coding_agent_requires_restart = FALSE, \
     coding_agent_is_external_repo = FALSE, coding_agent_applying = FALSE, \
     coding_agent_has_diff = FALSE";

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
    /// (fetched in-tx after the projection update) and for a few transient
    /// thread broadcasts that carry out-of-band projection refreshes.
    /// `None` for System events and projection-free transient Thread events.
    pub aggregate: Option<crate::core::store::ThreadAggregate>,
    /// How deep in an event-trigger chain the work that emitted this was
    /// running. Resolved by [`emit_depth`] from the emitting task's scope and
    /// the Thread Queue's per-thread registration, whichever is deeper.
    ///
    /// Broadcast-only, and never persisted: it describes the work, not the
    /// event. The scheduler dispatches a thread event at this depth. That is
    /// what makes the depth cap engage for a chain across triggers, where A's
    /// fire wakes B and B's fire wakes A. It reaches a chain routed through
    /// spawned work too, unlike `emitting_trigger_id` below. A domain event
    /// carries the same number in its own
    /// variant, where it IS persisted, so a replay reconstructs it.
    pub depth: u32,
    /// Which trigger's fire emitted this, if any. Read from
    /// `scheduler::user_tasks::current_trigger_id`, or stated outright by a
    /// caller of [`EventBus::emit_as_trigger`].
    ///
    /// Broadcast-only and never persisted: it describes the emitting task, not
    /// the event, so it is absent from the stored row and from every replay
    /// path. The scheduler drops this trigger from the matches, so a trigger is
    /// never woken by an event its own fire emitted. Absent means no
    /// suppression, which is the direction to fail in: an extra wake, never a
    /// missing one. See `docs/adr/0137-a-trigger-never-wakes-itself.md`.
    pub emitting_trigger_id: Option<String>,
}

/// The chain depth an event belongs to, resolved from both carriers.
///
/// A thread event has two: the emitting task's scope, and the registration the
/// Thread Queue made for that thread. They disagree whenever a task is too deep
/// in a spawn tree for the scope to reach it, which is most of a coding-agent
/// session. Take the deeper of the two.
///
/// The direction matters. Under-counting re-opens the loop the cap exists to
/// end, and does it silently. Over-counting suppresses a fire, which the cap
/// notification now names for the user. Prefer the recoverable error.
fn emit_depth(event: &BusEvent) -> u32 {
    match event {
        BusEvent::Thread { thread_id, .. } => thread_emit_depth(*thread_id),
        BusEvent::System(_) => current_event_trigger_depth(),
    }
}

/// [`emit_depth`] for a caller that has the thread id but no `BusEvent` yet.
fn thread_emit_depth(thread_id: Uuid) -> u32 {
    let registered =
        crate::scheduler::user_tasks::chain_depth_for_thread(thread_id).unwrap_or_default();
    current_event_trigger_depth().max(registered)
}

/// Arguments to [`EventBus::replay_historical_event`]. Fields mirror the
/// `events` table schema 1:1; named-field construction keeps the call site
/// self-documenting.
pub(crate) struct HistoricalReplay<'a> {
    pub event_id: Uuid,
    pub aggregate: &'a str,
    pub aggregate_id: &'a str,
    pub event_type: &'a str,
    pub payload: &'a serde_json::Value,
    pub thread_id: Option<Uuid>,
    pub created: Option<DateTime<Utc>>,
    /// Whether subscribers hear about the row. `true` is the default a
    /// backfill wants: SSE, the memory indexer and everything else observe
    /// the replay as they would a live emit.
    ///
    /// `false` for a bulk pass writing rows nothing needs to see arrive.
    /// `core::aux_context_backfill` writes tens of thousands of accounting
    /// rows. A burst that size buys no client anything, and would flood every
    /// open SSE connection on engine start.
    pub broadcast: bool,
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

#[path = "../event_bus_system_event.rs"]
mod system_event;
pub use system_event::SystemEvent;

mod parent_callback;

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
            BusEvent::System(domain @ SystemEvent::DomainEvent { event_type, .. }) => {
                // Domain events are user-defined at runtime, so they live inside a
                // wrapper variant in Rust. On the wire we unwrap to the inner type so
                // the frontend can dispatch by the actual event name (e.g. the SDK's
                // `lucidos.sse.on('SlidePresenterState', ...)` matches the producer's
                // `emit_event('SlidePresenterState', payload)`). `to_payload`
                // injects `actor` (when set) so SSE consumers see the same shape
                // as a DB-loaded event.
                serde_json::json!({ "type": event_type, "data": domain.to_payload() })
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
    /// The child's own terminal event, the one that drove this fan-in.
    ///
    /// Load-bearing for the stand-down gate in
    /// `notify_parent_of_child_completion`. An OR-wait naming both
    /// `CodingAgentIdled` and `ChildThreadCompleted` for one child resolves on
    /// whichever arm lands first. The delivery therefore routinely names THIS
    /// id rather than the completion card's. `None` on the boot re-fire sweep,
    /// which reads the card and not the terminal behind it. Absent means the
    /// probe abstains, and the wake is sent.
    pub child_terminal_event_id: Option<Uuid>,
    /// Parent's `is_coding_agent` flag — captured here on the existing
    /// `notify_parent_if_child` query (self-join in `thread_summaries`) so
    /// the FanOut consumer doesn't need a second roundtrip just to pick the
    /// CC vs chat routing fork.
    pub parent_is_coding_agent: bool,
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

/// Single entry point for every domain event. Carries two distinct contracts:
///
/// # In-emit pipeline (5 phases, in this order)
///
/// Every persisted-thread `emit()` runs the same 5-phase pipeline. Each phase
/// is marked in source with a `// === Phase: <Name> ===` banner; the
/// `emit_pipeline_has_five_named_phases_in_order` test pins the ordering.
///
/// 1. **Validate** — pre-INSERT checks (e.g. `request_event_id` existence) that
///    read the same tx snapshot the upcoming Persist will use. May short-circuit
///    with `Err` before any events row is written, or with `Ok(None)` for the
///    `ChangeApplied` single-fire guard, which `FOR UPDATE`-locks the change row
///    and drops a duplicate apply so one change can't emit two `ChangeApplied`
///    events.
/// 2. **Persist** — INSERT into the `events` table. Assigns the bigserial
///    sequence that drives every downstream consumer's ordering.
/// 3. **Project** — update derived state (`thread_summaries` and friends) in
///    the same tx. Cascades through parent rollup (`propagate_blocking_change`)
///    internally; returns side-effect events to fire in PostCommit and the list
///    of ancestor thread_ids whose `blocking_descendant_count` changed.
/// 4. **CaptureAggregate** — in-tx read-your-write fetches of the emitting
///    thread's aggregate and each affected ancestor's aggregate. Captured here
///    (still inside the tx) so subscribers see snapshots that match exactly
///    this event's post-state — no race with a concurrent emit committing
///    between the projection update and the broadcast.
/// 5. **PostCommit** — commit the tx, then fan out: broadcast on `event_tx` to
///    `subscribe()` consumers, rebroadcast each affected ancestor's aggregate,
///    wake the parent agent session via `parent_callback_tx`, and replay
///    Project's side-effects via recursive `emit()` calls.
///
/// New code added inside `emit()` must declare which phase it belongs to by
/// placing it under the matching banner. Phases 1–4 share one transaction —
/// they cannot be moved out without breaking validate-reads-projection or the
/// CaptureAggregate read-your-write guarantee. The system-event arm and
/// transient arms are simplified variants of the thread persisted arm and run
/// the relevant subset (no Validate for system events, no CaptureAggregate for
/// transient events).
///
/// # Post-commit subscriber contract
///
/// Two consumer surfaces, each with a different shape:
///
/// - **Broadcast subscribers** — call [`subscribe()`](EventBus::subscribe) to
///   get a [`broadcast::Receiver<EmittedEvent>`]. Every subscriber observes
///   every event after `tx.commit()`. No ordering between subscribers is
///   guaranteed. Channel capacity is [`BUS_CAPACITY`]; slow subscribers receive
///   `RecvError::Lagged` and must reconcile via a full state refresh.
/// - **Parent callbacks** — directed (not broadcast). The child→parent fan-in
///   sent on `parent_callback_tx` wakes the parent agent session's runtime
///   when a child thread reaches a terminal event. The lone consumer is the
///   `ParentCallback` receiver returned from [`EventBus::new`]; the channel is
///   unbounded.
///
/// The bus does not run projections in subscribers. Projections are part of
/// the write (Phase 3) and must observe read-your-write against the same tx as
/// Validate — moving them to a subscriber would break next-emit validation.
#[derive(Clone)]
pub struct EventBus {
    pool: PgPool,
    /// Broadcast channel for the post-commit subscriber contract — every
    /// `subscribe()` returns a `Receiver` cloned from here. Sent to once per
    /// emit (plus once per affected ancestor for rebroadcast). See struct
    /// doc-comment § Post-commit subscriber contract.
    event_tx: broadcast::Sender<EmittedEvent>,
    /// Directed mpsc for the child→parent fan-in. Distinct from `event_tx` —
    /// broadcast subscribers do NOT see these; the lone consumer is the
    /// `ParentCallback` receiver returned from [`EventBus::new`]. Fired in
    /// PostCommit via `notify_parent_if_child` when a child thread reaches a
    /// terminal event so the parent agent session wakes its run-loop.
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

    /// Subscribe to every post-commit event broadcast on the bus. Returns a
    /// fresh [`broadcast::Receiver<EmittedEvent>`] independent of all other
    /// subscribers. See the [`EventBus`] struct doc-comment § Post-commit
    /// subscriber contract for the visibility and ordering guarantees:
    /// post-`tx.commit()`, post-projection (the `EmittedEvent.aggregate`
    /// reflects the just-committed projection state), no inter-subscriber
    /// ordering, [`BUS_CAPACITY`] slots before `RecvError::Lagged`.
    ///
    /// For directed child→parent wake-ups use the `ParentCallback` receiver
    /// returned from [`EventBus::new`] instead — those do NOT flow through
    /// this broadcast.
    pub fn subscribe(&self) -> broadcast::Receiver<EmittedEvent> {
        self.event_tx.subscribe()
    }

    /// Clone of the underlying [`broadcast::Sender`]. Use when a consumer needs
    /// to inspect channel state (e.g. `receiver_count`, `len`) or hold onto the
    /// sender to keep the channel open across `subscribe()` calls. Same
    /// contract as [`subscribe()`](Self::subscribe).
    pub fn sender(&self) -> broadcast::Sender<EmittedEvent> {
        self.event_tx.clone()
    }

    /// Synchronously broadcast a transient, projection-free system event on the
    /// bus, bypassing the async [`emit`](Self::emit) pipeline.
    ///
    /// Use ONLY for high-frequency transient events emitted from a synchronous
    /// callback where ordering relative to a *subsequent* async `emit` matters.
    /// `BackupProgress` is the canonical case: routing each progress tick
    /// through a detached `tokio::spawn(emit(..))` let a late tick land on the
    /// wire AFTER the terminal `BackupCompleted`/`BackupFailed` that
    /// `run_backup` emits right after — re-setting the frontend's progress
    /// signal the terminal event had just cleared. A direct, in-order
    /// `event_tx.send` keeps progress strictly before the terminal event and
    /// works from both async and `spawn_blocking` contexts (the send is sync).
    ///
    /// Restricted to transient events with NO projection: it skips
    /// [`update_transient_system_projection`](Self::update_transient_system_projection),
    /// so a transient event that maintains projection state (e.g.
    /// `DeviceVisible`/`DeviceHidden`) MUST go through `emit` instead. A send
    /// error (no live subscribers) is ignored, matching `emit`'s transient arm.
    pub fn broadcast_transient_system(&self, event: SystemEvent) {
        debug_assert!(
            !event.is_persisted(),
            "broadcast_transient_system is for transient (non-persisted) system events only"
        );
        let _ = self.event_tx.send(EmittedEvent {
            event_id: Uuid::new_v4(),
            seq: None,
            created: Utc::now(),
            typed: BusEvent::System(event),
            aggregate: None,
            depth: current_event_trigger_depth(),
            emitting_trigger_id: current_trigger_id(),
        });
    }

    /// Reconcile the live git-diff flag and broadcast the updated aggregate if
    /// the flag changed.
    ///
    /// Used by the coding-agent worktree's post-commit hook. This intentionally
    /// updates only `coding_agent_has_diff`: the formal proposal lifecycle
    /// (`coding_agent_proposed`, `changes` row, Apply button) remains owned by
    /// `ChangeProposed` at coding-agent idle.
    pub async fn set_coding_agent_has_diff_and_broadcast(
        &self,
        thread_id: Uuid,
        has_diff: bool,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE thread_summaries \
             SET coding_agent_has_diff = $2 \
             WHERE thread_id = $1 \
               AND coding_agent_has_diff IS DISTINCT FROM $2",
        )
        .bind(thread_id)
        .bind(has_diff)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(false);
        }

        let aggregate = crate::core::store::fetch_thread_aggregate(&mut *tx, thread_id).await?;
        tx.commit().await?;

        if let Some(aggregate) = aggregate {
            let _ = self.event_tx.send(EmittedEvent {
                event_id: Uuid::new_v4(),
                seq: None,
                created: Utc::now(),
                typed: BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::CodingAgentDiffChanged { has_diff },
                    meta: EventMeta::NONE,
                },
                aggregate: Some(aggregate),
                depth: thread_emit_depth(thread_id),
                emitting_trigger_id: current_trigger_id(),
            });
        }

        Ok(true)
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

    /// Insert a backfilled / migrated event directly into the events table and
    /// broadcast it on the bus, **without** running projections or the
    /// request_event_id foreign-key check.
    ///
    /// Use ONLY for historical replay (one-shot backfills from legacy schemas,
    /// data migrations) where the row is a derived fact about events that
    /// already happened — the projection update would either be a no-op (no
    /// thread state delta) or actively wrong (the historical event happened
    /// long ago, applying it to today's projection state would corrupt it).
    /// The deterministic id supplied by the caller plus `ON CONFLICT (id) DO
    /// NOTHING` makes the operation idempotent across retries and restarts.
    ///
    /// Broadcasting on the bus means SSE / memory indexer / any other
    /// subscriber observes the backfilled event the same way they'd observe a
    /// live emit — which is the whole point of routing through this entry
    /// point instead of a raw `sqlx::query("INSERT INTO events ...")` scattered
    /// across modules.
    ///
    /// Returns `Some(seq)` when this call inserted the row, `None` when
    /// `ON CONFLICT (id) DO NOTHING` skipped (the row was already present
    /// from a prior run). Callers that count inserts use the `Option`
    /// shape directly.
    ///
    /// `created` lets the caller stamp the historical wall-clock time of the
    /// original event so projections that group by `created` see a
    /// chronologically-accurate row; `None` uses `NOW()` (right for backfills
    /// derived from data that has no original timestamp, like the
    /// `image_description` field where the description was written in place
    /// onto the source `MessageReceived` payload).
    pub(crate) async fn replay_historical_event(
        &self,
        replay: HistoricalReplay<'_>,
    ) -> Result<Option<i64>, sqlx::Error> {
        let HistoricalReplay {
            event_id,
            aggregate,
            aggregate_id,
            event_type,
            payload,
            thread_id,
            created,
            broadcast,
        } = replay;
        let seq: Option<i64> = sqlx::query_scalar(
            r#"INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, thread_id, created)
               VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, NOW()))
               ON CONFLICT (id) DO NOTHING
               RETURNING sequence"#,
        )
        .bind(event_id)
        .bind(aggregate)
        .bind(aggregate_id)
        .bind(event_type)
        .bind(payload)
        .bind(thread_id)
        .bind(created)
        .fetch_optional(&self.pool)
        .await?;

        // Broadcast on the bus so subscribers observe the replay. Only emit
        // when the insert actually happened — re-runs (ON CONFLICT skip) must
        // not double-fire downstream consumers.
        //
        // **A system frame, even for a row that carries a `thread_id`.** That
        // is deliberate: a backfilled historical event is not a thing that just
        // happened on a thread, and broadcasting it as `BusEvent::Thread` would
        // put it through the trigger matcher, the thread projections and the
        // parent-callback fan-in as though it had. It does mean this is the one
        // place where the *matchable payload* differs between the live frame
        // and the row (`core::event_subscription`, ADR 0062): a `thread_id`
        // condition cannot match this broadcast, while the catch-up scan and
        // the arming lookback reading the row it just wrote will inject one.
        // The direction is the safe one, since such a wait resolves from the
        // scan a moment later rather than never, and the scan is what covers
        // this path anyway: a backfill runs at startup, where the wait cache is
        // still being rebuilt.
        if let (Some(seq_val), true) = (seq, broadcast) {
            let _ = self.event_tx.send(EmittedEvent {
                event_id,
                seq: Some(seq_val),
                created: Utc::now(),
                typed: BusEvent::System(SystemEvent::DomainEvent {
                    event_type: event_type.to_string(),
                    payload: payload.clone(),
                    depth: 0,
                    transient: false,
                    actor: None,
                }),
                aggregate: None,
                // A backfill is not a link in anyone's trigger chain, and the
                // variant above says the same thing. No trigger owns it either,
                // so it wakes every subscriber it matches.
                depth: 0,
                emitting_trigger_id: None,
            });
        }

        Ok(seq)
    }

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

    /// [`Self::emit_or_log`] for a caller that states which trigger's fire the
    /// event belongs to, rather than inheriting it. See
    /// [`Self::emit_as_trigger`].
    pub async fn emit_or_log_as_trigger(
        &self,
        event: BusEvent,
        ctx: &str,
        emitting_trigger_id: Option<String>,
    ) {
        if let Err(e) = self.emit_as_trigger(event, emitting_trigger_id).await {
            log!("[EventBus] {} emit failed: {}", ctx, e);
        }
    }

    /// Resolve the user actor from request headers + the `devices` table, hand
    /// it to `build`, wrap the result in `BusEvent::System`, and emit through
    /// `emit_or_log` — the canonical helper for mutating HTTP handlers that:
    ///
    /// - take `&HeaderMap` from axum,
    /// - have a `&PgPool` in scope,
    /// - need to stamp a same-workspace device/api actor (no body-supplied
    ///   device-id override, no cross-workspace `caller`), and
    /// - want fire-and-forget emit with a per-call-site log prefix.
    ///
    /// Equivalent to (and replaces) the hand-rolled five-line dance:
    ///
    /// ```ignore
    /// let actor = actor::user_actor_resolved(&headers, &pool, None).await;
    /// if let Err(e) = bus.emit(BusEvent::System(SystemEvent::X { /* fields */, actor })).await {
    ///     log!("[Module] Failed to emit X: {}", e);
    /// }
    /// ```
    ///
    /// `ctx` carries the per-module log prefix (e.g. `"[Apps] AppDeleted"`)
    /// so `[EventBus] [Apps] AppDeleted emit failed: …` remains greppable by
    /// module.
    ///
    /// For per-device handlers (body-supplied device id), cross-workspace
    /// callers, Thread events, or handlers that need the `EmitResult` back,
    /// call `user_actor_resolved` + `emit` / `emit_or_log` directly.
    pub async fn emit_user_system<F>(
        &self,
        headers: &axum::http::HeaderMap,
        pool: &sqlx::PgPool,
        ctx: &str,
        build: F,
    ) where
        F: FnOnce(Option<crate::engine::thread_events::MessageOrigin>) -> SystemEvent,
    {
        let actor = crate::api::actor::user_actor_resolved(headers, pool, None).await;
        self.emit_or_log(BusEvent::System(build(actor)), ctx).await;
    }

    /// Single entry point for all events.
    /// Persistence is determined by the event's `is_persisted()` method.
    pub async fn emit(
        &self,
        event: BusEvent,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
        // Read before the first `.await`, so the marker is this caller's fire
        // and not one a later hop happens to be running under.
        self.emit_as_trigger(event, current_trigger_id()).await
    }

    /// Emit on behalf of `emitting_trigger_id`'s fire, whatever task this runs
    /// on. [`Self::emit`] passes the ambient marker. Two callers state one
    /// instead: the *Thread Queue* and `emit_domain_event`.
    ///
    /// A queue entry's lifecycle frames go out on tasks the fire does not own.
    /// `ThreadQueued` precedes the fire, and `ThreadQueueCompleted` comes from
    /// the sibling task that joins it. Neither sees the ambient scope, so the
    /// queue states the owner it recorded on the entry.
    ///
    /// A domain event has the opposite problem. An HTTP emit runs on a request
    /// task with no ambient scope, and the fire that caused it may be another
    /// process. Its marker comes off the verified origin token (ADR 0137).
    ///
    /// `None` states the opposite just as firmly. A sub-thread the fire
    /// submitted is not the fire, and must not inherit its marker. Suppressing
    /// a wake the trigger asked for is the one direction that cannot recover.
    pub async fn emit_as_trigger(
        &self,
        event: BusEvent,
        emitting_trigger_id: Option<String>,
    ) -> Result<Option<EmitResult>, Box<dyn std::error::Error + Send + Sync>> {
        // Read before the first `.await`, so it is this caller's chain depth
        // and not something a later hop happens to be running under. Every
        // branch below stamps the same value onto its broadcast.
        let depth = emit_depth(&event);
        match &event {
            BusEvent::Thread {
                thread_id,
                event: te,
                meta,
            } => {
                if te.is_persisted() {
                    let event_id = meta.event_id.unwrap_or_else(Uuid::new_v4);
                    let mut tx = self.pool.begin().await?;

                    // === Phase: Validate ===
                    // Pre-INSERT checks that read the same tx snapshot the upcoming
                    // Persist will use. May short-circuit with Err before any events
                    // row is written, or with Ok(None) for the ChangeApplied
                    // single-fire guard (a recognized duplicate is dropped, not an
                    // error). See the EventBus struct doc-comment for the full phase
                    // contract.
                    //
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

                    // Single-fire guard for ChangeApplied: claim the change row
                    // (`FOR UPDATE`) and suppress the emit if a ChangeApplied was
                    // already recorded for this change_id. Every apply path funnels
                    // its ChangeApplied through this one chokepoint, so the guard
                    // makes the event idempotent across concurrent double-fires
                    // (the ~0.6s race), HTTP/Apply-All retries, conflict-recovery
                    // cleanup, and post-restart recovery re-emits (the 4-minute-gap
                    // case) — fixing two "Change applied" timeline entries for one
                    // change. The lock is held until this tx commits and Phase
                    // Project's `write_applied` flips the row to `applied`, so a
                    // racing emit blocks, then reads `applied` and is suppressed.
                    // Legitimate single emits (recovery no-ops, rows with no
                    // aggregate) run while the row is still pending or absent and
                    // therefore `Proceed`, emitting exactly once.
                    if let ThreadEvent::ChangeApplied { change_id, .. } = te {
                        use crate::core::changes_projection::{
                            ChangeApplyDedup, ChangesProjection,
                        };
                        if ChangesProjection::change_apply_dedup(&mut tx, change_id).await?
                            == ChangeApplyDedup::Suppress
                        {
                            tx.rollback().await?;
                            crate::log!(
                                "[EventBus] ChangeApplied suppressed for {} — already applied \
                                 (duplicate apply path)",
                                change_id
                            );
                            return Ok(None);
                        }
                    }

                    // === Phase: Persist ===
                    // INSERT into the events table. Assigns the bigserial sequence
                    // that drives ordering for every downstream consumer (SSE
                    // catch-up, history queries, projection backfills).
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
                    // === Phase: Project ===
                    // Update derived state (`thread_summaries` and friends) in the
                    // same tx, so the next Validate (on a concurrent emit) and the
                    // upcoming CaptureAggregate phase observe read-your-write.
                    // Cascades through parent rollup (`propagate_blocking_change`)
                    // internally; returns side-effect events to fire in PostCommit
                    // and the list of ancestor thread_ids whose
                    // `blocking_descendant_count` changed.
                    let (side_effects, affected_ancestors) = self
                        .update_thread_projection(&mut tx, *thread_id, te, meta)
                        .await?;
                    // === Phase: CaptureAggregate ===
                    // In-tx read-your-write fetches of the emitting thread's
                    // aggregate plus each affected ancestor's aggregate. Captured
                    // here (still inside the tx) so the snapshots cannot race
                    // with a concurrent emit committing between the projection
                    // update and a post-commit fetch — every subscriber sees an
                    // aggregate that matches exactly this event's post-state.
                    // A failed fetch logs and broadcasts without aggregate
                    // (frontend tolerates absence with a warning, but it
                    // indicates a backend bug).
                    let aggregate = match crate::core::store::fetch_thread_aggregate(
                        &mut *tx, *thread_id,
                    )
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
                    // Ancestor aggregates fetched inside the same tx so each
                    // rebroadcast sees the post-projection state (read-your-write)
                    // with the bumped `blocking_descendant_count`. Empty when the
                    // event didn't flip `is_blocking` (the common case) —
                    // propagation's early-exit on delta==0 keeps this off the
                    // hot path.
                    let mut ancestor_aggregates: Vec<crate::core::store::ThreadAggregate> =
                        Vec::with_capacity(affected_ancestors.len());
                    for ancestor_id in &affected_ancestors {
                        match crate::core::store::fetch_thread_aggregate(&mut *tx, *ancestor_id)
                            .await
                        {
                            Ok(Some(agg)) => ancestor_aggregates.push(agg),
                            Ok(None) => {}
                            Err(e) => {
                                crate::log!(
                                    "[EventBus] Failed to fetch ancestor ThreadAggregate for {}: {}",
                                    ancestor_id,
                                    e
                                );
                            }
                        }
                    }

                    // === Phase: PostCommit ===
                    // Commit the tx, then fan out to external consumers: broadcast
                    // the EmittedEvent (with its CaptureAggregate snapshot) to all
                    // `bus.subscribe()` consumers, rebroadcast each affected
                    // ancestor's aggregate, wake the parent agent session via the
                    // `parent_callback_tx` mpsc, and replay any side-effect events
                    // from Project via recursive `emit()` calls (each gets its own
                    // tx). Anything after `tx.commit()` runs at-most-once on a
                    // panic — the events row is durable but downstream fan-out
                    // (broadcast, mpsc, side-effects) is fire-and-forget.
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
                        depth,
                        emitting_trigger_id,
                    });
                    // Rebroadcast each affected ancestor's aggregate so the
                    // frontend's `meta.blockingDescendantCount` stays live —
                    // without this, the cascading-archive button-hide
                    // wouldn't update in real time. We piggyback on the
                    // already-emitted ChildrenCountChanged transient: same
                    // shape, same channel, just carrying a different aggregate
                    // snapshot. Active/total counts are read off the same row
                    // we just fetched, so consumers that overlay on `thread.meta`
                    // see a self-consistent update.
                    for ancestor_agg in ancestor_aggregates {
                        let ancestor_id = match Uuid::parse_str(&ancestor_agg.thread_id) {
                            Ok(id) => id,
                            Err(e) => {
                                crate::log!(
                                    "[EventBus] Unparseable ancestor thread_id {:?}: {} — skipping rebroadcast",
                                    ancestor_agg.thread_id,
                                    e
                                );
                                continue;
                            }
                        };
                        let active = ancestor_agg.active_children_count;
                        let total = ancestor_agg.total_children_count;
                        self.send_children_count_event(
                            ancestor_id,
                            active,
                            total,
                            Some(ancestor_agg),
                        );
                    }
                    // Run after broadcast so a panic here can't skip SSE delivery
                    self.notify_parent_if_child(notify_thread_id, event_id, &notify_event)
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
                        depth,
                        emitting_trigger_id,
                    });
                    Ok(None)
                }
            }
            BusEvent::System(se) => {
                if se.is_persisted() {
                    let event_id = Uuid::new_v4();
                    let stored_event_type = se.stored_event_type();
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
                        depth,
                        emitting_trigger_id,
                    });
                    Ok(Some(EmitResult { event_id, seq }))
                } else {
                    // Transient system events still drive projections (e.g.
                    // device_presence). The events table is intentionally
                    // skipped — these are high-churn and not interesting to
                    // replay. Skip the SSE broadcast when the projection
                    // reports no real change (e.g. DeviceVisible heartbeats
                    // every 30s from the frontend).
                    let should_broadcast = self.update_transient_system_projection(se).await?;
                    if should_broadcast {
                        let _ = self.event_tx.send(EmittedEvent {
                            event_id: Uuid::new_v4(),
                            seq: None,
                            created: Utc::now(),
                            typed: event,
                            aggregate: None,
                            depth,
                            emitting_trigger_id,
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
            SystemEvent::DeviceVisible { device_id } => {
                crate::core::DevicePresenceStore::record_visible(&self.pool, device_id).await
            }
            SystemEvent::DeviceHidden { device_id } => {
                crate::core::DevicePresenceStore::record_hidden(&self.pool, device_id).await
            }
            _ => Ok(true),
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
#[path = "../event_bus_tests/mod.rs"]
mod tests;

#[path = "../event_bus_projection.rs"]
mod projection;
