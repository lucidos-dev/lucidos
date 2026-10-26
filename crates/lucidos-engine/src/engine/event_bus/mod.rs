//! EventBus — single emission point for all domain events.
//!
//! Producers call typed methods (`emit_thread`, `emit_notification`, etc.).
//! The bus drives an in-tx pipeline that persists the event, updates projections,
//! and captures a post-projection aggregate snapshot — then commits and broadcasts
//! the snapshot to consumers. Consumers (SSE, memory indexer, etc.) subscribe
//! independently and observe events only after `tx.commit()`.
//!
//! See the [`EventBus`] struct doc-comment for the full two-contract surface:
//! the six-phase in-emit pipeline (Serialize → Validate → Persist → Project →
//! CaptureAggregate → PostCommit) and the post-commit subscriber contract.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use crate::engine::thread_events::{CancelCause, EventMeta, ThreadEvent};
use crate::engine::thread_lifecycle::{
    self, ArchiveState, ChangeStateKind, ThreadStatus, ThreadType,
};
use crate::scheduler::user_tasks::{current_event_trigger_depth, current_trigger_id};

/// CC session-end status — always `'idle'`. A proposed change is a review
/// artifact, not a parked loop, so it does not block the parent's "is my
/// child still working?" rollup. Pending review surfaces via the `proposed`
/// change state (and `is_blocking` clause 3) instead.
pub(super) const STATUS_FROM_PROPOSED_CHANGE: &str = ThreadStatus::Idle.sql_literal();

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
pub(super) const PRESERVED_STATUS_VERDICTS: &[ThreadStatus] =
    &[ThreadStatus::Failed, ThreadStatus::Paused];

/// [`PRESERVED_STATUS_VERDICTS`] as a SQL `IN (...)` body. Built once: the
/// projection interpolates it on every status write.
pub(super) static PRESERVED_STATUS_VERDICTS_SQL: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        PRESERVED_STATUS_VERDICTS
            .iter()
            .map(|v| v.sql_literal())
            .collect::<Vec<_>>()
            .join(",")
    });

/// Wrap a `status` target so a verdict already on the row survives.
///
/// A verdict (see [`PRESERVED_STATUS_VERDICTS`]) is a statement about how the
/// turn ended: it failed, or an engine restart interrupted it. Only a real start
/// event (`MessageReceived`, `CodingAgentUserMessageSent`, `PromptInjected`,
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
/// Not expressible in the coarse `StatusRule` model that
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

/// The events that end a thread's turn or start one. The newest of them says
/// whether the thread is between turns (`MessageReceived` through
/// `ContinuationRequested` start one, the first four end one).
const TURN_BOUNDARY_EVENT_TYPES: &[&str] = &[
    "ResponseGenerated",
    "ResponseFailed",
    "ResponseCanceled",
    "ResponseAborted",
    "MessageReceived",
    "CodingAgentUserMessageSent",
    "PromptInjected",
    "CodingAgentPromptSent",
    "ContinuationRequested",
];

/// SQL boolean: the newest turn boundary of the thread named by `thread_id_text`
/// (a SQL expression of type text) is a redirect cancel. The engine interrupted
/// that turn to carry the work on, and the redirected turn has not started yet.
/// A start event after the cancel is the redirected turn, so the answer turns to
/// FALSE. A thread with no boundary at all is FALSE too.
///
/// One definition for the fan-in (`turn_ended_as_redirect`) and the thread
/// status projection, which keeps a redirected thread `running` between turns.
/// The redirect causes come from [`CancelCause::is_redirect`].
pub(super) fn turn_ended_as_redirect_sql(thread_id_text: &str) -> String {
    let boundaries = quoted_list(TURN_BOUNDARY_EVENT_TYPES.iter().copied());
    let redirects = quoted_list(
        CancelCause::ALL
            .into_iter()
            .filter(|cause| cause.is_redirect())
            .map(CancelCause::as_str),
    );
    format!(
        "COALESCE((SELECT e.event_type = 'ResponseCanceled' \
                          AND e.payload->>'cause' IN ({redirects}) \
                   FROM events e \
                   WHERE e.aggregate_id = {thread_id_text} \
                     AND e.event_type IN ({boundaries}) \
                   ORDER BY e.sequence DESC LIMIT 1), FALSE)"
    )
}

fn quoted_list<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items
        .map(|item| format!("'{item}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Wrap a `status` target so a thread between a redirect's cancel and its
/// resumed turn stays as it is. See [`turn_ended_as_redirect_sql`]. The
/// closing-out events of the interrupted run (`CodingAgentIdled`,
/// `SessionEnded`) use it, so the status does not flicker to `idle` in the gap.
pub(super) fn keeping_status_across_redirect(target: &str) -> String {
    format!(
        "CASE WHEN {redirected} THEN status ELSE {target} END",
        redirected = turn_ended_as_redirect_sql("thread_summaries.thread_id::text"),
    )
}

/// SET fragment for a fact about the branch: the SQL boolean `has_work` says
/// whether it differs from main. A pending change keeps the change state
/// `proposed`, and work that stays `unproposed` keeps its reason (ADR 0400).
/// Postgres evaluates every SET against the old row, so both arms read the
/// state from before this write.
pub(crate) fn branch_work_sql(has_work: &str) -> String {
    let proposed = ChangeStateKind::Proposed.as_str();
    let unproposed = ChangeStateKind::Unproposed.as_str();
    let none = ChangeStateKind::None.as_str();
    format!(
        "coding_agent_change_state = CASE WHEN coding_agent_change_state = '{proposed}' \
            THEN '{proposed}' WHEN {has_work} THEN '{unproposed}' ELSE '{none}' END, \
         coding_agent_unproposed_reason = CASE \
            WHEN coding_agent_change_state = '{unproposed}' AND {has_work} \
            THEN coding_agent_unproposed_reason END"
    )
}

/// SET fragment that clears the CC-state columns on `thread_summaries` that
/// events own, used identically by `ChangeApplied`, `ChangeDiscarded`, and
/// `ThreadArchived`. A pending change stays `proposed` here: it follows the
/// `changes` table (`ChangesProjection::sync_thread_proposal`).
pub(super) static CLEAR_CODING_AGENT_FLAGS: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        format!(
            "coding_agent_is_external_repo = FALSE, {}",
            branch_work_sql("FALSE")
        )
    });

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
    /// The scheduler dispatches a thread event at this depth. That is what
    /// makes the depth cap engage for a chain across triggers, where A's fire
    /// wakes B and B's fire wakes A. It reaches a chain routed through spawned
    /// work too, unlike `emitting_trigger_id` below. A domain event carries its
    /// own depth on the variant instead: see [`Self::trigger_chain_depth`].
    ///
    /// The stored row records the dispatch depth in `trigger_chain_depth`, so a
    /// lagged scheduler's replay dispatches at the depth the live frame had.
    pub depth: u32,
    /// Which trigger's fire emitted this, if any. Read from
    /// `scheduler::user_tasks::current_trigger_id`, or stated outright by a
    /// caller of [`EventBus::emit_as_trigger`].
    ///
    /// The scheduler drops this trigger from the matches, so a trigger is never
    /// woken by an event its own fire emitted. The stored row records it in
    /// `emitting_trigger_id`, so a replay suppresses the same wake. Absent means
    /// no suppression, which is the direction to fail in: an extra wake, never a
    /// missing one. See `docs/adr/0137-a-trigger-never-wakes-itself.md`.
    pub emitting_trigger_id: Option<String>,
}

/// The depth the trigger matcher dispatches `event` at, given the depth of the
/// frame that carries it.
///
/// A domain event reads its own variant's depth, set when it was built. Every
/// other event reads the frame's. `persist` stores the same answer, so a replay
/// dispatches at the live depth.
pub(crate) fn trigger_chain_depth(event: &BusEvent, frame_depth: u32) -> u32 {
    match event {
        BusEvent::System(se) => system_trigger_chain_depth(se, frame_depth),
        BusEvent::Thread { .. } => thread_trigger_chain_depth(frame_depth),
    }
}

/// [`trigger_chain_depth`] for a thread event.
fn thread_trigger_chain_depth(frame_depth: u32) -> u32 {
    frame_depth
}

/// [`trigger_chain_depth`] for a system event.
fn system_trigger_chain_depth(event: &SystemEvent, frame_depth: u32) -> u32 {
    match event {
        SystemEvent::DomainEvent { depth, .. } => *depth,
        _ => frame_depth,
    }
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

/// The events row one append writes.
struct NewEventRow<'a> {
    id: Uuid,
    aggregate: &'a str,
    aggregate_id: &'a str,
    event_type: &'a str,
    payload: &'a serde_json::Value,
    /// What the broadcast carries to the trigger matcher, so a replay of the
    /// row dispatches the same way: the depth and the emitting trigger.
    trigger_chain_depth: u32,
    emitting_trigger_id: Option<&'a str>,
}

/// A system event whose row has committed and whose broadcast has not gone out.
struct CommittedSystemEvent {
    event_id: Uuid,
    seq: i64,
    event: SystemEvent,
    depth: u32,
    emitting_trigger_id: Option<String>,
    ticket: AppendTicket,
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

mod auto_resume_hold;
pub(crate) use auto_resume_hold::AutoResumeHolds;

mod parent_wake_hold;
use parent_wake_hold::ParentWakeHold;

mod parent_callback;
pub(crate) use parent_callback::{batch_still_working_sql, ChildSettle, Delivery};

mod append_horizon;
pub(crate) use append_horizon::committed_event_horizon;
use append_horizon::Append;

mod append_ticket;

#[cfg(test)]
mod test_rows;
use append_ticket::{AppendTicket, AppendTickets};

mod lag_replay;
use lag_replay::UnbroadcastRows;
pub(crate) use lag_replay::{LagReplay, ReplayScope, ReplayWindow};

impl EmittedEvent {
    /// The depth the trigger matcher dispatches this frame at. See
    /// [`trigger_chain_depth`].
    pub(crate) fn trigger_chain_depth(&self) -> u32 {
        trigger_chain_depth(&self.typed, self.depth)
    }

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
                    // Goes to every client for every thread, and only the step
                    // modal reads it, which fetches it by event id.
                    if matches!(event, ThreadEvent::CodingAgentToolResult { .. }) {
                        ThreadEvent::strip_result_text(obj);
                    }
                    ThreadEvent::strip_tool_call_args(event.event_type(), obj);
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
    /// The thread's `summary_version` right after this event's projection,
    /// read from the aggregate this emit broadcast. `None` for a system event,
    /// which has no thread row. A write that answers with it lets the client
    /// hold its optimistic state until that version arrives (ADR 0437).
    pub summary_version: Option<i64>,
}

/// Whether a failed [`EventBus::emit`] lost at the unique index `index`.
///
/// An index such as `events_owner_approval_spent_unique` is a claim: the emit
/// that loses it is a refused caller, not a broken store.
pub(crate) fn violates_unique_index(
    error: &(dyn std::error::Error + Send + Sync + 'static),
    index: &str,
) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|db| db.constraint())
        == Some(index)
}

/// A thread event that [`EventBus::append_thread_events`] has persisted and
/// projected inside its transaction, waiting for the commit to publish it.
struct StagedThreadEvent {
    event_id: Uuid,
    seq: i64,
    event: ThreadEvent,
    meta: EventMeta,
    aggregate: Option<crate::core::store::ThreadAggregate>,
    ancestor_aggregates: Vec<crate::core::store::ThreadAggregate>,
    side_effects: Vec<BusEvent>,
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

/// A spawn command the bus sends after commit, on a directed channel the spawn
/// dispatcher drains. A broadcast lag cannot drop it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnRequest {
    /// Re-enter an interrupted coding-agent session with `--resume`, sending
    /// the continuation text in place of a new user message.
    Continue {
        thread_id: Uuid,
        /// The `ContinuationRequested` event id: the idempotency key.
        event_id: Uuid,
    },
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
pub(crate) const BUS_CAPACITY: usize = 4096;

/// Advisory-lock class for [`EventBus::lock_thread_append`]. The two-key form
/// keeps it apart from the one-key locks elsewhere in the engine.
pub const THREAD_APPEND_LOCK_CLASS: i32 = 0x6170_6e64;

/// Single entry point for every domain event. Carries two distinct contracts:
///
/// # In-emit pipeline (6 phases, in this order)
///
/// Every persisted-thread `emit()` runs the same 6-phase pipeline. Each phase
/// is marked in source with a `// === Phase: <Name> ===` banner; the
/// `emit_pipeline_has_six_named_phases_in_order` test pins the ordering.
///
/// The transaction opens with [`EventBus::begin_append`], before any phase. Its
/// in-flight append lock is what makes a committed horizon readable (ADR 0364).
///
/// An *atomic emit* ([`EventBus::emit_atomically`]) runs phases 1 to 4 once per
/// event inside one transaction, then PostCommit for all of them.
///
/// 0. **Serialize** — take the thread append lock, the transaction's second
///    lock after the in-flight append lock. A thread's events therefore commit
///    in sequence order, which every `sequence > after` catch-up relies on
///    (ADR 0360).
/// 1. **Validate** — pre-INSERT checks (e.g. `request_event_id` existence) that
///    read the same tx snapshot the upcoming Persist will use. May short-circuit
///    with `Err` before any events row is written, or with `Ok(None)` for an
///    event a guard drops (a stale reply sighting, a moved child's card).
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
///    `subscribe()` consumers, send a `ContinuationRequested` on
///    `spawn_request_tx`, rebroadcast each affected ancestor's aggregate,
///    wake the parent agent session via `parent_callback_tx`, and replay
///    Project's side-effects via recursive `emit()` calls.
///
/// The banners live in `append_thread_events`. New code there declares its
/// phase by sitting under the matching banner. Phases 0 to 4 share one
/// transaction, which validate-reads-projection and the CaptureAggregate
/// read-your-write guarantee both need. `append_system_event` and the
/// transient arms run a subset: no Serialize or Validate for system events,
/// and no CaptureAggregate for transient events.
///
/// # Post-commit subscriber contract
///
/// Two consumer surfaces, each with a different shape:
///
/// - **Broadcast subscribers** — call [`subscribe()`](EventBus::subscribe) to
///   get a [`broadcast::Receiver<EmittedEvent>`]. Every subscriber observes
///   every event after `tx.commit()`. No ordering between subscribers is
///   guaranteed. Channel capacity is [`BUS_CAPACITY`]; slow subscribers receive
///   `RecvError::Lagged`, and a lag must never drop work. A subscriber that
///   mirrors state re-reads it. One that acts per event subscribes through
///   [`LagReplay::subscribe`] and replays the rows it missed, as the scheduler
///   does for trigger fires.
/// - **Parent callbacks** — directed (not broadcast). The child→parent fan-in
///   sent on `parent_callback_tx` wakes the parent agent session's runtime
///   when a child thread reaches a terminal event. The lone consumer is the
///   `ParentCallback` receiver returned from [`EventBus::new`]; the channel is
///   unbounded.
/// - **Spawn requests** — directed, unbounded. Every committed
///   `ContinuationRequested` sends a [`SpawnRequest`] to the one consumer that
///   took [`EventBus::take_spawn_requests`]. The channel exists from `new`, so
///   a request emitted before the consumer starts waits for it.
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
    spawn_request_tx: mpsc::UnboundedSender<SpawnRequest>,
    /// The receiving end of `spawn_request_tx`, until the spawn dispatcher
    /// takes it. Shared so every clone of the bus hands out the same one.
    spawn_request_rx:
        std::sync::Arc<std::sync::Mutex<Option<mpsc::UnboundedReceiver<SpawnRequest>>>>,
    changes_projection: crate::core::changes_projection::ChangesProjection,
    /// Children the engine is about to auto-resume, so `notify_parent_if_child`
    /// withholds their completion card. See [`AutoResumeHolds`].
    auto_resume_holds: AutoResumeHolds,
    /// Parent wakes queued during boot recovery and teardown. See
    /// [`ParentWakeHold`].
    parent_wake_hold: ParentWakeHold,
    /// Rows written without a broadcast. A [`LagReplay`] skips them, because
    /// no live subscriber saw them and none was meant to.
    unbroadcast: UnbroadcastRows,
    /// Every persisted append in flight. A [`LagReplay`] opens from them.
    append_tickets: AppendTickets,
    /// Threads running a *cache keep-alive* turn, whose output the projection
    /// skips. See [`projection::keep_alive`].
    keep_alive_turns: projection::keep_alive::KeepAliveTurns,
}

impl EventBus {
    pub fn new(pool: PgPool) -> (Self, mpsc::UnboundedReceiver<ParentCallback>) {
        let (event_tx, _) = broadcast::channel(BUS_CAPACITY);
        let (parent_callback_tx, parent_callback_rx) = mpsc::unbounded_channel();
        let (spawn_request_tx, spawn_request_rx) = mpsc::unbounded_channel();
        let changes_projection =
            crate::core::changes_projection::ChangesProjection::new(pool.clone());
        (
            Self {
                pool,
                event_tx,
                parent_callback_tx,
                spawn_request_tx,
                spawn_request_rx: std::sync::Arc::new(std::sync::Mutex::new(Some(
                    spawn_request_rx,
                ))),
                changes_projection,
                auto_resume_holds: AutoResumeHolds::default(),
                parent_wake_hold: ParentWakeHold::default(),
                unbroadcast: UnbroadcastRows::default(),
                append_tickets: AppendTickets::default(),
                keep_alive_turns: Default::default(),
            },
            parent_callback_rx,
        )
    }

    /// The database this bus persists to.
    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The register the engine takes a hold in before it emits a terminal it
    /// intends to auto-resume past.
    pub(crate) fn auto_resume_holds(&self) -> &AutoResumeHolds {
        &self.auto_resume_holds
    }

    /// Queue every parent wake until [`Self::release_held_parent_wakes`]. Boot
    /// calls this before any recovery sweep emits, and teardown before its
    /// aborts.
    pub fn hold_parent_wakes(&self) {
        self.parent_wake_hold.engage();
    }

    /// Send every parent wake queued since [`Self::hold_parent_wakes`],
    /// and send later ones straight away. Returns how many went out.
    pub fn release_held_parent_wakes(&self) -> usize {
        let held = self.parent_wake_hold.release();
        let count = held.len();
        for callback in held {
            let child_thread_id = callback.child_thread_id;
            if let Err(e) = self.parent_callback_tx.send(callback) {
                crate::log!(
                    "[FanOut] Failed to release the boot-held parent wake for child {}: {}",
                    child_thread_id,
                    e
                );
            }
        }
        count
    }

    /// The receiver every committed `ContinuationRequested` is sent to. `None`
    /// once taken: the spawn dispatcher is its one consumer.
    pub(crate) fn take_spawn_requests(&self) -> Option<mpsc::UnboundedReceiver<SpawnRequest>> {
        self.spawn_request_rx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn send_spawn_request(&self, request: SpawnRequest) {
        if let Err(e) = self.spawn_request_tx.send(request) {
            crate::log!("[EventBus] Spawn request has no consumer: {:?}", e.0);
        }
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

    /// Reconcile the change state with the branch's live diff, and broadcast
    /// the updated aggregate if the state changed.
    ///
    /// Used by the coding-agent worktree's post-commit hook. This moves only
    /// between `none` and `unproposed`: a proposal (the `changes` row, the
    /// Apply button) stays owned by `ChangeProposed` at coding-agent idle.
    pub async fn refresh_branch_work_and_broadcast(
        &self,
        thread_id: Uuid,
        has_diff: bool,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(&format!(
            "UPDATE thread_summaries SET {} \
             WHERE thread_id = $1 \
               AND coding_agent_change_state <> '{}' \
               AND (coding_agent_change_state = '{}') IS DISTINCT FROM $2",
            branch_work_sql("$2"),
            ChangeStateKind::Proposed.as_str(),
            ChangeStateKind::Unproposed.as_str(),
        ))
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

    /// Take `thread_id`'s *thread append lock*, held until the transaction
    /// ends. Every append of a row carrying a `thread_id` takes it, so a
    /// thread's events commit in sequence order (ADR 0360). *Admission* takes
    /// it too, so an input's place and the thread's appends share one order
    /// (ADR 0449).
    ///
    /// **Only the in-flight append lock may come before it.** A waiter then
    /// holds just its own in-flight key, which only a horizon read waits on, so
    /// it cannot close a deadlock cycle (ADR 0364).
    pub(crate) async fn lock_thread_append(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2::text))")
            .bind(THREAD_APPEND_LOCK_CLASS)
            .bind(thread_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    /// Persist an event to the events table. Returns (event_id, sequence).
    ///
    /// `created` is `clock_timestamp()`, not the transaction start: a thread
    /// event's transaction may have waited for its append lock, and its stamp
    /// must not precede the event it queued behind.
    ///
    /// Depth 0 and no emitting trigger are stored as `NULL`, which is how
    /// every older row reads.
    async fn persist(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        ticket: &AppendTicket,
        row: NewEventRow<'_>,
    ) -> Result<i64, sqlx::Error> {
        let seq: i64 = sqlx::query_scalar(
            r#"INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id,
                                   trigger_chain_depth, emitting_trigger_id)
               VALUES ($1, $2, $3, $4, $5, clock_timestamp(),
                       CASE WHEN $2 = 'thread' THEN $3::uuid ELSE NULL END,
                       NULLIF($6, 0), $7)
               RETURNING sequence"#,
        )
        .bind(row.id)
        .bind(row.aggregate)
        .bind(row.aggregate_id)
        .bind(row.event_type)
        .bind(row.payload)
        .bind(i32::try_from(row.trigger_chain_depth).unwrap_or(i32::MAX))
        .bind(row.emitting_trigger_id)
        .fetch_one(&mut **tx)
        .await?;
        ticket.learned(seq);

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
    /// chronologically-accurate row; `None` stamps the insert time (right for
    /// backfills derived from data that has no original timestamp, like the
    /// `image_description` field where the description was written in place
    /// onto the source `MessageReceived` payload).
    ///
    /// It draws a sequence like any append, so it opens with
    /// [`Self::begin_append`] too (ADR 0364). A row with a `thread_id` then
    /// takes that thread's append lock, like every live append (ADR 0360).
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
        let Append { mut tx, ticket } = self.begin_append().await?;
        if let Some(thread_id) = thread_id {
            Self::lock_thread_append(&mut tx, thread_id).await?;
        }
        let seq: Option<i64> = sqlx::query_scalar(
            r#"INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, thread_id, created)
               VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, clock_timestamp()))
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
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(seq) = seq {
            ticket.learned(seq);
        }
        tx.commit().await?;

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
        if let (Some(seq_val), false) = (seq, broadcast) {
            self.unbroadcast.record(seq_val);
        }
        if let (Some(seq_val), true) = (seq, broadcast) {
            ticket.send(
                &self.event_tx,
                EmittedEvent {
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
                },
            );
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
        let event = match event {
            BusEvent::Thread {
                thread_id,
                event: te,
                meta,
            } if te.is_persisted() => {
                let appended = self
                    .append_thread_events(thread_id, vec![(te, meta)], depth, emitting_trigger_id)
                    .await?;
                return Ok(appended.and_then(|results| results.into_iter().next()));
            }
            BusEvent::System(se) if se.is_persisted() => {
                let append = self.begin_append().await?;
                let appended = self
                    .append_system_event(append, Uuid::new_v4(), se, depth, emitting_trigger_id)
                    .await?;
                return Ok(Some(appended));
            }
            event => event,
        };
        match &event {
            BusEvent::Thread { .. } => {
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
            BusEvent::System(se) => {
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

    /// Append a webhook delivery's domain event and record the delivery in the
    /// same transaction (ADR 0444). A resend inside its window rolls back and
    /// reports the event the first delivery emitted. No trigger emitted it: a
    /// delivery arrives from outside, so it wakes every subscriber.
    pub(crate) async fn append_webhook_delivery(
        &self,
        event: SystemEvent,
        delivery: &crate::core::webhook_deliveries::DeliveryKey,
    ) -> Result<
        crate::core::webhook_deliveries::DeliveryOutcome,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        use crate::core::webhook_deliveries::{DeliveryLedger, DeliveryOutcome};
        // Read before the first `.await`, as `emit` does.
        let depth = current_event_trigger_depth();
        let event_id = Uuid::new_v4();
        let mut append = self.begin_append().await?;
        if let Some(first) = DeliveryLedger::record(&mut append.tx, delivery, event_id).await? {
            append.tx.rollback().await?;
            return Ok(DeliveryOutcome::Duplicate(first));
        }
        let emitted = self
            .append_system_event(append, event_id, event, depth, None)
            .await?;
        Ok(DeliveryOutcome::Emitted(emitted.event_id))
    }

    /// Persist and project a system event on `tx`, commit, then broadcast it.
    async fn append_system_event(
        &self,
        append: Append,
        event_id: Uuid,
        event: SystemEvent,
        depth: u32,
        emitting_trigger_id: Option<String>,
    ) -> Result<EmitResult, Box<dyn std::error::Error + Send + Sync>> {
        let committed = self
            .commit_system_event(append, event_id, event, depth, emitting_trigger_id)
            .await?;
        Ok(self.publish_system_event(committed))
    }

    /// The first half of [`Self::append_system_event`]: persist, project and
    /// commit. A test parks between the halves to open a subscription there.
    async fn commit_system_event(
        &self,
        append: Append,
        event_id: Uuid,
        event: SystemEvent,
        depth: u32,
        emitting_trigger_id: Option<String>,
    ) -> Result<CommittedSystemEvent, Box<dyn std::error::Error + Send + Sync>> {
        let Append { mut tx, ticket } = append;
        let seq = self
            .persist(
                &mut tx,
                &ticket,
                NewEventRow {
                    id: event_id,
                    aggregate: event.aggregate(),
                    aggregate_id: &event.aggregate_id(),
                    event_type: event.stored_event_type(),
                    payload: &event.to_payload(),
                    trigger_chain_depth: system_trigger_chain_depth(&event, depth),
                    emitting_trigger_id: emitting_trigger_id.as_deref(),
                },
            )
            .await?;
        self.update_system_projection(&mut tx, event_id, &event)
            .await?;
        tx.commit().await?;
        Ok(CommittedSystemEvent {
            event_id,
            seq,
            event,
            depth,
            emitting_trigger_id,
            ticket,
        })
    }

    /// The second half of [`Self::append_system_event`]: broadcast the row.
    fn publish_system_event(&self, committed: CommittedSystemEvent) -> EmitResult {
        let CommittedSystemEvent {
            event_id,
            seq,
            event,
            depth,
            emitting_trigger_id,
            ticket,
        } = committed;
        ticket.send(
            &self.event_tx,
            EmittedEvent {
                event_id,
                seq: Some(seq),
                created: Utc::now(),
                typed: BusEvent::System(event),
                aggregate: None,
                depth,
                emitting_trigger_id,
            },
        );
        EmitResult {
            event_id,
            seq,
            summary_version: None,
        }
    }

    /// Emit `events` on one thread as one *atomic emit*: one transaction, all
    /// or none, broadcast in order after the commit.
    ///
    /// Use it when one event records that another happened. As two emits, a
    /// failure between them leaves the first standing alone. `Ok(None)` means
    /// a Validate guard suppressed one event, which drops them all.
    pub async fn emit_atomically(
        &self,
        thread_id: Uuid,
        events: Vec<(ThreadEvent, EventMeta)>,
    ) -> Result<Option<Vec<EmitResult>>, Box<dyn std::error::Error + Send + Sync>> {
        // Read before the first `.await`, as `emit` does.
        let emitting_trigger_id = current_trigger_id();
        if let Some((transient, _)) = events.iter().find(|(te, _)| !te.is_persisted()) {
            return Err(format!(
                "{} is transient, so it cannot join an atomic emit",
                transient.event_type()
            )
            .into());
        }
        self.append_thread_events(
            thread_id,
            events,
            thread_emit_depth(thread_id),
            emitting_trigger_id,
        )
        .await
    }

    /// Append persisted `events` to `thread_id` in one transaction, then
    /// publish each. Every persisted thread event runs this pipeline. The
    /// [`EventBus`] doc holds the phase contract.
    async fn append_thread_events(
        &self,
        thread_id: Uuid,
        events: Vec<(ThreadEvent, EventMeta)>,
        depth: u32,
        emitting_trigger_id: Option<String>,
    ) -> Result<Option<Vec<EmitResult>>, Box<dyn std::error::Error + Send + Sync>> {
        let Append { mut tx, ticket } = self.begin_append().await?;

        // === Phase: Serialize ===
        // Right after the in-flight lock and before any other statement:
        // Validate's row locks must never be held by a transaction waiting here.
        Self::lock_thread_append(&mut tx, thread_id).await?;

        let mut staged = Vec::with_capacity(events.len());
        for (mut te, meta) in events {
            let event_id = meta.event_id.unwrap_or_else(Uuid::new_v4);

            // === Phase: Validate ===
            // Pre-INSERT checks on the snapshot Persist will use. `Err` refuses
            // the event, and `Ok(None)` drops a recognised duplicate. Either
            // one drops the whole atomic emit.
            //
            // A `request_event_id` must exist. The frontend cannot group an
            // orphaned reference into an exchange, so log it loudly.
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
                        req_id,
                        te.event_type(),
                        thread_id
                    );
                }
            }

            // A sighting counts only for the request the client saw
            // (ADR 0409). The thread append lock above orders it against
            // a new `ThreadReadRequested`. So this read drops a duplicate,
            // and a sighting older than a newer request.
            if let ThreadEvent::ThreadReplySeen { seen_version } = &te {
                let row: Option<(bool, i64)> = sqlx::query_as(
                    "SELECT read_requested, summary_version FROM thread_summaries \
                     WHERE thread_id = $1",
                )
                .bind(thread_id)
                .fetch_optional(&mut *tx)
                .await?;
                if row != Some((true, *seen_version)) {
                    tx.rollback().await?;
                    crate::log!(
                        "[EventBus] ThreadReplySeen dropped for {}: the request it \
                         saw is gone or newer",
                        thread_id
                    );
                    return Ok(None);
                }
            }

            // A thread waiting on the user is never archived, and the
            // home thread is never archived, whoever emits the archive
            // (ADR 0259, ADR 0362).
            if matches!(te, ThreadEvent::ThreadArchived) {
                Self::check_archive_allowed(&mut tx, &thread_id).await?;
            }

            // Nothing titles the home thread automatically, whoever
            // emits the title. A rename by hand is `ThreadTitleRenamed`
            // and lands (ADR 0362).
            if matches!(te, ThreadEvent::ThreadTitleGenerated { .. }) {
                Self::check_title_generated_allowed(&mut tx, &thread_id).await?;
            }

            // A card for a child moved to top level is dropped, not
            // delivered (ADR 0278). The fan-in read the edge before this
            // transaction began, so a detach can land in between. A
            // move whose edge is already gone is dropped the same way,
            // so the former parent records each move once.
            let dropped_child = match &te {
                ThreadEvent::ChildThreadCompleted {
                    child_thread_id, ..
                }
                | ThreadEvent::ChildReportFiled {
                    child_thread_id, ..
                }
                | ThreadEvent::ChildThreadStopped {
                    child_thread_id, ..
                } => Self::child_was_moved_out(&mut tx, thread_id, *child_thread_id)
                    .await?
                    .then_some(*child_thread_id),
                ThreadEvent::ChildThreadDetached {
                    child_thread_id, ..
                } => (!Self::lock_edge_for_detach(&mut tx, thread_id, *child_thread_id).await?)
                    .then_some(*child_thread_id),
                _ => None,
            };
            if let Some(child_thread_id) = dropped_child {
                tx.rollback().await?;
                crate::log!(
                    "[EventBus] {} suppressed on {}: child {} is not its child",
                    te.event_type(),
                    thread_id,
                    child_thread_id
                );
                return Ok(None);
            }

            // A capture names its *agent session*. The append lock keeps a
            // session from opening between this read and the insert.
            if let ThreadEvent::ContextCaptured { session, .. } = &mut te {
                *session =
                    Some(super::session_scope::current_session_ordinal(&mut tx, thread_id).await?);
            }

            // === Phase: Persist ===
            // INSERT into the events table. Assigns the bigserial sequence
            // that drives ordering for every downstream consumer (SSE
            // catch-up, history queries, projection backfills).
            let seq = self
                .persist(
                    &mut tx,
                    &ticket,
                    NewEventRow {
                        id: event_id,
                        aggregate: "thread",
                        aggregate_id: &thread_id.to_string(),
                        event_type: te.event_type(),
                        payload: &te.to_payload(&meta),
                        trigger_chain_depth: thread_trigger_chain_depth(depth),
                        emitting_trigger_id: emitting_trigger_id.as_deref(),
                    },
                )
                .await?;

            // === Phase: Project ===
            // Update derived state in the same tx, so a later Validate and
            // CaptureAggregate read their own write. Returns the side-effect
            // events for PostCommit, and the ancestors whose
            // `blocking_descendant_count` changed.
            let (side_effects, affected_ancestors) = self
                .update_thread_projection(&mut tx, thread_id, &te, &meta)
                .await?;

            // === Phase: CaptureAggregate ===
            // Snapshot this thread and each affected ancestor inside the tx, so
            // a subscriber sees exactly this event's post-state. A later event
            // in the same atomic emit is not projected yet. A failed fetch
            // logs and broadcasts without the aggregate.
            let aggregate =
                match crate::core::store::fetch_thread_aggregate(&mut *tx, thread_id).await {
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
            let mut ancestor_aggregates = Vec::with_capacity(affected_ancestors.len());
            for ancestor_id in &affected_ancestors {
                match crate::core::store::fetch_thread_aggregate(&mut *tx, *ancestor_id).await {
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
            staged.push(StagedThreadEvent {
                event_id,
                seq,
                event: te,
                meta,
                aggregate,
                ancestor_aggregates,
                side_effects,
            });
        }

        // === Phase: PostCommit ===
        // Commit, then fan out. Every broadcast goes first, in sequence order,
        // each followed by its spawn request if it has one. Then each event's ancestor rebroadcasts, parent
        // wake and side effects follow, the side effects in their own
        // transactions. The rows are durable by now, so a panic here loses
        // only the fan-out.
        tx.commit().await?;
        let broadcast_created = Utc::now();
        let results = staged
            .iter()
            .map(|s| EmitResult {
                event_id: s.event_id,
                seq: s.seq,
                summary_version: s.aggregate.as_ref().map(|a| a.summary_version),
            })
            .collect();
        let mut fan_outs = Vec::with_capacity(staged.len());
        for s in staged {
            fan_outs.push((
                s.event_id,
                s.event.clone(),
                s.meta.actor.clone(),
                s.ancestor_aggregates,
                s.side_effects,
            ));
            let continuation = matches!(s.event, ThreadEvent::ContinuationRequested { .. });
            ticket.send(
                &self.event_tx,
                EmittedEvent {
                    event_id: s.event_id,
                    seq: Some(s.seq),
                    created: broadcast_created,
                    typed: BusEvent::Thread {
                        thread_id,
                        event: s.event,
                        meta: s.meta,
                    },
                    aggregate: s.aggregate,
                    depth,
                    emitting_trigger_id: emitting_trigger_id.clone(),
                },
            );
            // After its broadcast, so no subscriber sees the resume's own
            // events ahead of the request that started it.
            if continuation {
                self.send_spawn_request(SpawnRequest::Continue {
                    thread_id,
                    event_id: s.event_id,
                });
            }
        }
        for (event_id, event, actor, ancestor_aggregates, side_effects) in fan_outs {
            // Keeps the frontend's `meta.blockingDescendantCount` live, so the
            // thread menu's blocked Archive updates in real time. It rides the
            // `ChildrenCountChanged` transient with the ancestor's snapshot.
            for ancestor_agg in ancestor_aggregates {
                let ancestor_id = match Uuid::parse_str(&ancestor_agg.thread_id) {
                    Ok(id) => id,
                    Err(e) => {
                        crate::log!(
                            "[EventBus] Unparseable ancestor thread_id {:?}: {}, skipping rebroadcast",
                            ancestor_agg.thread_id,
                            e
                        );
                        continue;
                    }
                };
                let active = ancestor_agg.active_children_count;
                let total = ancestor_agg.total_children_count;
                self.send_children_count_event(ancestor_id, active, total, Some(ancestor_agg));
            }
            // After the broadcast, so a panic here cannot skip SSE delivery.
            self.notify_parent_if_child(thread_id, Some(event_id), &event, actor.as_ref())
                .await;
            // A new child: tell the parent its updated counts.
            if let ThreadEvent::MessageReceived {
                parent_thread_id: Some(pid),
                ..
            } = &event
            {
                self.broadcast_children_count(*pid).await;
            }
            // The aggregate already carries the section, so section changes
            // are never side effects.
            for effect in side_effects {
                if let Err(e) = Box::pin(self.emit(effect)).await {
                    crate::log!("[EventBus] Side-effect emit failed: {}", e);
                }
            }
        }
        Ok(Some(results))
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
                // A failed stamp costs an older "last seen". It must not keep
                // the presence row, which would hold off an OS push.
                if let Err(e) = crate::core::DeviceStore::mark_seen(&self.pool, device_id).await {
                    log!("[EventBus] DeviceHidden: last seen stamp failed for {device_id}: {e}");
                }
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
    ) -> Result<ThreadType, sqlx::Error> {
        let source: Option<String> =
            sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&mut **tx)
                .await?;
        Ok(ThreadType::from_source(
            source.as_deref().unwrap_or_default(),
        ))
    }

    /// Get the current stored section from thread_summaries.
    async fn get_current_section(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: &Uuid,
    ) -> Result<ArchiveState, sqlx::Error> {
        let section: Option<String> =
            sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&mut **tx)
                .await?;
        Ok(section
            .map(|s| ArchiveState::parse(&s))
            .unwrap_or(ArchiveState::Archived))
    }

    /// Refuse a `ThreadArchived` on a thread waiting on the user, or on the
    /// home thread.
    ///
    /// `FOR UPDATE` is load-bearing. A concurrent `UserQuestionAsked` holds the
    /// row until it commits, so this read waits and sees the parked status. A
    /// plain read would see the older snapshot and archive over the question.
    async fn check_archive_allowed(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: &Uuid,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let row: Option<(String, String, String, bool)> = sqlx::query_as(
            "SELECT source, archive_state, status, is_home FROM thread_summaries \
             WHERE thread_id = $1 FOR UPDATE",
        )
        .bind(thread_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((source, section, status, is_home)) = row else {
            return Ok(());
        };
        thread_lifecycle::check_archive_allowed(
            ThreadType::from_source(&source),
            ArchiveState::parse(&section),
            thread_lifecycle::ThreadStatus::parse(&status),
            is_home,
        )?;
        Ok(())
    }

    /// Refuse a `ThreadTitleGenerated` on the home thread. A missing row is a
    /// thread whose first event this is, and that is never the home thread.
    async fn check_title_generated_allowed(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        thread_id: &Uuid,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let row: Option<(String, String, bool)> = sqlx::query_as(
            "SELECT source, archive_state, is_home FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((source, section, is_home)) = row else {
            return Ok(());
        };
        thread_lifecycle::check_title_generated_allowed(
            ThreadType::from_source(&source),
            ArchiveState::parse(&section),
            is_home,
        )?;
        Ok(())
    }

    /// Whether `child_id` still has a row that no longer names `parent_id` as
    /// its parent: it was moved to top level (ADR 0278).
    ///
    /// A missing row is not a move. Deleting a child settles its parent with a
    /// card after the row may be gone (ADR 0252).
    ///
    /// `FOR UPDATE`, because the card's projection writes this row next. A
    /// shared lock would let a card and a detach both hold it, and both
    /// upgrades would then deadlock. Whichever locks first commits first.
    async fn child_was_moved_out(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        parent_id: Uuid,
        child_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        let row: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT parent_thread_id FROM thread_summaries WHERE thread_id = $1 FOR UPDATE",
        )
        .bind(child_id)
        .fetch_optional(&mut **tx)
        .await?;
        Ok(row.is_some_and(|parent| parent != Some(parent_id)))
    }

    /// Lock what a move to top level writes, and report whether `child_id`
    /// is still `parent_id`'s child. A missing row is no edge.
    ///
    /// The detach rebases `depth` over the child's subtree. So the rows are
    /// locked deepest first, the child last: a descendant's own terminal locks
    /// its row and then its parent's, and the same order cannot deadlock.
    async fn lock_edge_for_detach(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        parent_id: Uuid,
        child_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        let locked: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
            "WITH RECURSIVE subtree AS ( \
                SELECT $1::uuid AS thread_id \
                UNION \
                SELECT c.thread_id FROM thread_summaries c \
                JOIN subtree s ON c.parent_thread_id = s.thread_id \
             ) \
             SELECT t.thread_id, t.parent_thread_id FROM thread_summaries t \
             WHERE t.thread_id IN (SELECT thread_id FROM subtree) \
             ORDER BY t.depth DESC, t.thread_id \
             FOR UPDATE OF t",
        )
        .bind(child_id)
        .fetch_all(&mut **tx)
        .await?;
        Ok(locked
            .iter()
            .any(|(id, parent)| *id == child_id && *parent == Some(parent_id)))
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
