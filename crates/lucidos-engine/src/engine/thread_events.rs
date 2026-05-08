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
    /// CC session resumed after engine restart (orphan recovery).
    SessionRecovered,
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
    Selected { option_id: String },
    FreeText { text: String },
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_tokens: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_messages: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trimmed: Option<bool>,
    },
    /// Real prompt-token count from the LLM provider's `usage`. Emitted after
    /// the response arrives — overrides the chars/4 estimate carried by the
    /// preceding `Thinking` event so the UI shows the true cost.
    ContextTokensMeasured {
        input_tokens: u32,
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
    },
    ResponseFailed {
        error: String,
    },

    // Claude Code
    #[serde(alias = "SessionResumed")]
    SessionRecovered {
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
    CodingAgentPermissionResolved {
        request_id: String,
        allowed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
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
            Self::ContextTokensMeasured { .. } => "ContextTokensMeasured",
            Self::MemorySearched { .. } => "MemorySearched",
            Self::ToolCalled { .. } => "ToolCalled",
            Self::ToolResult { .. } => "ToolResult",
            Self::ResponseGenerated { .. } => "ResponseGenerated",
            Self::ResponseCanceled { .. } => "ResponseCanceled",
            Self::ResponseAborted { .. } => "ResponseAborted",
            Self::ResponseFailed { .. } => "ResponseFailed",
            Self::SessionRecovered { .. } => "SessionRecovered",
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
            // Transient
            Self::TextStreaming { .. } => "TextStreaming",
            Self::Retrying { .. } => "Retrying",
            Self::PreambleCompleting => "PreambleCompleting",
            Self::CredentialRequest { .. } => "CredentialRequest",
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
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn message_origin_engine_serializes_with_kind_engine() {
        let origin = MessageOrigin::Engine {
            reason: EngineReason::SessionRecovered,
        };
        let json = serde_json::to_value(&origin).unwrap();
        assert_eq!(json["kind"], "engine");
        assert_eq!(json["reason"]["kind"], "session_recovered");
    }

    /// `MessageOrigin::System` serializes as `{"kind":"system"}` with NO
    /// other fields. The frontend's MessageOrigin union has `{ kind: 'system' }`
    /// (no reason / no metadata) — adding fields here would break that contract.
    /// Distinct from Engine: System means the host killed the process; Engine
    /// means the engine deliberately took an action.
    #[test]
    fn message_origin_system_serializes_with_kind_system_no_other_fields() {
        let origin = MessageOrigin::System;
        let json = serde_json::to_value(&origin).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "system"}));
    }

    /// System is intrinsically engine-mode (deterministic, non-human, non-agent).
    /// Mirrors `MessageOrigin::Engine`'s mode — the chip differentiates via
    /// label override (System vs Lucidos Engine), not via mode.
    #[test]
    fn message_origin_system_mode_is_engine() {
        assert_eq!(MessageOrigin::System.mode(), ActorMode::Engine);
    }

    /// `MessageOrigin::system()` is the canonical constructor — emit sites use
    /// it for the "host killed the process" attribution (orphan recovery,
    /// shutdown, safety net, post-restart abort marker).
    #[test]
    fn message_origin_system_constructor() {
        assert!(matches!(MessageOrigin::system(), MessageOrigin::System));
    }

    #[test]
    fn message_origin_engine_scheduler_carries_trigger_metadata() {
        let trigger_id = uuid::Uuid::new_v4().to_string();
        let origin = MessageOrigin::Engine {
            reason: EngineReason::Scheduler {
                trigger_id: trigger_id.clone(),
                trigger_name: Some("nightly-backup".to_string()),
            },
        };
        let json = serde_json::to_value(&origin).unwrap();
        assert_eq!(json["reason"]["kind"], "scheduler");
        assert_eq!(json["reason"]["trigger_id"], trigger_id);
        assert_eq!(json["reason"]["trigger_name"], "nightly-backup");
    }

    #[test]
    fn message_origin_engine_round_trips_through_serde() {
        let original = MessageOrigin::Engine {
            reason: EngineReason::HardenRetrigger,
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: MessageOrigin = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn actor_mode_serializes_lowercase_strings() {
        assert_eq!(serde_json::to_string(&ActorMode::Human).unwrap(), "\"human\"");
        assert_eq!(serde_json::to_string(&ActorMode::Agent).unwrap(), "\"agent\"");
        assert_eq!(serde_json::to_string(&ActorMode::Engine).unwrap(), "\"engine\"");
    }

    #[test]
    fn actor_mode_deserializes_lowercase_strings() {
        assert_eq!(serde_json::from_str::<ActorMode>("\"human\"").unwrap(), ActorMode::Human);
        assert_eq!(serde_json::from_str::<ActorMode>("\"agent\"").unwrap(), ActorMode::Agent);
        assert_eq!(serde_json::from_str::<ActorMode>("\"engine\"").unwrap(), ActorMode::Engine);
    }

    #[test]
    fn message_origin_thread_link_defaults_mode_to_agent_when_missing() {
        let json = r#"{
            "kind": "thread_link",
            "thread_id": "00000000-0000-0000-0000-000000000001"
        }"#;
        let parsed: MessageOrigin = serde_json::from_str(json).unwrap();
        match parsed {
            MessageOrigin::ThreadLink {
                mode, direction, ..
            } => {
                assert_eq!(mode, ActorMode::Agent);
                assert_eq!(direction, ThreadDirection::Parent);
            }
            other => panic!("expected ThreadLink, got {:?}", other),
        }
    }

    /// Historical DB rows persisted under the old variant name. The
    /// `serde(alias = "parent_thread")` + default `direction` keep them
    /// readable as `ThreadLink { direction: Parent }`.
    #[test]
    fn message_origin_legacy_parent_thread_kind_deserializes_as_thread_link() {
        let json = r#"{
            "kind": "parent_thread",
            "thread_id": "00000000-0000-0000-0000-000000000001",
            "mode": "engine"
        }"#;
        let parsed: MessageOrigin = serde_json::from_str(json).unwrap();
        match parsed {
            MessageOrigin::ThreadLink {
                mode, direction, ..
            } => {
                assert_eq!(mode, ActorMode::Engine);
                assert_eq!(direction, ThreadDirection::Parent);
            }
            other => panic!("expected ThreadLink (from parent_thread alias), got {:?}", other),
        }
    }

    #[test]
    fn message_origin_workspace_defaults_mode_to_human_when_missing() {
        let json = r#"{ "kind": "workspace", "workspace": "personal" }"#;
        let parsed: MessageOrigin = serde_json::from_str(json).unwrap();
        match parsed {
            MessageOrigin::Workspace { mode, .. } => assert_eq!(mode, ActorMode::Human),
            other => panic!("expected Workspace, got {:?}", other),
        }
    }

    #[test]
    fn message_origin_thread_link_round_trips_with_explicit_engine_mode() {
        let original = MessageOrigin::ThreadLink {
            thread_id: uuid::Uuid::new_v4(),
            title: Some("recovered".into()),
            spawning_event_id: None,
            mode: ActorMode::Engine,
            direction: ThreadDirection::Parent,
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: MessageOrigin = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn message_origin_thread_link_child_round_trips() {
        let original = MessageOrigin::ThreadLink {
            thread_id: uuid::Uuid::new_v4(),
            title: Some("child task".into()),
            spawning_event_id: Some(uuid::Uuid::new_v4()),
            mode: ActorMode::Agent,
            direction: ThreadDirection::Child,
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: MessageOrigin = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn message_origin_mode_derives_human_for_device_and_api() {
        let device = MessageOrigin::Device { device_id: "d".into(), label: "l".into() };
        let api = MessageOrigin::Api { user_agent: None, mode: ActorMode::Human };
        assert_eq!(device.mode(), ActorMode::Human);
        assert_eq!(api.mode(), ActorMode::Human);
    }

    #[test]
    fn message_origin_mode_derives_engine_for_engine_variant() {
        let origin = MessageOrigin::Engine { reason: EngineReason::SessionRecovered };
        assert_eq!(origin.mode(), ActorMode::Engine);
    }

    #[test]
    fn message_origin_mode_reads_field_for_workspace_and_thread_link() {
        let ws = MessageOrigin::Workspace {
            workspace: "x".into(), thread_id: None, event_id: None, user_agent: None,
            mode: ActorMode::Agent,
        };
        let tl = MessageOrigin::ThreadLink {
            thread_id: uuid::Uuid::new_v4(), title: None, spawning_event_id: None,
            mode: ActorMode::Engine, direction: ThreadDirection::Parent,
        };
        assert_eq!(ws.mode(), ActorMode::Agent);
        assert_eq!(tl.mode(), ActorMode::Engine);
    }

    #[test]
    fn thread_event_serializes_with_type_tag() {
        let event = ThreadEvent::ToolCalled {
            name: "read_file".to_string(),
            args: json!({"path": "test.txt"}),
            description: "Reading test.txt...".to_string(),
        };
        let serialized = serde_json::to_value(&event).unwrap();
        assert_eq!(serialized["type"], "ToolCalled");
        assert_eq!(serialized["name"], "read_file");
        assert_eq!(serialized["args"]["path"], "test.txt");
        assert_eq!(serialized["description"], "Reading test.txt...");
    }

    #[test]
    fn thread_event_type_name_extraction() {
        let cases: Vec<(ThreadEvent, &str)> = vec![
            (
                ThreadEvent::MessageReceived {
                    text: "hi".into(),
                    user_image_hashes: vec![],
                    device_id: None,
                    device: None,
                    image_description: None,
                    parent_thread_id: None,
                    spawning_event_id: None,
                    mode: ActorMode::Human,
                    model: None,
                    reasoning_effort: None,
                    origin: None,
                },
                "MessageReceived",
            ),
            (
                ThreadEvent::TextStreamed { text: "t".into() },
                "TextStreamed",
            ),
            (
                ThreadEvent::Thinking {
                    text: "hmm".into(),
                    context_tokens: None,
                    context_messages: None,
                    trimmed: None,
                },
                "Thinking",
            ),
            (
                ThreadEvent::MemorySearched {
                    results: 5,
                    queries: vec!["birthday".into()],
                },
                "MemorySearched",
            ),
            (
                ThreadEvent::ToolCalled {
                    name: "x".into(),
                    args: json!({}),
                    description: String::new(),
                },
                "ToolCalled",
            ),
            (
                ThreadEvent::ToolResult {
                    name: "x".into(),
                    result: "ok".into(),
                    images: vec![],
                },
                "ToolResult",
            ),
            (
                ThreadEvent::ResponseGenerated {
                    text: String::new(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                },
                "ResponseGenerated",
            ),
            (
                ThreadEvent::ResponseCanceled {
                    text: String::new(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                },
                "ResponseCanceled",
            ),
            (
                ThreadEvent::ResponseAborted {
                    text: String::new(),
                    images: vec![],
                    model: None,
                    reasoning_effort: None,
                },
                "ResponseAborted",
            ),
            (
                ThreadEvent::ResponseFailed { error: "e".into() },
                "ResponseFailed",
            ),
            (
                ThreadEvent::SessionRecovered {
                    branch: String::new(),
                    origin: None,
                },
                "SessionRecovered",
            ),
            (
                ThreadEvent::SessionStarted {
                    session_id: "s".into(),
                    branch: String::new(),
                    repo_id: None,
                },
                "SessionStarted",
            ),
            (
                ThreadEvent::SessionEnded {
                    reason: SessionEndReason::Shutdown,
                },
                "SessionEnded",
            ),
            (
                ThreadEvent::CodingAgentTextStreamed {
                    text: "t".into(),
                    agent: crate::runtime::AgentKind::ClaudeCode,
                },
                "CodingAgentTextStreamed",
            ),
            (
                ThreadEvent::CodingAgentToolCalled {
                    name: "n".into(),
                    args: json!({}),
                    description: String::new(),
                    agent: crate::runtime::AgentKind::ClaudeCode,
                    tool_use_id: String::new(),
                },
                "CodingAgentToolCalled",
            ),
            (
                ThreadEvent::CodingAgentToolResult {
                    name: "n".into(),
                    result: "r".into(),
                    agent: crate::runtime::AgentKind::ClaudeCode,
                    tool_use_id: String::new(),
                },
                "CodingAgentToolResult",
            ),
            (
                ThreadEvent::CodingAgentUserMessageSent {
                    text: "t".into(),
                    agent: crate::runtime::AgentKind::ClaudeCode,
                },
                "CodingAgentUserMessageSent",
            ),
            (
                ThreadEvent::MissingHardeningDetected { origin: None },
                "MissingHardeningDetected",
            ),
            (
                ThreadEvent::CodingAgentIdled {
                    has_changes: false,
                    is_external_repo: false,
                    requires_restart: false,
                    cc_session_id: None,
                    agent: crate::runtime::AgentKind::ClaudeCode,
                    reason: None,
                    worktree_path: None,
                    worktree_head_sha: None,
                },
                "CodingAgentIdled",
            ),
            (
                ThreadEvent::ThreadTitleGenerated { title: "t".into() },
                "ThreadTitleGenerated",
            ),
            (
                ThreadEvent::ThreadTitleRenamed {
                    title: "new".into(),
                },
                "ThreadTitleRenamed",
            ),
            (ThreadEvent::ThreadSaved, "ThreadSaved"),
            (ThreadEvent::ThreadUnsaved, "ThreadUnsaved"),
            (ThreadEvent::ThreadArchived, "ThreadArchived"),
            (
                ThreadEvent::TriggerStarted {
                    trigger_id: "id".into(),
                    trigger_name: None,
                    prompt: None,
                    invocation: None,
                    origin: None,
                    go_to_review: false,
                },
                "TriggerStarted",
            ),
            (
                ThreadEvent::TriggerCompleted {
                    trigger_id: "id".into(),
                    trigger_name: None,
                    result_summary: None,
                },
                "TriggerCompleted",
            ),
            (
                ThreadEvent::ChangeProposed {
                    change_id: "c".into(),
                    description: None,
                    files: vec![],
                    requires_restart: false,
                    origin: None,
                    commit_sha: None,
                    branch_name: String::new(),
                    repo_root: String::new(),
                    hardened: false,
                    incomplete: false,
                    path: String::new(),
                    diff: String::new(),
                },
                "ChangeProposed",
            ),
            (
                ThreadEvent::ChangeApplied {
                    change_id: "c".into(),
                    requires_restart: false,
                    client_update: false,
                    commits: vec![],
                    thread_title: None,
                    actor: None,
                    pre_merge_sha: None,
                    post_merge_sha: None,
                    path: String::new(),
                },
                "ChangeApplied",
            ),
            (
                ThreadEvent::ChangeDiscarded {
                    change_id: "c".into(),
                    actor: None,
                    path: String::new(),
                },
                "ChangeDiscarded",
            ),
            (
                ThreadEvent::ChangeReverted {
                    change_id: "c".into(),
                    actor: None,
                    path: String::new(),
                },
                "ChangeReverted",
            ),
            (
                ThreadEvent::ChangeApplyFailed {
                    change_id: "c".into(),
                    error: "conflict".into(),
                    actor: None,
                },
                "ChangeApplyFailed",
            ),
            (
                ThreadEvent::MergeConflictDetected {
                    change_id: "c".into(),
                    files: vec!["file.rs".into()],
                    origin: None,
                },
                "MergeConflictDetected",
            ),
            (
                ThreadEvent::MergeResolutionStarted {
                    change_id: "c".into(),
                    worktree_path: "/tmp/wt".into(),
                    temp_branch: "merge-tmp/c".into(),
                },
                "MergeResolutionStarted",
            ),
            (
                ThreadEvent::MergeResolutionCleared {
                    change_id: "c".into(),
                },
                "MergeResolutionCleared",
            ),
            (
                ThreadEvent::ChangeHardened {
                    change_id: "c".into(),
                    actor: None,
                },
                "ChangeHardened",
            ),
            (
                ThreadEvent::CredentialRequested {
                    provider: "github".into(),
                },
                "CredentialRequested",
            ),
            (
                ThreadEvent::McpConsentRequested {
                    tool: "t".into(),
                    args: json!({}),
                },
                "McpConsentRequested",
            ),
        ];
        for (event, expected) in cases {
            assert_eq!(
                event.event_type(),
                expected,
                "event_type() mismatch for {:?}",
                event
            );
        }
    }

    #[test]
    fn transient_event_serializes() {
        let event = ThreadEvent::Retrying {
            reason: "rate limited".to_string(),
        };
        let serialized = serde_json::to_value(&event).unwrap();
        assert_eq!(serialized["type"], "Retrying");
        assert_eq!(serialized["reason"], "rate limited");

        let event2 = ThreadEvent::PreambleCompleting;
        let serialized2 = serde_json::to_value(&event2).unwrap();
        assert_eq!(serialized2["type"], "PreambleCompleting");
    }

    #[test]
    fn all_db_event_types_have_variants() {
        // Every known DB event_type string must round-trip through serde deserialization.
        // Old format (unit variants) and new format (struct variants) must both work.
        let known_types = vec![
            // Chat
            r#"{"type":"MessageReceived","text":"hi"}"#,
            r#"{"type":"TextStreamed","text":"t"}"#,
            r#"{"type":"Thinking","text":"hmm"}"#,
            r#"{"type":"Thinking","text":"ctx","context_tokens":1000,"context_messages":5,"trimmed":true}"#,
            r#"{"type":"MemorySearched","results":3}"#,
            r#"{"type":"MemorySearched","results":5,"queries":["birthday","date of birth"]}"#,
            r#"{"type":"ToolCalled","name":"x","args":{}}"#,
            r#"{"type":"ToolResult","name":"x","result":"ok"}"#,
            // Old format (unit) — must still deserialize
            r#"{"type":"ResponseGenerated"}"#,
            r#"{"type":"ResponseCanceled"}"#,
            // New format (struct)
            r#"{"type":"ResponseGenerated","text":"answer","images":["img.png"]}"#,
            r#"{"type":"ResponseCanceled","text":"partial","images":[]}"#,
            r#"{"type":"ResponseAborted"}"#,
            r#"{"type":"ResponseAborted","text":"partial","images":[]}"#,
            r#"{"type":"ResponseFailed","error":"e"}"#,
            // Claude Code
            r#"{"type":"SessionRecovered","branch":"claude-code/20260318"}"#,
            r#"{"type":"SessionRecovered"}"#,
            // Legacy: old events stored as SessionResumed must still deserialize
            r#"{"type":"SessionResumed","branch":"claude-code/20260318"}"#,
            r#"{"type":"SessionResumed"}"#,
            r#"{"type":"SessionStarted","session_id":"s"}"#,
            r#"{"type":"SessionStarted","session_id":"s","branch":"claude-code/20260318"}"#,
            // New format with repo_id for external repo binding
            r#"{"type":"SessionStarted","session_id":"s","branch":"claude-code/20260318","repo_id":"550e8400-e29b-41d4-a716-446655440000"}"#,
            r#"{"type":"SessionEnded"}"#,
            // New format with reason
            r#"{"type":"SessionEnded","reason":"user_ended"}"#,
            r#"{"type":"SessionEnded","reason":"changes_proposed"}"#,
            r#"{"type":"SessionEnded","reason":"changes_applied"}"#,
            r#"{"type":"SessionEnded","reason":"auto_ended"}"#,
            r#"{"type":"CodingAgentTextStreamed","text":"t"}"#,
            r#"{"type":"CodingAgentToolCalled","name":"n","args":{}}"#,
            r#"{"type":"CodingAgentToolResult","name":"n","result":"r"}"#,
            r#"{"type":"CodingAgentUserMessageSent","text":"t"}"#,
            r#"{"type":"MissingHardeningDetected"}"#,
            r#"{"type":"CodingAgentIdled"}"#,
            // New format with has_changes
            r#"{"type":"CodingAgentIdled","has_changes":true}"#,
            // Thread lifecycle
            r#"{"type":"ThreadTitleGenerated","title":"t"}"#,
            r#"{"type":"ThreadTitleRenamed","title":"new title"}"#,
            r#"{"type":"ThreadSaved"}"#,
            r#"{"type":"ThreadUnsaved"}"#,
            r#"{"type":"ThreadArchived"}"#,
            // EventMeta.actor merged into payload — must round-trip on unit and
            // struct variants alike. Internally-tagged enums tolerate extra
            // fields by default, but make it a regression test so a future
            // `#[serde(deny_unknown_fields)]` flip would fail loudly here.
            r#"{"type":"ThreadSaved","actor":{"kind":"device","device_id":"d","label":"Chrome"}}"#,
            r#"{"type":"ThreadUnsaved","actor":{"kind":"api","user_agent":"curl/8"}}"#,
            r#"{"type":"ThreadArchived","actor":{"kind":"workspace","workspace":"dev"}}"#,
            r#"{"type":"ThreadTitleRenamed","title":"x","actor":{"kind":"device","device_id":"d","label":"l"}}"#,
            // Triggers — minimal + full + legacy task_id alias on the renamed variant
            r#"{"type":"TriggerStarted","trigger_id":"id"}"#,
            r#"{"type":"TriggerStarted","trigger_id":"id","trigger_name":"daily","prompt":"run","invocation":{"kind":"Schedule"}}"#,
            r#"{"type":"TriggerStarted","trigger_id":"id","trigger_name":"sleep-import","invocation":{"kind":"Event","event_type":"DataImported","event_id":"00000000-0000-0000-0000-000000000001"}}"#,
            r#"{"type":"TriggerStarted","task_id":"id","task_name":"legacy"}"#,
            r#"{"type":"TriggerCompleted","trigger_id":"id"}"#,
            r#"{"type":"TriggerCompleted","trigger_id":"id","trigger_name":"daily","result_summary":"done"}"#,
            r#"{"type":"TriggerCompleted","task_id":"id","task_name":"legacy"}"#,
            // Changes — old format (path/diff)
            r#"{"type":"ChangeProposed","path":"p","diff":"d"}"#,
            r#"{"type":"ChangeApplied","path":"p"}"#,
            r#"{"type":"ChangeDiscarded","path":"p"}"#,
            r#"{"type":"ChangeReverted","path":"p"}"#,
            // Changes — new format (change_id)
            r#"{"type":"ChangeProposed","change_id":"c-1","description":"fix","files":["a.rs"],"requires_restart":true}"#,
            r#"{"type":"ChangeApplied","change_id":"c-1","requires_restart":false}"#,
            r#"{"type":"ChangeDiscarded","change_id":"c-1"}"#,
            r#"{"type":"ChangeReverted","change_id":"c-1"}"#,
            r#"{"type":"ChangeApplyFailed","change_id":"c-1","error":"merge conflict"}"#,
            // Interactive
            r#"{"type":"CredentialRequested","provider":"github"}"#,
            r#"{"type":"McpConsentRequested","tool":"t","args":{}}"#,
        ];
        for json_str in known_types {
            let result: Result<ThreadEvent, _> = serde_json::from_str(json_str);
            assert!(
                result.is_ok(),
                "Failed to deserialize: {}\nError: {:?}",
                json_str,
                result.err()
            );
        }
    }

    #[test]
    fn to_payload_removes_type_tag() {
        let event = ThreadEvent::ToolCalled {
            name: "read_file".to_string(),
            args: json!({"path": "test.txt"}),
            description: "Reading test.txt...".to_string(),
        };
        let payload = event.to_payload(&EventMeta::NONE);
        assert!(
            payload.get("type").is_none(),
            "to_payload() must strip the 'type' tag"
        );
        assert_eq!(payload["name"], "read_file");
        assert_eq!(payload["args"]["path"], "test.txt");

        // ResponseGenerated with empty text — should produce empty object (skip_serializing_if)
        let event2 = ThreadEvent::ResponseGenerated {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        };
        let payload2 = event2.to_payload(&EventMeta::NONE);
        assert!(payload2.get("type").is_none());
        assert!(
            payload2.as_object().unwrap().is_empty(),
            "empty ResponseGenerated should produce {{}}"
        );

        // ResponseGenerated with content
        let event3 = ThreadEvent::ResponseGenerated {
            text: "answer".into(),
            images: vec!["img.png".into()],
            model: None,
            reasoning_effort: None,
        };
        let payload3 = event3.to_payload(&EventMeta::NONE);
        assert_eq!(payload3["text"], "answer");
        assert_eq!(payload3["images"][0], "img.png");
    }

    #[test]
    fn claude_code_idled_has_changes_serialization() {
        // With has_changes=true → field included
        let event = ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        };
        let serialized = serde_json::to_value(&event).unwrap();
        assert_eq!(serialized["type"], "CodingAgentIdled");
        assert_eq!(serialized["has_changes"], true);

        // With has_changes=false → field skipped (skip_serializing_if = "is_false")
        let event2 = ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            agent: crate::runtime::AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        };
        let serialized2 = serde_json::to_value(&event2).unwrap();
        assert_eq!(serialized2["type"], "CodingAgentIdled");
        assert!(
            serialized2.get("has_changes").is_none(),
            "false has_changes should be skipped"
        );

        // Old DB format without has_changes deserializes with default=false
        let old_format: ThreadEvent =
            serde_json::from_str(r#"{"type":"CodingAgentIdled"}"#).unwrap();
        match old_format {
            ThreadEvent::CodingAgentIdled { has_changes, .. } => assert!(!has_changes),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn tool_called_description_serialization() {
        // With description → included in JSON
        let event = ThreadEvent::ToolCalled {
            name: "read_file".into(),
            args: json!({"path": "test.txt"}),
            description: "Reading test.txt...".into(),
        };
        let serialized = serde_json::to_value(&event).unwrap();
        assert_eq!(serialized["description"], "Reading test.txt...");

        // Empty description → skipped (skip_serializing_if = "is_empty_str")
        let event2 = ThreadEvent::ToolCalled {
            name: "read_file".into(),
            args: json!({"path": "test.txt"}),
            description: String::new(),
        };
        let serialized2 = serde_json::to_value(&event2).unwrap();
        assert!(
            serialized2.get("description").is_none(),
            "empty description should be skipped"
        );
    }

    #[test]
    fn tool_called_backward_compat_no_description() {
        // Old DB rows without description field must still deserialize
        let old_format: ThreadEvent = serde_json::from_str(
            r#"{"type":"ToolCalled","name":"read_file","args":{"path":"test.txt"}}"#,
        )
        .unwrap();
        match old_format {
            ThreadEvent::ToolCalled {
                name, description, ..
            } => {
                assert_eq!(name, "read_file");
                assert!(
                    description.is_empty(),
                    "missing description should default to empty string"
                );
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn cc_tool_called_description_serialization() {
        let event = ThreadEvent::CodingAgentToolCalled {
            name: "Read".into(),
            args: json!({"file_path": "/src/main.rs"}),
            description: "Read main.rs".into(),
            agent: crate::runtime::AgentKind::ClaudeCode,
            tool_use_id: String::new(),
        };
        let serialized = serde_json::to_value(&event).unwrap();
        assert_eq!(serialized["description"], "Read main.rs");

        // Empty → skipped
        let event2 = ThreadEvent::CodingAgentToolCalled {
            name: "Read".into(),
            args: json!({}),
            description: String::new(),
            agent: crate::runtime::AgentKind::ClaudeCode,
            tool_use_id: String::new(),
        };
        let serialized2 = serde_json::to_value(&event2).unwrap();
        assert!(serialized2.get("description").is_none());
    }

    #[test]
    fn cc_tool_called_result_tool_use_id_round_trip() {
        let call = ThreadEvent::CodingAgentToolCalled {
            name: "Bash".into(),
            args: json!({"command": "ls"}),
            description: "ls".into(),
            agent: crate::runtime::AgentKind::ClaudeCode,
            tool_use_id: "toolu_42".into(),
        };
        let serialized = serde_json::to_value(&call).unwrap();
        assert_eq!(serialized["tool_use_id"], "toolu_42");

        // Empty id → skipped from the wire
        let call_no_id = ThreadEvent::CodingAgentToolCalled {
            name: "Bash".into(),
            args: json!({}),
            description: String::new(),
            agent: crate::runtime::AgentKind::ClaudeCode,
            tool_use_id: String::new(),
        };
        assert!(serde_json::to_value(&call_no_id).unwrap().get("tool_use_id").is_none());

        // Legacy DB row without tool_use_id deserializes cleanly
        let legacy: ThreadEvent = serde_json::from_str(
            r#"{"type":"CodingAgentToolResult","name":"","result":"ok"}"#,
        )
        .unwrap();
        match legacy {
            ThreadEvent::CodingAgentToolResult { tool_use_id, .. } => {
                assert!(tool_use_id.is_empty());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn cc_tool_called_backward_compat_no_description() {
        let old_format: ThreadEvent =
            serde_json::from_str(r#"{"type":"CodingAgentToolCalled","name":"Read","args":{}}"#)
                .unwrap();
        match old_format {
            ThreadEvent::CodingAgentToolCalled {
                name, description, ..
            } => {
                assert_eq!(name, "Read");
                assert!(description.is_empty());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn message_received_with_image_hashes() {
        let event = ThreadEvent::MessageReceived {
            text: "look at this".into(),
            user_image_hashes: vec!["abcd1234".into(), "ef567890".into()],
            device_id: Some("phone-1".into()),
            device: Some("Test iPhone".into()),
            image_description: Some("a cat".into()),
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        };
        let payload = event.to_payload(&EventMeta::NONE);
        assert_eq!(payload["text"], "look at this");
        // Hashes are stored as a flat string array — no inline base64 anywhere.
        assert_eq!(payload["user_image_hashes"][0], "abcd1234");
        assert_eq!(payload["user_image_hashes"][1], "ef567890");
        assert!(
            payload.get("images").is_none(),
            "legacy `images` field must not appear in the new shape"
        );
        assert_eq!(payload["device_id"], "phone-1");
        assert_eq!(payload["image_description"], "a cat");
    }

    #[test]
    fn message_received_without_optional_fields() {
        let event = ThreadEvent::MessageReceived {
            text: "hello".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        };
        let payload = event.to_payload(&EventMeta::NONE);
        assert_eq!(payload["text"], "hello");
        // Optional fields should be absent
        assert!(
            payload.get("user_image_hashes").is_none(),
            "empty user_image_hashes should be skipped"
        );
        assert!(
            payload.get("device_id").is_none(),
            "None device_id should be skipped"
        );
    }

    #[test]
    fn trigger_started_with_details() {
        let event = ThreadEvent::TriggerStarted {
            trigger_id: "t-1".into(),
            trigger_name: Some("daily-report".into()),
            prompt: Some("Run the daily report".into()),
            invocation: Some(TriggerInvocation::Schedule),
            origin: None,
            go_to_review: false,
        };
        let payload = event.to_payload(&EventMeta::NONE);
        assert_eq!(payload["trigger_id"], "t-1");
        assert_eq!(payload["trigger_name"], "daily-report");
        assert_eq!(payload["prompt"], "Run the daily report");
        assert_eq!(payload["invocation"]["kind"], "Schedule");
    }

    #[test]
    fn trigger_started_event_invocation_serializes_event_type_and_id() {
        let event_id = uuid::Uuid::new_v4();
        let event = ThreadEvent::TriggerStarted {
            trigger_id: "t-2".into(),
            trigger_name: Some("sleep-import".into()),
            prompt: Some("Import overnight sleep data".into()),
            invocation: Some(TriggerInvocation::Event {
                event_type: "DataImported".into(),
                event_id: Some(event_id),
            }),
            origin: None,
            go_to_review: false,
        };
        let payload = event.to_payload(&EventMeta::NONE);
        assert_eq!(payload["invocation"]["kind"], "Event");
        assert_eq!(payload["invocation"]["event_type"], "DataImported");
        assert_eq!(payload["invocation"]["event_id"], event_id.to_string());
    }

    #[test]
    fn trigger_started_legacy_task_id_alias_deserializes() {
        // Old DB rows persisted before the rename used `task_id`/`task_name`.
        // The migration renames event_type values, but field names live in the
        // jsonb payload and must continue to deserialize via serde aliases so
        // historical rows replay cleanly.
        let json = r#"{"type":"TriggerStarted","task_id":"old","task_name":"legacy"}"#;
        let event: ThreadEvent = serde_json::from_str(json).unwrap();
        match event {
            ThreadEvent::TriggerStarted {
                trigger_id,
                trigger_name,
                ..
            } => {
                assert_eq!(trigger_id, "old");
                assert_eq!(trigger_name.as_deref(), Some("legacy"));
            }
            _ => panic!("expected TriggerStarted"),
        }
    }

    #[test]
    fn change_proposed_new_format() {
        let event = ThreadEvent::ChangeProposed {
            change_id: "c-1".into(),
            description: Some("Fix the bug".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: true,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        };
        let payload = event.to_payload(&EventMeta::NONE);
        assert_eq!(payload["change_id"], "c-1");
        assert_eq!(payload["description"], "Fix the bug");
        assert_eq!(payload["requires_restart"], true);
        // Legacy fields should be absent (empty → skipped); `incomplete: false`
        // is the default and must also be skipped so legacy DB rows decode
        // without a wire-shape diff.
        assert!(payload.get("path").is_none());
        assert!(payload.get("diff").is_none());
        assert!(
            payload.get("incomplete").is_none(),
            "incomplete=false (the common case) must skip serialization to keep \
             new event payloads byte-compatible with pre-field DB rows"
        );
    }

    #[test]
    fn event_meta_defaults() {
        let meta = EventMeta::default();
        assert!(meta.request_event_id.is_none());
        assert!(meta.channel.is_none());
        assert!(meta.event_id.is_none());
    }

    #[test]
    fn event_meta_merges_into_payload() {
        let event = ThreadEvent::ResponseGenerated {
            text: "answer".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        };
        let meta = EventMeta {
            request_event_id: Some(
                uuid::Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap(),
            ),
            channel: Some(EventChannel::CodingAgent),
            event_id: Some(uuid::Uuid::parse_str("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap()),
            actor: None,
        };
        let payload = event.to_payload(&meta);
        assert_eq!(payload["text"], "answer");
        assert_eq!(
            payload["request_event_id"],
            "12345678-1234-1234-1234-123456789abc"
        );
        assert_eq!(payload["channel"], "claude_code");
        // event_id is NOT merged into payload — it's used as the DB primary key
        assert!(payload.get("event_id").is_none());
    }

    #[test]
    fn event_meta_none_adds_nothing() {
        let event = ThreadEvent::TextStreamed {
            text: "chunk".into(),
        };
        let payload = event.to_payload(&EventMeta::NONE);
        assert_eq!(payload["text"], "chunk");
        assert!(payload.get("request_event_id").is_none());
        assert!(payload.get("channel").is_none());
        assert!(payload.get("event_id").is_none());
    }

    #[test]
    fn event_meta_actor_merges_into_payload() {
        // Auditability: every mutating endpoint stamps the event with who
        // initiated it. EventMeta carries that across all ThreadEvent variants
        // without per-variant struct churn or backward-compat churn for unit
        // variants like ThreadSaved.
        let event = ThreadEvent::ThreadSaved;
        let meta = EventMeta {
            actor: Some(MessageOrigin::Device {
                device_id: "dev-1".into(),
                label: "Chrome on Mac".into(),
            }),
            ..EventMeta::NONE
        };
        let payload = event.to_payload(&meta);
        assert_eq!(payload["actor"]["kind"], "device");
        assert_eq!(payload["actor"]["device_id"], "dev-1");
        assert_eq!(payload["actor"]["label"], "Chrome on Mac");
    }

    #[test]
    fn event_meta_actor_none_omits_field() {
        let event = ThreadEvent::ThreadSaved;
        let payload = event.to_payload(&EventMeta::NONE);
        assert!(payload.get("actor").is_none());
    }

    #[test]
    fn indexable_text_returns_content_for_chat_events() {
        let msg = ThreadEvent::MessageReceived {
            text: "hello".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        };
        assert_eq!(msg.indexable_text(), Some("hello"));

        let resp = ThreadEvent::ResponseGenerated {
            text: "answer".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        };
        assert_eq!(resp.indexable_text(), Some("answer"));

        let canceled = ThreadEvent::ResponseCanceled {
            text: "partial".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        };
        assert_eq!(canceled.indexable_text(), Some("partial"));
    }

    #[test]
    fn indexable_text_returns_none_for_non_chat_events() {
        assert!(ThreadEvent::TextStreamed {
            text: "chunk".into()
        }
        .indexable_text()
        .is_none());
        assert!(ThreadEvent::ToolCalled {
            name: "x".into(),
            args: json!({}),
            description: String::new()
        }
        .indexable_text()
        .is_none());
        assert!(ThreadEvent::ToolResult {
            name: "x".into(),
            result: "ok".into(),
            images: vec![]
        }
        .indexable_text()
        .is_none());
        assert!(ThreadEvent::SessionStarted {
            session_id: "s".into(),
            branch: String::new(),
            repo_id: None
        }
        .indexable_text()
        .is_none());
        assert!(ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown
        }
        .indexable_text()
        .is_none());
        assert!(ThreadEvent::ThreadTitleGenerated { title: "t".into() }
            .indexable_text()
            .is_none());
    }

    #[test]
    fn user_question_asked_serialization() {
        let event = ThreadEvent::UserQuestionAsked {
            tool_use_id: "tu_1".into(),
            cc_session_id: "sess_abc".into(),
            question: "Pick one:".into(),
            options: vec![
                QuestionOption {
                    id: "o1".into(),
                    label: "First".into(),
                    description: Some("desc".into()),
                },
                QuestionOption {
                    id: "o2".into(),
                    label: "Second".into(),
                    description: None,
                },
            ],
            worktree_path: Some("/tmp/cc-abc".into()),
        };
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(v["type"], "UserQuestionAsked");
        assert_eq!(v["tool_use_id"], "tu_1");
        assert_eq!(v["cc_session_id"], "sess_abc");
        assert_eq!(v["question"], "Pick one:");
        assert_eq!(v["options"][0]["id"], "o1");
        assert_eq!(v["options"][0]["label"], "First");
        assert_eq!(v["options"][0]["description"], "desc");
        assert_eq!(v["options"][1]["id"], "o2");
        assert!(
            v["options"][1].get("description").is_none(),
            "None description should be skipped"
        );
        assert_eq!(v["worktree_path"], "/tmp/cc-abc");
    }

    #[test]
    fn user_question_asked_empty_options_skipped() {
        let event = ThreadEvent::UserQuestionAsked {
            tool_use_id: "tu_1".into(),
            cc_session_id: "sess_abc".into(),
            question: "Continue?".into(),
            options: vec![],
            worktree_path: None,
        };
        let v = serde_json::to_value(&event).unwrap();
        assert!(
            v.get("options").is_none(),
            "empty options should be skipped"
        );
        assert!(
            v.get("worktree_path").is_none(),
            "None worktree_path should be skipped — keeps payload small for the common case"
        );
    }

    #[test]
    fn user_question_asked_event_type() {
        let event = ThreadEvent::UserQuestionAsked {
            tool_use_id: "tu_1".into(),
            cc_session_id: "sess_abc".into(),
            question: "?".into(),
            options: vec![],
            worktree_path: None,
        };
        assert_eq!(event.event_type(), "UserQuestionAsked");
        assert!(event.is_persisted(), "UserQuestionAsked must be persisted");
    }

    #[test]
    fn user_question_answered_selected_serialization() {
        let event = ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu_1".into(),
            answer: AnswerKind::Selected {
                option_id: "o1".into(),
            },
        };
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(v["type"], "UserQuestionAnswered");
        assert_eq!(v["tool_use_id"], "tu_1");
        assert_eq!(v["answer"]["kind"], "Selected");
        assert_eq!(v["answer"]["option_id"], "o1");
    }

    #[test]
    fn user_question_answered_free_text_serialization() {
        let event = ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu_1".into(),
            answer: AnswerKind::FreeText {
                text: "let's do X".into(),
            },
        };
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(v["answer"]["kind"], "FreeText");
        assert_eq!(v["answer"]["text"], "let's do X");
    }

    #[test]
    fn user_question_answered_canceled_serialization() {
        let event = ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu_1".into(),
            answer: AnswerKind::Canceled,
        };
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(v["answer"]["kind"], "Canceled");
    }

    #[test]
    fn user_question_answered_event_type() {
        let event = ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu_1".into(),
            answer: AnswerKind::Canceled,
        };
        assert_eq!(event.event_type(), "UserQuestionAnswered");
        assert!(
            event.is_persisted(),
            "UserQuestionAnswered must be persisted"
        );
    }

    #[test]
    fn user_question_round_trips_through_db_payload() {
        // Old DB rows or fresh inserts must deserialize cleanly.
        let cases = [
            r#"{"type":"UserQuestionAsked","tool_use_id":"tu","cc_session_id":"s","question":"q"}"#,
            r#"{"type":"UserQuestionAsked","tool_use_id":"tu","cc_session_id":"s","question":"q","options":[{"id":"a","label":"A"}]}"#,
            r#"{"type":"UserQuestionAnswered","tool_use_id":"tu","answer":{"kind":"Selected","option_id":"a"}}"#,
            r#"{"type":"UserQuestionAnswered","tool_use_id":"tu","answer":{"kind":"FreeText","text":"hi"}}"#,
            r#"{"type":"UserQuestionAnswered","tool_use_id":"tu","answer":{"kind":"Canceled"}}"#,
        ];
        for raw in cases {
            let parsed: Result<ThreadEvent, _> = serde_json::from_str(raw);
            assert!(
                parsed.is_ok(),
                "Failed to deserialize {}: {:?}",
                raw,
                parsed.err()
            );
        }
    }

    #[test]
    fn session_ended_reason_serialization() {
        // Each emit-able variant round-trips on the wire.
        for (reason, expected) in [
            (SessionEndReason::Shutdown, "shutdown"),
            (SessionEndReason::Panic, "panic"),
            (SessionEndReason::Closed, "closed"),
            (SessionEndReason::StaleResume, "stale_resume"),
        ] {
            let event = ThreadEvent::SessionEnded { reason };
            let serialized = serde_json::to_value(&event).unwrap();
            assert_eq!(serialized["type"], "SessionEnded");
            assert_eq!(
                serialized["reason"], expected,
                "{:?} must serialize as {:?}",
                reason, expected
            );
        }

        // Backwards compat: old DB rows without a `reason` field deserialize
        // as `LegacyNonTerminal` via the serde default.
        let old: ThreadEvent = serde_json::from_str(r#"{"type":"SessionEnded"}"#).unwrap();
        match old {
            ThreadEvent::SessionEnded { reason } => {
                assert_eq!(reason, SessionEndReason::LegacyNonTerminal)
            }
            _ => panic!("wrong variant"),
        }

        // Backwards compat: removed reasons (completed, changes_proposed,
        // changes_applied, auto_ended, user_ended, discarded) on legacy rows
        // deserialize via `#[serde(other)]` to `LegacyNonTerminal` so old data
        // doesn't crash the engine.
        for legacy in [
            "completed",
            "user_ended",
            "changes_proposed",
            "changes_applied",
            "auto_ended",
            "discarded",
        ] {
            let raw = format!(r#"{{"type":"SessionEnded","reason":"{}"}}"#, legacy);
            let parsed: ThreadEvent = serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("legacy reason {:?} should deserialize: {}", legacy, e));
            match parsed {
                ThreadEvent::SessionEnded { reason } => assert_eq!(
                    reason,
                    SessionEndReason::LegacyNonTerminal,
                    "legacy reason {:?} should map to LegacyNonTerminal",
                    legacy
                ),
                _ => panic!("wrong variant for legacy reason {:?}", legacy),
            }
        }
    }

    #[test]
    fn session_recovered_event_can_carry_engine_origin() {
        let event = ThreadEvent::SessionRecovered {
            branch: "claude-code/20260422".into(),
            origin: Some(MessageOrigin::Engine {
                reason: EngineReason::SessionRecovered,
            }),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "SessionRecovered");
        assert_eq!(json["origin"]["kind"], "engine");
        assert_eq!(json["origin"]["reason"]["kind"], "session_recovered");
    }

    #[test]
    fn session_recovered_event_origin_defaults_to_none_when_missing() {
        // Old DB rows without origin must deserialize cleanly.
        let json = r#"{"type":"SessionRecovered","branch":"claude-code/20260318"}"#;
        let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
        match parsed {
            ThreadEvent::SessionRecovered { branch, origin } => {
                assert_eq!(branch, "claude-code/20260318");
                assert!(origin.is_none());
            }
            other => panic!("expected SessionRecovered, got {:?}", other),
        }
    }

    #[test]
    fn change_proposed_event_can_carry_engine_origin() {
        let event = ThreadEvent::ChangeProposed {
            change_id: "abc".into(),
            description: Some("stale session cleanup".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: Some(MessageOrigin::Engine {
                reason: EngineReason::StaleSession,
            }),
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "ChangeProposed");
        assert_eq!(json["origin"]["kind"], "engine");
        assert_eq!(json["origin"]["reason"]["kind"], "stale_session");
    }

    #[test]
    fn change_proposed_event_origin_defaults_to_none_when_missing() {
        // Old DB rows without origin must deserialize cleanly.
        let json = r#"{"type":"ChangeProposed","change_id":"x","description":"y","files":[],"requires_restart":false}"#;
        let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
        match parsed {
            ThreadEvent::ChangeProposed { change_id, origin, .. } => {
                assert_eq!(change_id, "x");
                assert!(origin.is_none());
            }
            other => panic!("expected ChangeProposed, got {:?}", other),
        }
    }

    #[test]
    fn change_proposed_event_can_carry_orphan_recovery_origin() {
        let event = ThreadEvent::ChangeProposed {
            change_id: "def".into(),
            description: Some("orphan cleanup".into()),
            files: vec!["src/lib.rs".into()],
            requires_restart: false,
            origin: Some(MessageOrigin::Engine {
                reason: EngineReason::OrphanRecovery,
            }),
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["origin"]["reason"]["kind"], "orphan_recovery");
    }

    #[test]
    fn coding_agent_prompt_sent_can_carry_orphan_recovery_origin() {
        let event = ThreadEvent::CodingAgentPromptSent {
            text: "resume after restart".into(),
            agent: crate::runtime::AgentKind::ClaudeCode,
            origin: Some(MessageOrigin::Engine {
                reason: EngineReason::OrphanRecovery,
            }),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "CodingAgentPromptSent");
        assert_eq!(json["origin"]["kind"], "engine");
        assert_eq!(json["origin"]["reason"]["kind"], "orphan_recovery");
    }

    #[test]
    fn coding_agent_prompt_sent_can_carry_harden_retrigger_origin() {
        let event = ThreadEvent::CodingAgentPromptSent {
            text: "Run /harden now.".into(),
            agent: crate::runtime::AgentKind::ClaudeCode,
            origin: Some(MessageOrigin::Engine {
                reason: EngineReason::HardenRetrigger,
            }),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "CodingAgentPromptSent");
        assert_eq!(json["origin"]["kind"], "engine");
        assert_eq!(json["origin"]["reason"]["kind"], "harden_retrigger");
    }

    #[test]
    fn coding_agent_prompt_sent_origin_defaults_to_none_when_missing() {
        let json = r#"{"type":"CodingAgentPromptSent","text":"hi"}"#;
        let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
        match parsed {
            ThreadEvent::CodingAgentPromptSent { text, origin, .. } => {
                assert_eq!(text, "hi");
                assert!(origin.is_none());
            }
            other => panic!("expected CodingAgentPromptSent, got {:?}", other),
        }
    }

    #[test]
    fn trigger_started_can_carry_scheduler_origin() {
        let id = uuid::Uuid::new_v4().to_string();
        let event = ThreadEvent::TriggerStarted {
            trigger_id: id.clone(),
            trigger_name: Some("nightly".into()),
            prompt: Some("run".into()),
            invocation: Some(TriggerInvocation::Schedule),
            origin: Some(MessageOrigin::Engine {
                reason: EngineReason::Scheduler {
                    trigger_id: id.clone(),
                    trigger_name: Some("nightly".into()),
                },
            }),
            go_to_review: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "TriggerStarted");
        assert_eq!(json["origin"]["reason"]["kind"], "scheduler");
        assert_eq!(json["origin"]["reason"]["trigger_id"], id);
        assert_eq!(json["origin"]["reason"]["trigger_name"], "nightly");
    }

    #[test]
    fn trigger_started_origin_defaults_to_none_when_missing() {
        let json = r#"{"type":"TriggerStarted","trigger_id":"id"}"#;
        let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
        match parsed {
            ThreadEvent::TriggerStarted { origin, .. } => assert!(origin.is_none()),
            other => panic!("expected TriggerStarted, got {:?}", other),
        }
    }

    // ---- `mode` field deserialization for MessageReceived ----

    #[test]
    fn message_received_mode_field_deserializes() {
        let json = r#"{"type":"MessageReceived","text":"hi","mode":"engine"}"#;
        let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
        match parsed {
            ThreadEvent::MessageReceived { mode, .. } => assert_eq!(mode, ActorMode::Engine),
            other => panic!("expected MessageReceived, got {:?}", other),
        }
    }

    /// Legacy DB rows predating the `mode` field must replay as `Human` so
    /// historical events keep loading. New emissions are forced by the API
    /// layer to set `mode` explicitly.
    #[test]
    fn message_received_no_mode_defaults_to_human() {
        let json = r#"{"type":"MessageReceived","text":"hi"}"#;
        let parsed: ThreadEvent = serde_json::from_str(json).unwrap();
        match parsed {
            ThreadEvent::MessageReceived { mode, .. } => assert_eq!(mode, ActorMode::Human),
            other => panic!("expected MessageReceived, got {:?}", other),
        }
    }

    /// ImageUploaded is a per-thread audit fact emitted by POST /threads/:id/blobs.
    /// The hash uniquely identifies the blob bytes (sha256 hex, 64 chars). The
    /// mime + byte_size are convenience fields so consumers can render the
    /// upload entry without a HEAD on the blob endpoint. Past-tense, persisted.
    #[test]
    fn image_uploaded_serializes_with_all_fields() {
        let event = ThreadEvent::ImageUploaded {
            hash: "a".repeat(64),
            mime: "image/png".to_string(),
            byte_size: 4096,
            actor: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "ImageUploaded");
        assert_eq!(json["hash"], "a".repeat(64));
        assert_eq!(json["mime"], "image/png");
        assert_eq!(json["byte_size"], 4096);
        // actor: None must skip-serialize so the wire shape matches the
        // pattern used by ThreadStarted / ThreadDiscarded — frontend treats
        // missing actor as "unknown", not as a literal null.
        assert!(json.get("actor").is_none());
    }

    /// ImageUploaded is reported by `event_type()` so the projection / SSE
    /// dispatcher can route by name without matching on the variant. The
    /// name must match the PascalCase variant exactly (used in JSONB queries).
    #[test]
    fn image_uploaded_event_type_is_pascal_case_name() {
        let event = ThreadEvent::ImageUploaded {
            hash: "b".repeat(64),
            mime: "image/jpeg".to_string(),
            byte_size: 1,
            actor: None,
        };
        assert_eq!(event.event_type(), "ImageUploaded");
    }

    /// ImageUploaded is past-tense and represents a durable fact (the user
    /// attached this image). `is_persisted()` must agree so the EventBus
    /// writes a row to the events table — without persistence the audit
    /// trail and migration story collapse.
    #[test]
    fn image_uploaded_is_persisted() {
        let event = ThreadEvent::ImageUploaded {
            hash: "c".repeat(64),
            mime: "image/webp".to_string(),
            byte_size: 1,
            actor: None,
        };
        assert!(event.is_persisted());
    }
}
