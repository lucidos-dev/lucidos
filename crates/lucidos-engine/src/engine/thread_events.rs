use crate::runtime::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Replay default for events persisted before the `agent` field existed —
/// all such events were Claude Code (the only agent at the time).
fn default_agent_kind_claude_code() -> AgentKind {
    AgentKind::ClaudeCode
}

fn is_empty_str(s: &str) -> bool {
    s.is_empty()
}
fn is_false(b: &bool) -> bool {
    !b
}
fn default_session_ended_reason() -> SessionEndReason {
    // Legacy DB rows persisted before `reason` was a required wire field lack
    // the field entirely. Removed non-terminal reasons that DO carry a value
    // deserialize via `#[serde(other)]` into `LegacyNonTerminal`; this default
    // covers the missing-field case for the same row class.
    SessionEndReason::LegacyNonTerminal
}

/// Backward-compat default for `MessageReceived.mode` on DB rows persisted
/// before the `mode` field existed. New emissions MUST set `mode` explicitly
/// — the API layer enforces this on incoming requests.
fn default_mode_human() -> ActorMode {
    ActorMode::Human
}

fn default_cancel_cause() -> CancelCause {
    CancelCause::Unknown
}

fn default_abort_cause() -> AbortCause {
    AbortCause::Unknown
}

fn default_true() -> bool {
    true
}

/// Single grep target for "where can a response be canceled?". `meta.actor`
/// should be `Some(_)` — cancel is user-driven by definition — but the
/// agentic_loop's chat-thread cancel path doesn't yet plumb the actor down,
/// so the contract is documented rather than asserted. Once that's fixed,
/// add `debug_assert!(meta.actor.is_some(), ...)`.
///
/// Idempotent against pre-emitted terminators: if `meta.request_event_id` is
/// set and a `ResponseGenerated`/`ResponseCanceled`/`ResponseAborted`/
/// `ResponseFailed` already exists for it, this is a no-op. Covers the
/// `/api/restart` race — `abort_in_flight_for_restart` pre-emits
/// `ResponseAborted{actor: device}` for the in-flight chat thread, then
/// cancels its token; the agentic loop's cancel branch fires moments later
/// and would otherwise stack a phantom `ResponseCanceled{UserStop}` boundary
/// on top of the "You — Restarted" panel.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_response_canceled(
    bus: &crate::engine::event_bus::EventBus,
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    cause: CancelCause,
    text: String,
    images: Vec<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    meta: EventMeta,
    log_tag: &str,
) {
    if let Some(req_id) = meta.request_event_id {
        if has_terminator_for(pool, thread_id, req_id).await {
            crate::log!(
                "[{}] Skipping ResponseCanceled — terminator already exists for request {} on thread {}",
                log_tag,
                req_id,
                thread_id
            );
            return;
        }
    }
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseCanceled {
                text,
                images,
                model,
                reasoning_effort,
                cause,
            },
            meta,
        },
        log_tag,
    )
    .await;
}

/// True iff a terminator (`ResponseGenerated`/`ResponseCanceled`/
/// `ResponseAborted`/`ResponseFailed`) already exists in the events table
/// for `(thread_id, request_event_id)`. Shared by `emit_response_canceled`
/// (skip-emit gate) and `agentic_loop::ensure_terminator_emitted`
/// (post-loop fallback gate). Fails open on DB error: returns `false` so
/// the caller still emits — a phantom terminal is much better than leaving
/// the UI stuck on "running" because the DB hiccupped.
pub(crate) async fn has_terminator_for(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    request_event_id: uuid::Uuid,
) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(\
            SELECT 1 FROM events \
            WHERE aggregate_id = $1 \
              AND event_type = ANY($3::text[]) \
              AND payload->>'request_event_id' = $2\
        )",
    )
    .bind(thread_id.to_string())
    .bind(request_event_id.to_string())
    .bind(ThreadEvent::TERMINATOR_EVENT_TYPES)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!(
            "[ThreadEvents] has_terminator_for query failed for request {} on thread {}: {}",
            request_event_id,
            thread_id,
            e
        );
        false
    })
}

/// Single grep target for "where can a response be aborted?". `meta.actor`
/// covers both pure system kills (`None` or `Some(MessageOrigin::system())`)
/// and engine-deliberate terminations carrying `Engine{reason}` for the chip.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_response_aborted(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: uuid::Uuid,
    cause: AbortCause,
    text: String,
    images: Vec<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    meta: EventMeta,
    log_tag: &str,
) {
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseAborted {
                text,
                images,
                model,
                reasoning_effort,
                cause,
            },
            meta,
        },
        log_tag,
    )
    .await;
}

/// Why a `ResponseCanceled` was emitted. Cancellation is always a user-driven
/// action that interrupts a *real* in-flight response — the actor on
/// `EventMeta` identifies the user, this enum identifies what they did. New
/// emit sites must specify the cause; `Unknown` only appears on legacy DB
/// rows persisted before the field existed.
///
/// If you want to settle a thread whose process is already gone, that's an
/// abort (`AbortCause::StaleSettle`), not a cancel — nothing was running to
/// cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelCause {
    /// User clicked the Stop button on a running response.
    UserStop,
    /// User clicked Apply / Discard / Archive on a CC session that was still
    /// running — the action implies "stop the current turn first."
    UserAction,
    /// Pre-typed-cause legacy event, or a now-removed cause string (e.g. the
    /// retired `stale_settle` cancel cause that's now an abort cause). Catches
    /// anything unrecognized so old DB rows replay cleanly. Never emit fresh.
    #[serde(other)]
    Unknown,
}

/// Why a `ResponseAborted` was emitted. Aborts are system-driven cleanup —
/// the engine or the OS terminated the process, or the engine settled a
/// projection whose live process was already gone. The actor on `EventMeta`
/// records *who triggered* the cleanup (a user button can fire stale-settle),
/// not who decided to terminate the work — that's always the system. New emit
/// sites must specify the cause; `Unknown` only appears on legacy DB rows
/// persisted before the field existed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortCause {
    /// Engine is shutting down — every running CC session gets a clean abort
    /// so the next process can resume from a known state.
    EngineShutdown,
    /// Safety net fired in `run_session`: CC's event loop ended without a
    /// `Result` event (process crash, stream EOF before Result, parser
    /// glitch). The thread surfaces in error state; any commits CC made
    /// before dying stay on the branch but are NOT proposed as a change
    /// (see `SessionEndAction::CrashedKeepBranch`).
    SafetyNet,
    /// Engine started up and found a session marked `running` with no live
    /// process — recovery emitted an abort to settle the projection.
    RecoveryAfterRestart,
    /// CC subprocess died unexpectedly (OS signal, panic, external `kill`).
    ProcessKilled,
    /// Engine settled a thread the projection still showed as `running` but
    /// for which no live process existed. Surfaces a stuck UI; not a real
    /// process kill (the process was already gone). The user's action that
    /// exposed the stuck row (Stop / Apply / Discard / Archive / Interrupt)
    /// flows through as the actor, but no real response was canceled — the
    /// thread is just being cleaned up.
    StaleSettle,
    /// Pre-typed-cause legacy event or unrecognized cause string. Never emit
    /// fresh.
    #[serde(other)]
    Unknown,
}

impl AbortCause {
    /// True when the abort is expected to be followed by a fresh `SessionStarted`
    /// (engine shutdown, recovery sweep) — the child is mid-retry, not done.
    /// Callers must NOT decrement the parent's `active_children_count` or fire
    /// the completion callback in this case, or the resumed child's eventual
    /// `CodingAgentIdled` would be orphaned.
    ///
    /// `SafetyNet`, `ProcessKilled`, and `StaleSettle` are NOT transient: the
    /// thread sits in error state (or, for stale-settle, was already done and
    /// is just being projection-cleaned) and no fresh `SessionStarted` will
    /// follow — the parent's counter must drop so it doesn't display as Active
    /// forever. `Unknown` is legacy; treat as terminal so the prior
    /// decrement-on-abort behavior holds for old DB rows.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::EngineShutdown | Self::RecoveryAfterRestart)
    }

    /// SQL fragment for the `status` column on the `thread_summaries` row when
    /// this abort lands. Most aborts surface a red `failed` indicator (with
    /// pending changes overriding to `waiting`); `StaleSettle` is engine
    /// cleanup of a stuck row whose process was already gone — fired by a user
    /// button (Stop / Apply / Discard / Archive / Interrupt). No real abort
    /// happened, so it uses the cancel-style mapping (idle, or waiting if
    /// pending changes) rather than the red failed indicator.
    pub fn status_sql(&self) -> &'static str {
        match self {
            Self::StaleSettle => crate::engine::event_bus::STATUS_FROM_CC_HAS_CHANGES,
            _ => "CASE WHEN cc_has_changes THEN 'waiting' ELSE 'failed' END",
        }
    }
}

/// Who semantically drove this request:
/// - `Human`: a person clicked/typed (device, API client, or upstream workspace's human)
/// - `Agent`: an LLM decided (parent thread's agent spawned this, upstream workspace's agent called us)
/// - `Engine`: the runtime/scheduler/recovery decided (no LLM in the loop)
///
/// Orthogonal to `MessageOrigin` (which captures *where* the request entered).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorMode {
    Human,
    Agent,
    Engine,
}

impl ActorMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
            Self::Engine => "engine",
        }
    }
}

/// Why the engine itself spawned a piece of work without a human or agent prompting it.
/// Surfaced in the route popover so users can see "Engine · auto-resumed after restart".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineReason {
    /// Resume after an interrupted response (chat/CC). Past name was
    /// `session_recovered` — kept as serde alias for old DB rows. Renamed
    /// alongside the `ContinuationStarted` event for the same reasons
    /// (session-vs-thread ambiguity; the new response is a continuation, not a
    /// recovery of the prior one).
    #[serde(alias = "session_recovered")]
    ContinuationStarted,
    /// Generic orphan thread recovery (non-CC).
    OrphanRecovery,
    /// Scheduled trigger fired. `trigger_id` is the trigger's user-facing
    /// `config.id` (a string — typically a v4 UUID) so the route popover can
    /// match it directly against `/api/v1/triggers`.
    Scheduler {
        trigger_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger_name: Option<String>,
    },
    /// Engine auto-retriggered `/harden` because the marker was stale.
    HardenRetrigger,
    /// Stale CC session detected on startup; changes proposed from its branch.
    StaleSession,
    /// Engine detected a merge conflict pulling main into a CC branch.
    MergeConflict,
    /// Engine detected the harden marker is missing or stale before apply.
    MissingHardening,
}

/// Direction of a `ThreadLink` origin: which end of the parent⇄child
/// relationship the linked thread sits on relative to the receiving thread.
///
/// - `Parent` — the linked thread *spawned* the receiving thread. Used on
///   the child's first MessageReceived to attribute the spawn.
/// - `Child`  — the linked thread is a *child* of the receiving thread, posting
///   a callback (e.g. `[Child thread completed] ...`) back into the parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadDirection {
    Parent,
    Child,
}

fn default_thread_direction_parent() -> ThreadDirection {
    // Historical DB rows only ever carried the parent direction (the variant
    // used to be named ParentThread). New writes always set it explicitly.
    ThreadDirection::Parent
}

/// Where an inbound `MessageReceived` originated. Stamped at the HTTP boundary
/// in `api/chat.rs::chat_submit`. Optional on the wire so old DB rows
/// deserialize cleanly; the frontend has a `legacyOrigin()` fallback that
/// synthesizes Device / ThreadLink from the legacy `device_id` /
/// `parent_thread_id` fields when origin is missing.
///
/// Invariants enforced at construction in `engine/chat/events.rs`:
/// - `Device { .. }`            ⇒ `mode == ActorMode::Human` (intrinsic)
/// - `Api { mode, .. }`         ⇒ `mode == carried mode` (Human, Agent, or Engine)
/// - `Workspace { mode, .. }`   ⇒ `mode == carried mode`
/// - `ThreadLink { mode, .. }`  ⇒ `mode == carried mode`
/// - `Engine { .. }`            ⇒ `mode == ActorMode::Engine` (intrinsic)
///
/// Workspace caller_* body fields are display hints only — they are user-controllable
/// (any HTTP client can send them) and MUST NOT be used for authorization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageOrigin {
    /// Mode = Human. Human at a known device. `device_id` is the TEXT primary
    /// key from the `devices` table (not necessarily a UUID).
    Device { device_id: String, label: String },
    /// HTTP request without a `device_id` and without a `caller_workspace`
    /// body field. Mode is carried explicitly so SDK callers can declare
    /// themselves as agent/engine; defaults to Human for back-compat.
    Api {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_agent: Option<String>,
        /// Defaults to Human because all pre-existing API calls were human-initiated;
        /// agent/engine-initiated API calls must set this explicitly.
        #[serde(default = "default_api_mode")]
        mode: ActorMode,
    },
    /// Mode = Human or Agent (carried in `mode`). HTTP request from another
    /// Lucidos workspace, identified by the `caller_workspace` body field.
    Workspace {
        workspace: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_agent: Option<String>,
        /// Upstream intent. Defaults to Human because all pre-existing
        /// cross-workspace calls were human-initiated; agent/engine-initiated
        /// cross-workspace calls must set this explicitly via the body's `mode`
        /// field. Read by `MessageOrigin::mode()` to derive the request's `ActorMode`.
        #[serde(default = "default_workspace_mode")]
        mode: ActorMode,
    },
    /// Mode = Human, Agent, or Engine (carried in `mode`). Bidirectional link
    /// to another thread in the same workspace. `direction = Parent` means
    /// the linked thread *spawned* the receiving thread; `direction = Child`
    /// means the linked thread is a child posting a callback back into the
    /// receiving (parent) thread.
    ///
    /// `serde(alias = "parent_thread")` keeps historical DB rows readable —
    /// they all originate from the unidirectional `ParentThread` variant, so
    /// `direction` defaults to `Parent` when missing.
    #[serde(alias = "parent_thread")]
    ThreadLink {
        thread_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawning_event_id: Option<uuid::Uuid>,
        /// Defaults to Agent because all pre-existing thread links were
        /// LLM-driven; engine-initiated links must set this explicitly.
        /// Read by `MessageOrigin::mode()` to derive the request's `ActorMode`.
        #[serde(default = "default_thread_link_mode")]
        mode: ActorMode,
        #[serde(default = "default_thread_direction_parent")]
        direction: ThreadDirection,
    },
    /// The engine itself initiated this work (recovery, scheduler, retrigger).
    /// Implies `ActorMode::Engine`.
    Engine {
        reason: EngineReason,
    },
    /// The host system killed the underlying process (engine shutdown, OS signal,
    /// crash, safety net catch). The engine only marks the abort — it didn't
    /// decide to abort. Distinct from `Engine` which represents engine-deliberate
    /// actions (hardening retrigger, merge conflict, scheduler). Renders as
    /// "System" in the UI. Implies `ActorMode::Engine` (deterministic, non-human,
    /// non-agent), but the chip label differentiates from engine-deliberate work.
    System,
}

fn default_workspace_mode() -> ActorMode {
    ActorMode::Human
}

fn default_thread_link_mode() -> ActorMode {
    ActorMode::Agent
}

fn default_api_mode() -> ActorMode {
    ActorMode::Human
}

fn default_inject_mode() -> ActorMode {
    // Historical UserPromptInjected rows pre-date the mode field. The only
    // emit site at the time was the user-correction path, so defaulting to
    // Human keeps legacy rows attributed correctly.
    ActorMode::Human
}

impl MessageOrigin {
    /// Derive the actor mode from the origin variant. For `Workspace` and
    /// `ThreadLink` this reads the carried field; for the rest the mode is
    /// intrinsic to the variant.
    pub fn mode(&self) -> ActorMode {
        match self {
            Self::Device { .. } => ActorMode::Human,
            Self::Api { mode, .. } => *mode,
            Self::Workspace { mode, .. } | Self::ThreadLink { mode, .. } => *mode,
            Self::Engine { .. } | Self::System => ActorMode::Engine,
        }
    }

    /// Convenience constructor for engine-initiated origins. Tightens emit sites
    /// like `Some(MessageOrigin::engine(EngineReason::SessionRecovered))`.
    pub fn engine(reason: EngineReason) -> Self {
        Self::Engine { reason }
    }

    /// Convenience constructor for system-killed-process aborts.
    /// Use for `ResponseAborted` paths where the underlying process died
    /// (engine shutdown, safety-net catch, recovery after restart, OS signal).
    /// NOT for engine-deliberate actions like hardening retrigger or scheduler.
    pub fn system() -> Self {
        Self::System
    }

    /// Convenience constructor for child→parent callback origins. The
    /// receiving thread is the parent; `child_thread_id` is the child whose
    /// completion produced the message. Title/spawning_event_id default to
    /// `None` since the parent already has its own thread context.
    pub fn thread_link_child(child_thread_id: uuid::Uuid, mode: ActorMode) -> Self {
        Self::ThreadLink {
            thread_id: child_thread_id,
            title: None,
            spawning_event_id: None,
            mode,
            direction: ThreadDirection::Child,
        }
    }
}

/// Source channel for events — determines thread type and routing.
/// The `CodingAgent` variant serializes as `"claude_code"` for wire-format
/// continuity with persisted events and the frontend; rename pending a
/// coordinated migration of all consumers.
///
/// `Trigger` is the umbrella for all trigger-driven runs (scheduled, event,
/// hybrid). The actual invocation that fired a given run is recorded on
/// `TriggerStarted.invocation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventChannel {
    Chat,
    #[serde(rename = "claude_code")]
    CodingAgent,
    #[serde(alias = "scheduled_trigger")]
    Trigger,
}

impl EventChannel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::CodingAgent => "claude_code",
            Self::Trigger => "trigger",
        }
    }
}

/// Records *which path* fired a particular trigger run.
///
/// A trigger config can have cron-only (`schedule`), event-only (`event`), or
/// both (`hybrid`). When the scheduler dispatches a run it knows exactly which
/// path won — this enum captures that for the popover panel and any consumer
/// that wants to reason about the actual invocation rather than the config
/// shape.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum TriggerInvocation {
    /// Cron schedule fired this run.
    Schedule,
    /// A domain event fired this run. `event_type` is the matched event name;
    /// `event_id` is the source `events.id` (when known) so the popover can
    /// deep-link back to the originating event row.
    Event {
        event_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<uuid::Uuid>,
    },
}

/// One option offered by CC's AskUserQuestion tool. Persisted inside
/// `UserQuestionAsked` and looked up when the user picks one to send the
/// matching `tool_result` back to CC.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// How the user answered a `UserQuestionAsked`. Tagged so the JSON payload
/// is `{ "kind": "Selected", "option_id": "..." }` etc.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum AnswerKind {
    Selected {
        option_id: String,
    },
    FreeText {
        text: String,
    },
    /// Multi-select answer. `text` carries optional freetext typed alongside
    /// the toggled options — the prompt textarea folds into the answer when a
    /// multi-select question is pending. Backend joins the resolved labels and
    /// the freetext together when relaying to CC. Either side may be empty
    /// (but not both — see `validate_answer`).
    MultiSelected {
        option_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Canceled,
}

/// Why a session ended — used by frontend for status/display logic.
///
/// Mostly terminal-only (Phase 4 of the CC resume architecture): every CC turn
/// ends with `CodingAgentIdled` as a turn boundary; `SessionEnded` fires when
/// the thread is truly done. Removed non-terminal reasons (`Completed`,
/// `ChangesProposed`, `ChangesApplied`, `AutoEnded`, `UserEnded`, `Discarded`)
/// survive in the DB on legacy rows — they deserialize as `LegacyNonTerminal`
/// via `#[serde(other)]` so old data doesn't crash.
///
/// `StaleResume` is the one transient exception: emitted when CC returns an
/// empty Result against a stale `--resume` token. The chat handler retries
/// internally with a fresh session; `event_bus` skips the status flip and the
/// frontend skips the AbortPanel so the user doesn't see a phantom "Aborted"
/// during the retry window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEndReason {
    /// Engine graceful stop. Frontend renders as "Aborted".
    Shutdown,
    /// CC subprocess crashed irrecoverably.
    Panic,
    /// User explicitly closed the thread (Phase 10 will wire the UI; no emit
    /// site uses this yet, but the variant is reserved so the frontend can
    /// branch on it.)
    Closed,
    /// CC's `--resume <sid>` returned an empty Result — the prior session is
    /// gone. The engine emits this so restart-recovery's auto-detect resolver
    /// (`resolve_resume_context`) sees SessionEnded as the latest lifecycle
    /// event and does NOT try to resume the stale sid; the chat handler then
    /// retries the user's message against a fresh session. Frontend treats
    /// this as a transient lifecycle marker — no "Aborted" display, status
    /// stays `running` until the retry's SessionStarted lands.
    StaleResume,
    /// Catch-all for legacy DB rows persisted before this enum was reduced to
    /// terminal-only reasons. Treat as a harmless terminal end on read; never
    /// emit going forward.
    #[serde(other)]
    LegacyNonTerminal,
}

impl SessionEndReason {
    /// True when the session-end is mid-flight — the engine is still working on
    /// the current turn and a fresh `SessionStarted` is expected to follow.
    /// Callers should NOT treat the SessionEnded as a turn boundary in this
    /// case (don't flip status to terminal, don't decrement parent child
    /// counters, don't fire completion callbacks). Defaults to `false` so any
    /// future variant is treated as terminal unless explicitly opted in.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::StaleResume)
    }
}

/// Outcome of a child thread that just completed, surfaced to the parent via
/// `ThreadEvent::ChildThreadCompleted`. Replaces the prose discriminator
/// strings (`"completed with proposed changes"`, `"completed (no changes)"`,
/// `"completed"`, `"failed"`) the previous `[Child thread completed]`
/// callback embedded in its plaintext body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildCompletionStatus {
    /// Child finished its turn normally (with or without proposed changes).
    Success,
    /// Child terminated with `ResponseFailed` — the parent should treat the
    /// summary as an error string and decide whether to retry / give up.
    Failure,
    /// CC child idled with no pending changes proposed. Distinguished from
    /// `Success` so the parent can branch on "did the child actually produce
    /// reviewable work?" without re-querying the changes table.
    NoChanges,
    /// Child was canceled by the user (`ResponseCanceled`). The summary is
    /// the partial response text (or empty); the parent can decide whether to
    /// retry, prompt again, or give up. `ResponseAborted` (system-driven, e.g.
    /// engine restart) deliberately does NOT surface here — that case is
    /// transient and the engine resumes the child on next visit.
    Canceled,
}

/// Persisted thread events — stored in the DB with thread_id + sequence.
/// Names are past tense, matching the `event_type` column.
///
/// Every field needed for persistence AND SSE broadcast lives here.
/// New fields use `#[serde(default)]` so old DB rows deserialize safely.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ThreadEvent {
    // Chat
    MessageReceived {
        text: String,
        /// Content-addressed sha256 hashes of user-attached image blobs.
        /// Bytes live exactly once under `data/blobs/<hh>/<hash>.<ext>`;
        /// the LLM call resolves hashes to bytes at send time. Old DB rows
        /// migrate from `images: [{base64, mime_type}, ...]` via the
        /// startup migration in `core::image_migration`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        user_image_hashes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image_description: Option<String>,
        /// Set when this thread was spawned by another thread. Required when
        /// `mode != Human` and the spawn originated from a parent thread.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_thread_id: Option<uuid::Uuid>,
        /// Event in the parent thread that triggered this spawn (e.g. the
        /// `ToolCalled` event for a `run_thread` invocation). Only set when
        /// `mode != Human`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawning_event_id: Option<uuid::Uuid>,
        /// Semantic mode of the actor that emitted this message. Required for
        /// new emissions (enforced at the API layer). The serde default exists
        /// only to replay old DB rows persisted before the `mode` field existed.
        #[serde(default = "default_mode_human")]
        mode: ActorMode,
        /// Model the engine will use to answer this message. Stamped at request
        /// time so the route tooltip can display it before ResponseGenerated
        /// fires. CC sessions still rely on CodingAgentSettingsChanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Reasoning effort the engine will use to answer this message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        /// Structured origin — captured from HTTP headers / device lookup at
        /// the API boundary. Optional on the wire so old DB rows deserialize
        /// cleanly; the frontend's `legacyOrigin()` synthesizes from the
        /// legacy `device_id` / `parent_thread_id` fields when this is None.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    TextStreamed {
        text: String,
    },
    Thinking {
        text: String,
    },
    /// One captured context per LLM call. `usage` is None pre-call and on
    /// providers that don't report it (OpenAI, Gemini); when present it
    /// reflects the real prompt-token cost from the provider's `usage`
    /// block. `estimated_total_tokens` keeps the chars/4 estimate so
    /// the modal can show estimator drift.
    ContextCaptured {
        producer: crate::engine::ContextProducer,
        model: String,
        context_window: usize,
        sections: Vec<crate::engine::ContextSection>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tools: Vec<String>,
        estimated_total_tokens: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<crate::engine::ApiUsage>,
        #[serde(default, skip_serializing_if = "is_false")]
        trimmed: bool,
    },
    MemorySearched {
        #[serde(default)]
        results: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        queries: Vec<String>,
    },
    ToolCalled {
        name: String,
        args: Value,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        description: String,
    },
    ToolResult {
        name: String,
        result: String,
        #[serde(default)]
        images: Vec<String>,
        #[serde(default = "default_true")]
        success: bool,
    },
    /// Background bash task spawned via `run_bash_background`. Paired with
    /// a later `BackgroundBashCompleted`. The two events are the durable
    /// audit trail of every long-running shell command — `bash_output`
    /// reads from the in-memory registry while a task runs and falls back
    /// to the `BackgroundBashCompleted` payload after the task is evicted.
    BackgroundBashStarted {
        task_id: String,
        command: String,
        timeout_secs: u64,
        started_at: chrono::DateTime<chrono::Utc>,
    },
    BackgroundBashCompleted {
        task_id: String,
        /// Truncated to 200 chars for log readability; full command lives
        /// on the paired `BackgroundBashStarted`.
        command: String,
        /// `None` when the watchdog killed the child on timeout — a
        /// signal-only exit gives no usable code on macOS.
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
        started_at: chrono::DateTime<chrono::Utc>,
        finished_at: chrono::DateTime<chrono::Utc>,
        /// Watchdog killed the task because `timeout_secs` elapsed.
        #[serde(default, skip_serializing_if = "is_false")]
        timed_out: bool,
        /// `bash_kill` killed the task explicitly.
        #[serde(default, skip_serializing_if = "is_false")]
        killed: bool,
    },
    ResponseGenerated {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
    },
    ResponseCanceled {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default = "default_cancel_cause")]
        cause: CancelCause,
    },
    ResponseAborted {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default = "default_abort_cause")]
        cause: AbortCause,
    },
    ResponseFailed {
        error: String,
    },

    // Resume-after-abort boundary. Opens a new exchange in the timeline whose
    // body is the rerun (chat: SessionRecovered's predecessor → engine note +
    // re-LLM call; CC: --resume into the same cc_session_id). Past name was
    // SessionRecovered (and SessionResumed before that) — kept as serde aliases
    // so older DB rows still deserialize. Renamed to ContinuationStarted
    // because (a) "session" was ambiguous between chat and CC, and (b) the
    // resumed response is actually a *new* response continuing the prior
    // attempt, not the prior response coming back to life.
    #[serde(alias = "SessionRecovered", alias = "SessionResumed")]
    ContinuationStarted {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        branch: String,
        /// Engine-stamped origin so the route popover can render
        /// "Engine · Auto-resumed after restart" for recovered sessions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    SessionStarted {
        session_id: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        branch: String,
        /// External repository ID this session is bound to.
        /// Persisted in thread_summaries so follow-ups reuse the same repo.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo_id: Option<String>,
    },
    SessionEnded {
        /// Why the session ended. Always serialized — the frontend reads
        /// `reason` to distinguish a normal completion from a system abort,
        /// and `Completed` must reach the wire as `"completed"`. The default
        /// applies only when reading old DB rows persisted before `reason`
        /// existed.
        #[serde(default = "default_session_ended_reason")]
        reason: SessionEndReason,
    },
    #[serde(alias = "ClaudeCodeTextStreamed")]
    CodingAgentTextStreamed {
        text: String,
        #[serde(default = "default_agent_kind_claude_code")]
        agent: AgentKind,
    },
    #[serde(alias = "ClaudeCodeToolCalled")]
    CodingAgentToolCalled {
        name: String,
        args: Value,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        description: String,
        #[serde(default = "default_agent_kind_claude_code")]
        agent: AgentKind,
        /// Agent-issued identifier for this tool invocation, persisted so the
        /// matching `CodingAgentToolResult` can be paired with the call even
        /// when an `EXCHANGE_START_TYPES` event (e.g. permission prompt)
        /// splits them across exchanges. Empty for legacy DB rows.
        #[serde(default, skip_serializing_if = "is_empty_str")]
        tool_use_id: String,
    },
    #[serde(alias = "ClaudeCodeToolResult")]
    CodingAgentToolResult {
        name: String,
        result: String,
        #[serde(default = "default_agent_kind_claude_code")]
        agent: AgentKind,
        /// Matches the originating `CodingAgentToolCalled.tool_use_id`.
        /// Empty for legacy DB rows.
        #[serde(default, skip_serializing_if = "is_empty_str")]
        tool_use_id: String,
    },
    #[serde(alias = "ClaudeCodeUserMessageSent")]
    CodingAgentUserMessageSent {
        text: String,
        #[serde(default = "default_agent_kind_claude_code")]
        agent: AgentKind,
    },
    /// Automated prompt sent to a coding agent (e.g., conflict resolution, hardening).
    /// Persisted for audit trail but not rendered in the chat UI.
    #[serde(alias = "ClaudeCodePromptSent")]
    CodingAgentPromptSent {
        text: String,
        #[serde(default = "default_agent_kind_claude_code")]
        agent: AgentKind,
        /// Engine-stamped origin for prompts the engine itself synthesized
        /// (orphan recovery, hardening retrigger, merge conflict). Surfaced in
        /// the route popover so users can distinguish engine-driven prompts
        /// from agent-driven ones. `None` for legacy DB rows and for prompts
        /// that already carry their origin elsewhere in the chain.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    /// Emitted when the engine detects that a coding-agent session ended without
    /// running the required hardening. A recovery hardening session is spawned
    /// automatically. This is NOT a completion event — the thread stays active
    /// until hardening finishes.
    MissingHardeningDetected {
        /// Engine-stamped origin (always `MessageOrigin::Engine { reason:
        /// EngineReason::MissingHardening }` for new emits). Optional on the
        /// wire so legacy DB rows decode cleanly.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    #[serde(alias = "ClaudeCodeIdled")]
    CodingAgentIdled {
        #[serde(default, skip_serializing_if = "is_false")]
        has_changes: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        is_external_repo: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        requires_restart: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cc_session_id: Option<String>,
        #[serde(default = "default_agent_kind_claude_code")]
        agent: AgentKind,
        /// Optional short tag describing why this idle was emitted. Most idles
        /// have no `reason` (the agent simply finished its turn). Recovery
        /// emits `Some("engine_restart_interrupt")` when a mid-turn-crashed
        /// session is surfaced to the UI as "interrupted, click to continue"
        /// instead of being auto-spawned. The frontend reads this field to
        /// render the continue affordance.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Absolute filesystem path of the worktree the agent ran in for this
        /// turn. Phase 6.1 of the CC resume architecture: persisted so that
        /// follow-up turns can look up the deterministic per-thread worktree
        /// directly from the events stream instead of scanning
        /// `git worktree list`. `None` on legacy rows that predate the field
        /// and on out-of-band idles emitted without a worktree (e.g. the
        /// "no branch" recovery path).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_path: Option<String>,
        /// `git rev-parse HEAD` in the worktree at the moment the agent went
        /// idle. Phase 8.1 of the CC resume architecture: persisted so that
        /// the next spawn can diff against this SHA + check `git status` to
        /// detect external user edits made between turns and inject a note
        /// into the resumed prompt. `None` on legacy rows, on idles emitted
        /// without a worktree, or when `git rev-parse` fails (e.g. branch
        /// has zero commits yet).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_head_sha: Option<String>,
    },
    /// Continuation signal — emitted when a CC turn that was interrupted
    /// (engine restart mid-turn, Q9a recovery path) needs to be resumed
    /// without a new user message. Picked up by the spawn dispatcher
    /// (Phase 5, Task 5.2), which re-spawns CC via `--resume` with no
    /// new input. The dispatcher uses the event's id as the idempotency
    /// key so a single `ContinueSignal` produces exactly one spawn.
    ///
    /// `reason` is a short tag describing why a continuation was needed
    /// (e.g. `"engine_restart_interrupt"`); it is purely informational and
    /// surfaced for debugging / route-popover context.
    ContinueSignal {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        reason: String,
    },

    // Thread lifecycle
    ThreadTitleGenerated {
        title: String,
    },
    ThreadTitleRenamed {
        title: String,
    },
    ThreadSaved,
    ThreadUnsaved,
    ThreadArchived,
    /// A thread was created in `composing` state. Emitted by the first
    /// successful POST /threads (debounced first user input — keystroke,
    /// image attach, or mode toggle on a fresh compose). The thread can
    /// be addressed by id immediately after this event lands.
    ThreadStarted {
        /// Initial mode the user opened compose with. Mutable while the
        /// thread is `Composing`; locked on first `MessageReceived`.
        mode: String,
        /// Stamped by `api::actor::user_actor_resolved` so the timeline
        /// shows which device started the draft thread.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A thread in `composing` state was explicitly discarded. Emitted by
    /// DELETE /threads/:id. Terminal — the state-machine guard rejects
    /// every subsequent compose PUT and message POST with 410 Gone, which
    /// is the "make impossible states impossible" lever that replaces the
    /// old LWW + tombstone machinery.
    ThreadDiscarded {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A user attached an image to this thread's compose draft. Emitted by
    /// POST /api/v1/threads/:id/blobs after the bytes are content-addressed
    /// to disk under `data/blobs/<hh>/<hash>.<ext>`. The `hash` is the sole
    /// identity used by every downstream consumer (compose payload, message
    /// payload, LLM call); `mime` and `byte_size` are convenience fields so
    /// SSE subscribers can render the upload entry without fetching the
    /// blob. Same blob attached to two threads = two events, one per thread
    /// (the disk write is a no-op the second time, but the per-thread fact
    /// stays distinct).
    ImageUploaded {
        hash: String,
        mime: String,
        byte_size: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    TriggerStarted {
        #[serde(alias = "task_id")]
        trigger_id: String,
        #[serde(default, alias = "task_name", skip_serializing_if = "Option::is_none")]
        trigger_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        /// Which path fired this run. `None` only on legacy DB rows persisted
        /// before this field existed — new emissions always set it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation: Option<TriggerInvocation>,
        /// Engine-stamped origin for scheduler-fired triggers — always set to
        /// `Engine { Scheduler { trigger_id, trigger_name } }` so the route
        /// popover can render "Engine · Scheduled · <name>". `None` only on
        /// legacy DB rows persisted before this field existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        /// Snapshot of the firing trigger's `go_to_review` flag. When true,
        /// the section transition logic treats this trigger thread as
        /// top-level so its terminal event surfaces it in REVIEW. Defaults
        /// to false for backward compat with pre-flag DB rows.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        go_to_review: bool,
    },
    TriggerCompleted {
        #[serde(alias = "task_id")]
        trigger_id: String,
        #[serde(default, alias = "task_name", skip_serializing_if = "Option::is_none")]
        trigger_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_summary: Option<String>,
    },

    // Changes — change_id is the primary identifier
    ChangeProposed {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        requires_restart: bool,
        /// Engine-stamped origin for change proposals from engine-internal
        /// recovery paths (stale-session cleanup, orphan worktree cleanup).
        /// `None` for proposals authored by a live agent session — those
        /// inherit their origin from the surrounding `MessageReceived`.
        /// Surfaced in the route popover so users can render
        /// "Engine · Stale session cleanup" etc.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        /// Per-commit emission (Phase 4.2): set to the commit SHA when a
        /// `post-commit` hook in the CC worktree fires. `None` for legacy
        /// aggregate emits from engine-internal recovery paths. Multiple
        /// per-commit `ChangeProposed` events may share the same `change_id`
        /// (the branch's pending change) — `commit_sha` is the unique key
        /// for the commit itself. The frontend can group by `change_id` and
        /// display per-commit details when `commit_sha` is set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit_sha: Option<String>,
        /// Branch the change lives on. Stamped by the first
        /// `ChangeProposed` per `change_id`; used by the projection to
        /// reconstruct the row without consulting the legacy `changes` table.
        #[serde(default, skip_serializing_if = "is_empty_str")]
        branch_name: String,
        /// Repo root the change targets. Same lifetime/intent as `branch_name`.
        #[serde(default, skip_serializing_if = "is_empty_str")]
        repo_root: String,
        /// `true` if the originating commit landed on a hardened HEAD; surfaced
        /// in the projection so the apply UI can short-circuit re-hardening.
        /// Per-commit emits stamp this; aggregate recovery emits leave it false.
        #[serde(default, skip_serializing_if = "is_false")]
        hardened: bool,
        /// `true` when the proposing CC turn ended in `ResponseFailed`
        /// (mid-stream API drop, panic) — the worktree contents are whatever
        /// CC happened to dirty before the failure, not a deliberate
        /// completion. The frontend reads this to confirm before Apply so
        /// the user knows they're landing partial work. Per-commit emits
        /// always stamp `false` (the failure determination only happens at
        /// the aggregate emit fired after the terminal event).
        #[serde(default, skip_serializing_if = "is_false")]
        incomplete: bool,
        // Legacy fields — kept for backward compat with old DB rows
        #[serde(default, skip_serializing_if = "is_empty_str")]
        path: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        diff: String,
    },
    ChangeApplied {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "is_false")]
        requires_restart: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        client_update: bool,
        /// Commit subjects merged to main, oldest first. Empty for no-op applies.
        /// Surfaced in the restart-required toast grouped by thread.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        commits: Vec<String>,
        /// Title of the originating thread, included so the restart toast can
        /// group entries without an extra lookup.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_title: Option<String>,
        /// Who applied the change — always the human/HTTP/SDK initiator
        /// stamped by `api/actor::build_message_origin` from the originating
        /// HTTP call. None on legacy DB rows or when the conflict-resolution
        /// ff-merge path runs (the original applier's actor is dropped across
        /// the async gap; popover then renders "Unknown").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
        /// SHA of `main` before the merge — paired with `post_merge_sha` to
        /// give Revert the exact commit range to drop. `None` for events
        /// emitted before the projection rewrite.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_merge_sha: Option<String>,
        /// SHA of `main` after the merge.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        post_merge_sha: Option<String>,
        // Legacy
        #[serde(default, skip_serializing_if = "is_empty_str")]
        path: String,
    },
    ChangeDiscarded {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
        // Legacy
        #[serde(default, skip_serializing_if = "is_empty_str")]
        path: String,
    },
    ChangeReverted {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
        // Legacy
        #[serde(default, skip_serializing_if = "is_empty_str")]
        path: String,
    },
    ChangeApplyFailed {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },

    // Merge conflict resolution
    MergeConflictDetected {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
        /// Engine-stamped origin (always `MessageOrigin::Engine { reason:
        /// EngineReason::MergeConflict }` for new emits). Optional on the wire
        /// so legacy DB rows decode cleanly.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    /// A merge-resolution worktree was set up for a change with conflicts.
    /// The projection treats this as the change's `merge_worktree_path` /
    /// `merge_temp_branch` until a `MergeResolutionCleared` arrives. Survives
    /// restart so startup cleanup can find dangling worktrees.
    MergeResolutionStarted {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        worktree_path: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        temp_branch: String,
    },
    /// The merge-resolution worktree was torn down (cleanup finished).
    MergeResolutionCleared {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
    },
    /// A change's working tree was hardened (`/harden` marker stamped on
    /// HEAD). Idempotent: re-emitting after an unrelated commit is fine,
    /// the projection treats only the latest event per `change_id`.
    /// Downgraded to false implicitly when a fresh `ChangeProposed` arrives
    /// with `hardened: false`.
    ChangeHardened {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },

    /// Coding-agent session settings changed mid-session (model, reasoning effort,
    /// or permission mode). Persisted per-thread so settings survive idle exit
    /// and respawn.
    #[serde(alias = "CCSettingsChanged")]
    CodingAgentSettingsChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_mode: Option<String>,
        #[serde(default = "default_agent_kind_claude_code")]
        agent: AgentKind,
    },

    // Mid-flight injection — a user correction OR a system message (e.g. parent
    // thread receiving a child's [Child thread completed] callback) sent into
    // the agentic loop. `mode` and `origin` describe the actor; both default
    // for back-compat with old DB rows that pre-date these fields.
    UserPromptInjected {
        text: String,
        #[serde(default = "default_inject_mode")]
        mode: ActorMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        // None for engine-driven injections (resume notes from chat/rerun.rs)
        // and for legacy DB rows that pre-date this field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        injected_message_id: Option<uuid::Uuid>,
    },

    // Interactive (persisted)
    CredentialRequested {
        provider: String,
    },
    McpConsentRequested {
        tool: String,
        args: Value,
    },

    // CC AskUserQuestion — interactive question with selectable options.
    // Emitted when CC's built-in `AskUserQuestion` tool fires. The CC subprocess
    // is killed after this event; resume happens via POST /api/cc/answer-question
    // which emits UserQuestionAnswered and respawns CC with --resume + tool_result.
    //
    // `worktree_path` is the absolute path of the CC worktree at intercept time.
    // CC stores session JSONLs keyed by CWD (`~/.claude/projects/<encoded-cwd>/<sid>.jsonl`),
    // so resume MUST start the new subprocess in the same directory or `--resume`
    // returns "No conversation found". Branch lookup is unreliable here because CC
    // is free to `git checkout -b ...` inside the worktree — see
    // `engine/agent_session/run_session.rs` for how this is used on resume.
    UserQuestionAsked {
        tool_use_id: String,
        cc_session_id: String,
        question: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<QuestionOption>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_path: Option<String>,
        #[serde(default)]
        multi_select: bool,
    },
    UserQuestionAnswered {
        tool_use_id: String,
        answer: AnswerKind,
    },

    /// CC requested permission for a tool call (e.g. Edit/Write/Bash on a path
    /// outside cwd or under `.claude/`). Renders as a `PermissionCard` in the
    /// thread; the user's answer resolves the oneshot keyed by `request_id`
    /// in `Engine.pending_mcp_consent`. Persisted so the card survives reload.
    CodingAgentPermissionRequest {
        request_id: String,
        tool_use_id: String,
        tool_name: String,
        input: Value,
        summary: String,
    },
    /// User answered (or system timed out) a `CodingAgentPermissionRequest`.
    /// Emitted by the permission-prompt handler immediately after the oneshot
    /// resolves, before returning to the MCP subprocess.
    ///
    /// `persist_scope` records which scope the user picked when granting an
    /// "Always allow"-style click (`narrow` / `broad` / `session`). `None`
    /// covers Allow-once, Deny, and the recovery-emitted orphan resolution
    /// (engine doesn't know what the user *would* have picked). The frontend
    /// uses it to render the answered card with a check on the chosen button
    /// and strike-through on the rest, so reload reproduces the same view.
    CodingAgentPermissionResolved {
        request_id: String,
        allowed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        persist_scope: Option<crate::engine::claude_code::AllowScope>,
    },

    /// Background worktree cleanup happened on this thread (Phase 10.2).
    /// `tier=1` means build artifacts (`target/`, `node_modules/`,
    /// `.lucidos/cache/`) were stripped from a long-idle worktree; the
    /// worktree itself is still on disk. `tier=2` means the entire worktree
    /// directory was removed (long-idle, clean, unsaved). `freed_bytes` is
    /// a best-effort sum of file sizes reclaimed; on filesystems where
    /// metadata is partially unavailable it may be `0` even if real space was
    /// freed. `branch_deleted` is `true` when Tier 2 also dropped a
    /// fully-merged branch (Phase 10.3).
    WorktreeCleaned {
        tier: u8,
        freed_bytes: u64,
        #[serde(default, skip_serializing_if = "is_false")]
        branch_deleted: bool,
    },

    /// A child thread spawned by `run_thread` / `run_claude` finished its turn.
    /// Emitted on the **parent** thread by the EventBus fan-out fan-in path
    /// when the child reaches a terminal event (CC: `CodingAgentIdled` or
    /// `SessionEnded`; chat: `ResponseGenerated` / `ResponseFailed`).
    ///
    /// Replaces the previous prose-only `[Child thread completed] ...` user
    /// message that was injected via the `parent_callback_tx` channel. The
    /// channel still wakes the parent — the payload is now the typed event id
    /// rather than a prebuilt string.
    ChildThreadCompleted {
        child_thread_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_thread_title: Option<String>,
        status: ChildCompletionStatus,
        /// Free-form summary — prose body of the child's final
        /// `ResponseGenerated` (truncated to 2000 chars), or the failure error
        /// for `Failure`. Indexed by [`ThreadEvent::indexable_text`].
        summary: String,
        /// IDs of changes the child left in `pending` state. Empty for chat
        /// children and for CC children that ended without proposing anything.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_change_ids: Vec<String>,
    },

    /// The agent (LLM) explicitly asked to drop a prior `ToolCalled` (and its
    /// matching `ToolResult`) or `ChildThreadCompleted` from future resume
    /// context. Emitted by the `dismiss_from_context` tool handler after it
    /// validates that `dismissed_event_id` is a real, dismissible event in the
    /// same thread. The resume helper (`build_resume_tool_blocks_with_skip_ids`)
    /// honours these on every subsequent assembly.
    ContextDismissed {
        dismissed_event_id: uuid::Uuid,
    },

    // ---- Transient (present participle) — never persisted ----
    // Streaming state
    TextStreaming {
        text: String,
    },
    Retrying {
        reason: String,
    },
    PreambleCompleting,
    // Side-effect commands — trigger frontend modals/actions
    CredentialRequest {
        payload: String,
    },
    /// Plugin install request awaiting user confirmation. Carries the JSON
    /// preview emitted by `install_plugin` (manifest, file list, overwrites,
    /// optional `setup`) so the frontend can render the install panel.
    /// Resolved by `POST /api/v1/plugins/install/{install_id}/{confirm|cancel}`.
    PluginInstallRequest {
        payload: String,
    },
    /// Plugin uninstall request awaiting user confirmation. Carries the JSON
    /// preview emitted by `uninstall_plugin` (plugin name + version, file
    /// list partitioned into still-on-disk vs already-missing) so the
    /// frontend can render the uninstall panel. Resolved by
    /// `POST /api/v1/plugins/uninstall/{uninstall_id}/{confirm|cancel}`.
    PluginUninstallRequest {
        payload: String,
    },
    EmailConfirmRequest {
        payload: String,
    },
    PushNotificationRequest,
    McpConsentRequest {
        data: String,
    },
    RefreshFile {
        path: String,
    },
    RefreshAppUI {
        app_id: String,
    },
    CaptureAppUI {
        app_id: String,
        request_id: String,
    },
    NavigationRequested {
        payload: String,
    },
    #[serde(alias = "CcThreadSpawned")]
    CodingAgentThreadSpawned {
        cc_thread_id: String,
        title: String,
        #[serde(default = "default_agent_kind_claude_code")]
        agent: AgentKind,
    },
    ChildrenCountChanged {
        active: i64,
        total: i64,
    },
}

impl ThreadEvent {
    /// Names of every event variant that closes a chat-mode request.
    /// Excludes CC-specific terminators (`CodingAgentIdled`, `SessionEnded`)
    /// — `chat::recovery::recover_orphaned_threads` checks both sets and
    /// keeps its own enumeration. Use this for code that operates on the
    /// chat agentic loop only.
    pub const TERMINATOR_EVENT_TYPES: &'static [&'static str] = &[
        "ResponseGenerated",
        "ResponseCanceled",
        "ResponseAborted",
        "ResponseFailed",
    ];

    /// Event names that orphan an unanswered `UserQuestionAsked` on a CC
    /// thread — once any of these lands after a question, the surrounding
    /// turn is gone and the user's next typed text must start a fresh
    /// follow-up rather than being routed as a `FreeText` answer to the
    /// dead question. Used by `agent_question::lookup_active_question_tool_use_id`.
    /// `ResponseGenerated` is omitted because `UserQuestionAsked` is CC-only;
    /// CC turns end with `CodingAgentIdled`, not `ResponseGenerated`.
    pub const QUESTION_ORPHANING_EVENT_TYPES: &'static [&'static str] = &[
        "ResponseAborted",
        "ResponseCanceled",
        "ResponseFailed",
        "CodingAgentIdled",
    ];

    /// Convert a control request into a CodingAgentSettingsChanged event,
    /// if applicable. `agent` identifies which backend issued the change.
    pub fn from_control_request(
        request: &crate::runtime::ControlRequest,
        agent: AgentKind,
    ) -> Option<Self> {
        use crate::runtime::ControlRequest;
        let (model, effort, perm) = match request {
            ControlRequest::SetModel { model } => (Some(model.clone()), None, None),
            ControlRequest::SetReasoningEffort { effort } => (None, Some(effort.clone()), None),
            ControlRequest::SetPermissionMode { mode } => (None, None, Some(mode.clone())),
            _ => return None,
        };
        Some(Self::CodingAgentSettingsChanged {
            model,
            reasoning_effort: effort,
            permission_mode: perm,
            agent,
        })
    }

    /// Returns the variant name as a string, matching the DB `event_type` column.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::MessageReceived { .. } => "MessageReceived",
            Self::TextStreamed { .. } => "TextStreamed",
            Self::Thinking { .. } => "Thinking",
            Self::ContextCaptured { .. } => "ContextCaptured",
            Self::MemorySearched { .. } => "MemorySearched",
            Self::ToolCalled { .. } => "ToolCalled",
            Self::ToolResult { .. } => "ToolResult",
            Self::BackgroundBashStarted { .. } => "BackgroundBashStarted",
            Self::BackgroundBashCompleted { .. } => "BackgroundBashCompleted",
            Self::ResponseGenerated { .. } => "ResponseGenerated",
            Self::ResponseCanceled { .. } => "ResponseCanceled",
            Self::ResponseAborted { .. } => "ResponseAborted",
            Self::ResponseFailed { .. } => "ResponseFailed",
            Self::ContinuationStarted { .. } => "ContinuationStarted",
            Self::SessionStarted { .. } => "SessionStarted",
            Self::SessionEnded { .. } => "SessionEnded",
            Self::CodingAgentTextStreamed { .. } => "CodingAgentTextStreamed",
            Self::CodingAgentToolCalled { .. } => "CodingAgentToolCalled",
            Self::CodingAgentToolResult { .. } => "CodingAgentToolResult",
            Self::CodingAgentUserMessageSent { .. } => "CodingAgentUserMessageSent",
            Self::CodingAgentPromptSent { .. } => "CodingAgentPromptSent",
            Self::MissingHardeningDetected { .. } => "MissingHardeningDetected",
            Self::CodingAgentIdled { .. } => "CodingAgentIdled",
            Self::ContinueSignal { .. } => "ContinueSignal",
            Self::ThreadTitleGenerated { .. } => "ThreadTitleGenerated",
            Self::ThreadTitleRenamed { .. } => "ThreadTitleRenamed",
            Self::ThreadSaved => "ThreadSaved",
            Self::ThreadUnsaved => "ThreadUnsaved",
            Self::ThreadArchived => "ThreadArchived",
            Self::ThreadStarted { .. } => "ThreadStarted",
            Self::ThreadDiscarded { .. } => "ThreadDiscarded",
            Self::ImageUploaded { .. } => "ImageUploaded",
            Self::TriggerStarted { .. } => "TriggerStarted",
            Self::TriggerCompleted { .. } => "TriggerCompleted",
            Self::ChangeProposed { .. } => "ChangeProposed",
            Self::ChangeApplied { .. } => "ChangeApplied",
            Self::ChangeDiscarded { .. } => "ChangeDiscarded",
            Self::ChangeReverted { .. } => "ChangeReverted",
            Self::ChangeApplyFailed { .. } => "ChangeApplyFailed",
            Self::MergeConflictDetected { .. } => "MergeConflictDetected",
            Self::MergeResolutionStarted { .. } => "MergeResolutionStarted",
            Self::MergeResolutionCleared { .. } => "MergeResolutionCleared",
            Self::ChangeHardened { .. } => "ChangeHardened",
            Self::CodingAgentSettingsChanged { .. } => "CodingAgentSettingsChanged",
            Self::UserPromptInjected { .. } => "UserPromptInjected",
            Self::CredentialRequested { .. } => "CredentialRequested",
            Self::McpConsentRequested { .. } => "McpConsentRequested",
            Self::UserQuestionAsked { .. } => "UserQuestionAsked",
            Self::UserQuestionAnswered { .. } => "UserQuestionAnswered",
            Self::CodingAgentPermissionRequest { .. } => "CodingAgentPermissionRequest",
            Self::CodingAgentPermissionResolved { .. } => "CodingAgentPermissionResolved",
            Self::WorktreeCleaned { .. } => "WorktreeCleaned",
            Self::ChildThreadCompleted { .. } => "ChildThreadCompleted",
            Self::ContextDismissed { .. } => "ContextDismissed",
            // Transient
            Self::TextStreaming { .. } => "TextStreaming",
            Self::Retrying { .. } => "Retrying",
            Self::PreambleCompleting => "PreambleCompleting",
            Self::CredentialRequest { .. } => "CredentialRequest",
            Self::PluginInstallRequest { .. } => "PluginInstallRequest",
            Self::PluginUninstallRequest { .. } => "PluginUninstallRequest",
            Self::EmailConfirmRequest { .. } => "EmailConfirmRequest",
            Self::PushNotificationRequest => "PushNotificationRequest",
            Self::McpConsentRequest { .. } => "McpConsentRequest",
            Self::RefreshFile { .. } => "RefreshFile",
            Self::RefreshAppUI { .. } => "RefreshAppUI",
            Self::CaptureAppUI { .. } => "CaptureAppUI",
            Self::NavigationRequested { .. } => "NavigationRequested",
            Self::CodingAgentThreadSpawned { .. } => "CodingAgentThreadSpawned",
            Self::ChildrenCountChanged { .. } => "ChildrenCountChanged",
        }
    }

    /// Whether this event should be persisted to the DB.
    /// Past-tense variants are persisted, present-participle variants are transient.
    pub fn is_persisted(&self) -> bool {
        !matches!(
            self,
            Self::TextStreaming { .. }
                | Self::Retrying { .. }
                | Self::PreambleCompleting
                | Self::CredentialRequest { .. }
                | Self::PluginInstallRequest { .. }
                | Self::PluginUninstallRequest { .. }
                | Self::EmailConfirmRequest { .. }
                | Self::PushNotificationRequest
                | Self::McpConsentRequest { .. }
                | Self::RefreshFile { .. }
                | Self::RefreshAppUI { .. }
                | Self::CaptureAppUI { .. }
                | Self::NavigationRequested { .. }
                | Self::CodingAgentThreadSpawned { .. }
                | Self::ChildrenCountChanged { .. }
        )
    }

    /// Returns the text content to index into memory, if this event type is indexable.
    /// Used by both the live memory consumer and the rebuild path.
    pub fn indexable_text(&self) -> Option<&str> {
        match self {
            Self::MessageReceived { text, .. } => Some(text),
            Self::UserPromptInjected { text, .. } => Some(text),
            Self::ResponseGenerated { text, .. } => Some(text),
            Self::ResponseCanceled { text, .. } => Some(text),
            Self::ResponseAborted { text, .. } => Some(text),
            Self::ChildThreadCompleted { summary, .. } => Some(summary),
            _ => None,
        }
    }

    /// Serializes to JSON payload for DB storage, stripping the "type" tag.
    pub fn to_payload(&self, meta: &EventMeta) -> Value {
        let mut v = serde_json::to_value(self).expect("ThreadEvent serialization cannot fail");
        if let Some(obj) = v.as_object_mut() {
            obj.remove("type");
            meta.apply(obj);
        }
        v
    }
}

/// Cross-cutting metadata merged into the event payload during persistence.
/// Not part of the ThreadEvent type itself — these are routing/grouping fields
/// used by read queries (message grouping, channel filtering).
#[derive(Clone, Debug, Default)]
pub struct EventMeta {
    /// Links response events back to the request that triggered them.
    pub request_event_id: Option<uuid::Uuid>,
    /// Source channel. Always set on origin events.
    pub channel: Option<EventChannel>,
    /// Client-provided UUID to use as the DB event primary key.
    /// If set, EventBus uses this instead of generating a new UUID.
    /// Allows frontend to match SSE events back to pending messages.
    pub event_id: Option<uuid::Uuid>,
    /// Audit: who initiated. Mutating handlers stamp via `api/actor::user_actor`;
    /// internal state machines leave None.
    pub actor: Option<MessageOrigin>,
}

impl EventMeta {
    pub const NONE: EventMeta = EventMeta {
        request_event_id: None,
        channel: None,
        event_id: None,
        actor: None,
    };

    pub fn with_actor(actor: Option<MessageOrigin>) -> Self {
        EventMeta {
            actor,
            ..EventMeta::NONE
        }
    }

    /// Merge typed metadata fields into a JSON payload object.
    pub fn apply(&self, obj: &mut serde_json::Map<String, Value>) {
        if let Some(id) = &self.request_event_id {
            obj.insert("request_event_id".into(), Value::String(id.to_string()));
        }
        if let Some(ch) = &self.channel {
            obj.insert(
                "channel".into(),
                serde_json::to_value(ch).expect("EventChannel serialization"),
            );
        }
        if let Some(actor) = &self.actor {
            obj.insert(
                "actor".into(),
                serde_json::to_value(actor).expect("MessageOrigin serialization"),
            );
        }
    }
}

#[cfg(test)]
#[path = "thread_events_tests.rs"]
mod tests;
