use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::core::event_subscription::EventSubscription;
use crate::engine::thread_events::permission::Action;
use crate::engine::thread_events::CodingAgent;

use super::{
    AbortCause, ActorMode, AnswerKind, ApplyNowOutcome, CacheKeepAliveStatus, CancelCause,
    CaptureFormat, ChildCompletionStatus, CodingAgentExit, Dashboard, EventWaitCancelCause,
    FireRetry, FormRequestOutcome, HandoffConsent, LegacyArrangedTile, McpEnablementSource,
    MessageOrigin, OwnerApproval, ProviderError, QuestionOption, RecalledMemory, RetryOf,
    SessionEndReason, TodoItem, TriggerInvocation, UnproposedReason, UnqueueReason,
    VoiceSessionEndReason, WakeBucket, WakeMode, WakeSource,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum BackgroundWaitTiming {
    Queued,
    Running {
        deadline: chrono::DateTime<chrono::Utc>,
    },
}

/// INVARIANT: Initial restored composer, inserted only when its fresh destination is absent.
/// The generation fences requests delayed across another workspace restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveredCompose {
    pub generation: uuid::Uuid,
    pub text: String,
    pub image_hashes: Vec<String>,
    pub selection: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDraft {
    pub text: String,
    pub image_hashes: Vec<String>,
    pub selection: Option<Value>,
    pub drafted_by_thread_id: uuid::Uuid,
}

fn default_coding_agent_claude_code() -> CodingAgent {
    CodingAgent::ClaudeCode
}

fn is_empty_str(s: &str) -> bool {
    s.is_empty()
}
fn is_false(b: &bool) -> bool {
    !b
}
fn is_default_coding_agent_kind(k: &crate::engine::thread_events::CodingAgentKind) -> bool {
    matches!(k, crate::engine::thread_events::CodingAgentKind::Lucidos)
}
fn default_session_ended_reason() -> SessionEndReason {
    SessionEndReason::LegacyNonTerminal
}

/// INVARIANT: Backward-compat default for `MessageReceived.mode` on DB rows persisted
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

fn default_inject_mode() -> ActorMode {
    ActorMode::Human
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ThreadEvent {
    MessageReceived {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        user_image_hashes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image_description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_thread_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawning_event_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wake_mode: Option<WakeMode>,
        /// INVARIANT: Semantic mode of the actor that emitted this message. Required for
        /// new emissions (enforced at the API layer). The serde default exists
        /// only to replay old DB rows persisted before the `mode` field existed.
        #[serde(default = "default_mode_human")]
        mode: ActorMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        /// INVARIANT: The backend this message was PINNED to: an explicit pick, or one
        /// this thread remembered for the same model. Stamped beside the model
        /// so the pick outlives a later change to the model's *preferred
        /// provider*. `None` means nothing was pinned, so the row decides.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voice_session_id: Option<uuid::Uuid>,
    },
    /// INVARIANT: User removed a queued chat follow-up before the agentic loop ingested it.
    /// The original `MessageReceived` stays in the append-only log. Renderers
    /// hide it while it is still stepless, and the agentic loop skips the
    /// matching injected prompt when it drains the queue.
    QueuedMessageRemoved {
        removed_message_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        restore_to_compose: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compose: Option<Box<crate::core::store::ComposeReport>>,
    },
    MessageHeld {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        user_image_hashes: Vec<String>,
        #[serde(default = "default_mode_human")]
        mode: ActorMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    /// INVARIANT: A `MessageHeld` was handed to the agent. Emitted just before its
    /// `MessageReceived`, so a message is released at most once.
    HeldMessageReleased {
        held_message_id: uuid::Uuid,
    },
    TextStreamed {
        text: String,
    },
    #[serde(alias = "Thinking")]
    ThoughtStreamed {
        text: String,
    },
    /// INVARIANT: One captured context per LLM call. `usage` is None pre-call and on
    /// providers that don't report it (OpenAI, Gemini). When present it
    /// reflects the real prompt-token cost from the provider's `usage`
    /// block. `estimated_total_tokens` is the engine's pre-call estimate
    /// (`estimate_tokens_from_chars`, a measured 2.5 chars/token, NOT the
    /// trim budget's conservative 1.5). The modal renders both so the user
    /// can spot estimator drift.
    ///
    /// A section's `budget_delta_chars` is the one that sums, and over a
    /// capture they sum to exactly the chars behind `estimated_total_tokens`.
    /// That is what lets the LLM Context Viewer render each section as a share
    /// of the headline total. Without it the panel would re-derive a ratio of
    /// its own. Keep it true when adding a section, and never sum
    /// `content_chars`: on `Conversation` it counts the tree a second time.
    ///
    /// **Not every capture is a turn.** `purpose` names which call this was.
    /// An *auxiliary model call* emits one too, so token accounting sees the
    /// engine's own spend. Build those through
    /// [`crate::engine::AuxCapture`], never by hand: it is what keeps
    /// `producer` and `purpose` agreeing. The transcript projection renders
    /// only `Turn` rows. A capture binds to the step it follows, so an aux
    /// row would overwrite that step's context chip.
    ContextCaptured {
        producer: crate::engine::thread_events::ContextProducer,
        model: String,
        context_window: usize,
        sections: Vec<crate::engine::thread_events::ContextSection>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tools: Vec<String>,
        estimated_total_tokens: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<crate::llm::api_usage::ApiUsage>,
        #[serde(default, skip_serializing_if = "is_false")]
        trimmed: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        trim_passes: Vec<u8>,
        #[serde(
            default,
            skip_serializing_if = "crate::engine::thread_events::ContextPurpose::is_turn"
        )]
        purpose: crate::engine::thread_events::ContextPurpose,
        /// INVARIANT: True when the row was rebuilt after the fact from other events
        /// rather than recorded at the call. Its numbers are estimates, and
        /// it deliberately carries no `usage`, so a rollup that filters on a
        /// present `usage` block keeps reporting measured spend only. Written
        /// by `core::aux_context_backfill` and by nothing else.
        #[serde(default, skip_serializing_if = "is_false")]
        reconstructed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stable_prefix_fingerprint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        served_model: Option<String>,
        /// INVARIANT: The backend the router chose or the coding agent reported
        /// (`claude-gateway`, `vertex`, ...). Coding-agent calls use `unknown`
        /// when unresolved; `CodingAgentBillingObserved` preserves why.
        /// Absent on older captures without provider attribution. Gateway
        /// telemetry selects engine-owned producers as well as this field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// INVARIANT: The caller ran in a CLI process that owns its telemetry. Engine
        /// exporters must leave this capture to that process. Defaults false
        /// for older captures; producer-based exclusions still apply.
        #[serde(default, skip_serializing_if = "is_false")]
        telemetry_owned_by_cli: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttft_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<u32>,
    },
    /// INVARIANT: Secret-free billing evidence observed from a coding-agent process.
    /// Scoped to this launch, never a pin to replay as current auth on resume.
    CodingAgentBillingObserved {
        coding_agent: CodingAgent,
        launch_id: uuid::Uuid,
        attribution: crate::engine::thread_events::billing_provider::BillingAttribution,
        #[serde(default)]
        billing_mode: crate::engine::thread_events::billing_provider::BillingMode,
    },
    /// INVARIANT: A metered chat call that failed. It is **never a `ContextCaptured`**: a
    /// capture binds to a transcript step and every usage rollup sums it, while
    /// a failed call has no context to show and no spend. Written for any
    /// provider. The gateway telemetry exporter selects on `provider`.
    ///
    /// Carries facts only. A provider's error body can echo request content, so
    /// no message is stored here: a turn's `ResponseFailed` keeps its own.
    ModelCallFailed {
        model: String,
        #[serde(
            default,
            skip_serializing_if = "crate::engine::thread_events::ContextPurpose::is_turn"
        )]
        purpose: crate::engine::thread_events::ContextPurpose,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        duration_ms: u64,
        attempts: u32,
        failure: crate::llm::call_trace::CallFailure,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status_code: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },
    #[serde(alias = "MemorySearched")]
    MemoryRecalled {
        #[serde(default)]
        results: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        queries: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        memories: Vec<RecalledMemory>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_called_event_id: Option<uuid::Uuid>,
    },
    TodoListWritten {
        items: Vec<TodoItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
    BackgroundBashStarted {
        task_id: String,
        command: String,
        timeout_secs: u64,
        started_at: chrono::DateTime<chrono::Utc>,
    },
    BackgroundBashCompleted {
        task_id: String,
        command: String,
        /// INVARIANT: The child's normal exit status. `None` whenever there isn't one:
        /// the child died on a signal (see `signal`), or the engine failed to
        /// reap it at all. Never a stand-in number: a reader that sees
        /// `exit_code: 0` can trust the command really exited 0.
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
        stdout: String,
        stderr: String,
        started_at: chrono::DateTime<chrono::Utc>,
        finished_at: chrono::DateTime<chrono::Utc>,
        #[serde(default, skip_serializing_if = "is_false")]
        timed_out: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        killed: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        abandoned: bool,
    },
    /// INVARIANT: The engine started a *confinement run* as background task `task_id`,
    /// under permission `permission_id`. `revision` is the worktree's `HEAD`
    /// when the tree was clean, and absent when it was dirty. The paired
    /// `BackgroundBashCompleted` carries the result. A passing run certifies
    /// its own clean revision for a later harden retry.
    ConfinementRunStarted {
        task_id: String,
        permission_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<String>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spend_cap: Option<crate::llm::spend_cap::SpendCapNotice>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_exit: Option<CodingAgentExit>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_error: Option<ProviderError>,
    },

    #[serde(alias = "SessionRecovered", alias = "SessionResumed")]
    ContinuationStarted {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        branch: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// INVARIANT: The `ResponseAborted` a chat or trigger Continue resumes. A unique
        /// index lets one `ContinuationStarted` claim each abort, so a double
        /// Continue spawns one rerun. `None` on the coding-agent path, the
        /// answer-driven fallback and legacy rows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resumed_abort_id: Option<uuid::Uuid>,
    },
    SessionStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sandbox: Option<super::SandboxMode>,
        session_id: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        branch: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo_id: Option<String>,
        #[serde(default, skip_serializing_if = "is_default_coding_agent_kind")]
        coding_agent_kind: crate::engine::thread_events::CodingAgentKind,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        coding_agent_folder: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        #[serde(default = "default_coding_agent_claude_code")]
        coding_agent: CodingAgent,
        #[serde(default, skip_serializing_if = "is_false")]
        integrated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auto_compact_window: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auto_compact_arm: Option<super::auto_compact::AutoCompactArm>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        continuity: Option<super::SessionContinuity>,
    },
    SessionEnded {
        /// INVARIANT: Why the session ended. Always serialized, because the frontend reads
        /// `reason` to distinguish a normal completion from a system abort,
        /// and `Completed` must reach the wire as `"completed"`. The default
        /// applies only when reading old DB rows persisted before `reason`
        /// existed. A row written before the terminal-only reasons still
        /// carries a retired value. The snapshot serves the raw column, so a
        /// reader must tolerate any string here.
        #[serde(default = "default_session_ended_reason")]
        reason: SessionEndReason,
    },
    SuccessorThreadSpawned {
        successor_thread_id: uuid::Uuid,
        base_sha: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        starts_build_chain: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handoff_consent: Option<HandoffConsent>,
    },
    SessionHandedOff {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<super::HandoffCause>,
        #[serde(default)]
        starts: super::SessionStart,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crossing: Option<crate::engine::thread_events::CeilingCrossing>,
        opening: super::HandoffOpening,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<String>,
        coding_agent: CodingAgent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        coding_agent_model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<crate::engine::thread_events::HandoffRequest>,
    },
    HandoffCheckpointReached {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<crate::engine::thread_events::checkpoint::CheckpointProgress>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        offered: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pays: Option<bool>,
        #[serde(default, skip_serializing_if = "is_false")]
        default_floor: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        floor_rung: Option<crate::llm::intelligence_rungs::IntelligenceRung>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        risk_rung: Option<crate::llm::intelligence_rungs::IntelligenceRung>,
    },
    SuccessorOfferDeclined {
        input_tokens: u64,
    },
    SuccessorRequested {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        coding_agent: Option<CodingAgent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        coding_agent_model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
    },
    ContextCeilingSet {
        ceiling: crate::engine::thread_events::ContextCeiling,
        source: crate::engine::thread_events::CeilingSource,
    },
    ContextCeilingReached {
        context_tokens: u64,
        ceiling: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<u64>,
    },
    SessionHandoffWithdrawn,
    CodingAgentCompacted {
        trigger: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        post_tokens: Option<u64>,
        #[serde(default = "default_coding_agent_claude_code")]
        coding_agent: CodingAgent,
    },
    CodingAgentModelSubstituted {
        requested: String,
        served: String,
        coding_agent: CodingAgent,
    },
    #[serde(alias = "ClaudeCodeTextStreamed")]
    CodingAgentTextStreamed {
        text: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        progress_note: bool,
    },
    #[serde(alias = "ClaudeCodeThoughtStreamed")]
    CodingAgentThoughtStreamed {
        text: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
    },
    #[serde(alias = "ClaudeCodeToolCalled")]
    CodingAgentToolCalled {
        name: String,
        args: Value,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        description: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
        /// INVARIANT: Agent-issued identifier for this tool invocation, persisted so the
        /// matching `CodingAgentToolResult` can be paired with the call even
        /// when an `EXCHANGE_START_TYPES` event (e.g. permission prompt)
        /// splits them across exchanges. Empty for legacy DB rows.
        #[serde(default, skip_serializing_if = "is_empty_str")]
        tool_use_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_call_id: Option<String>,
    },
    #[serde(alias = "ClaudeCodeToolResult")]
    CodingAgentToolResult {
        name: String,
        result: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        tool_use_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },
    #[serde(alias = "ClaudeCodeUserMessageSent")]
    CodingAgentUserMessageSent {
        text: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
    },
    #[serde(alias = "ClaudeCodePromptSent")]
    CodingAgentPromptSent {
        text: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        /// INVARIANT: The event that carried the forwarded input, as on
        /// `CodingAgentInputRead`: the `MessageReceived` of a user follow-up.
        /// `None` for an engine prompt with no starter event. The turn gap
        /// counts this event as a boundary only once a `CodingAgentInputRead`
        /// names that input, or this event itself for an engine prompt.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_event_id: Option<uuid::Uuid>,
    },
    CodingAgentInputRead {
        input_event_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "is_false")]
        started_turn: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        verbatim: bool,
    },
    /// INVARIANT: The coding agent took a child's report mid-turn, through its own
    /// `lucidos` call, before its driver delivered the queued copy. The engine
    /// withdrew that copy, so the report reaches the agent once. Follows the
    /// card, so the boot refire treats the card as handled.
    ChildReportTaken {
        input_event_id: uuid::Uuid,
    },
    /// INVARIANT: An effort restart interrupted the turn before the coding agent read
    /// the input that opened it, so the agent never will. The input stays
    /// owed until a later `CodingAgentInputRead` names it, and the next
    /// session to start delivers it (`agent_session::stranded_follow_ups`).
    CodingAgentInputDiscarded {
        input_event_id: uuid::Uuid,
    },
    /// INVARIANT: Emitted when the engine detects that a coding-agent session ended without
    /// running the required hardening. A recovery hardening session is spawned
    /// automatically. This is NOT a completion event: the thread stays active
    /// until hardening finishes.
    MissingHardeningDetected {
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
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
        /// INVARIANT: Optional short tag describing why this idle was emitted. Most idles
        /// have no `reason` (the agent simply finished its turn). Recovery
        /// emits `Some("engine_restart_interrupt")` when a mid-turn-crashed
        /// session is surfaced to the UI as "interrupted, click to continue"
        /// instead of being auto-spawned. The frontend reads this field to
        /// render the continue affordance. An engine turn stamps
        /// `"cache_keep_alive"` or `"ceiling_reentry"`, so no trigger or event
        /// wait reads its idle as the agent finishing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_head_sha: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        bg_bash_pending: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_end: Option<super::TurnEndFacts>,
    },
    /// INVARIANT: Continuation requested, emitted when a CC turn that was interrupted
    /// (engine restart mid-turn, Q9a recovery path) needs to be resumed
    /// without a new user message. Four emit sites: HTTP `/continue`
    /// (user clicks Continue), the *in-loop watchdog* (10-min silence with
    /// the gate open), the *external watchdog* (12-min wedged `select!`),
    /// and the missing-hardening recovery sweep. Picked up by the spawn
    /// dispatcher (Phase 5, Task 5.2), which re-spawns CC via `--resume`
    /// with no new input. The dispatcher uses the event's id as the
    /// idempotency key so a single `ContinuationRequested` produces exactly
    /// one spawn.
    ///
    /// Past name was `ContinueSignal`. The rename migration
    /// (`20260518212540_rename_continue_signal_to_continuation_requested`)
    /// rewrote existing rows, and the serde alias remains as a safety net
    /// for any in-flight JSON not yet persisted. Renamed to past-tense
    /// per the events-only model
    /// (`AppUiRefreshRequested` not `RefreshAppUI`) and to pair with the
    /// already-past-tense `ContinuationStarted` terminal that opens the
    /// resulting exchange.
    ///
    /// `reason` is a short tag describing why a continuation was needed
    /// (e.g. `"engine_restart_interrupt"`); it is purely informational and
    /// surfaced for debugging / route-popover context.
    #[serde(alias = "ContinueSignal")]
    ContinuationRequested {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        reason: String,
    },

    ThreadTitleGenerated {
        title: String,
    },
    ThreadTitleRenamed {
        title: String,
    },
    ThreadSaved,
    ThreadUnsaved,
    ThreadMovedToFolder {
        #[serde(default)]
        folder: Option<String>,
    },
    ThreadArchived,
    ThreadArchiveRequested,
    ThreadUnarchived,
    ThreadReadRequested {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        parent_report: bool,
    },
    ThreadReadNotRequested {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        supersedes_request: bool,
    },
    ParentWakeRequested,
    ParentWakeNotRequested,
    /// INVARIANT: The user saw the reply a read request pointed at: its end stayed on
    /// screen for the seen dwell. Clears `read_requested`, or with `personal`
    /// adds the actor's device to `reply_seen_devices`. Recorded only while
    /// the thread's summary still reads `seen_version`, so a duplicate or a
    /// sighting older than a newer request records nothing.
    ThreadReplySeen {
        seen_version: i64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        personal: bool,
    },
    ThreadTriageProposed {
        entries: Vec<super::TriageProposalEntry>,
    },
    #[serde(alias = "WorkItemDriverTaken")]
    RootThreadDriverTaken,
    #[serde(alias = "WorkItemDriverReleased")]
    RootThreadDriverReleased,
    /// INVARIANT: A gate waited on its driver past `driver_timeout_hours`, so it now
    /// reaches the whole team. Recorded once per gate, on the family root.
    /// The driver keeps the root thread.
    #[serde(alias = "WorkItemGateEscalated")]
    RootThreadGateEscalated {
        gate_thread_id: uuid::Uuid,
        tool_use_id: String,
    },
    #[serde(alias = "WorkItemWatched")]
    RootThreadWatched {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_id: Option<String>,
    },
    #[serde(alias = "WorkItemUnwatched")]
    RootThreadUnwatched,
    UserOnlyMessagePosted {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        user_image_hashes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mentioned_user_ids: Vec<String>,
    },
    AgentPaused {
        cause: super::AgentPauseCause,
    },
    AgentResumed {
        cause: super::AgentPauseCause,
    },
    AutomationPaused {
        reason: String,
    },
    AutomationUnpaused {
        cause: super::AutomationUnpauseCause,
    },
    ThreadHoldPlaced {
        reason: String,
        hold_id: uuid::Uuid,
        root_thread_id: uuid::Uuid,
    },
    ThreadHoldLifted {
        hold_id: uuid::Uuid,
        cause: super::HoldLiftCause,
    },
    /// INVARIANT: The owner's pick on a *permission request* granted a *permission*.
    /// `request_tool_use_id` names the card, and `picked_by` says whether a tap,
    /// typed words or spoken words picked it. The meta actor is the owner's
    /// device. A tool card's "Allow for this thread" carries the grant file
    /// line it stores as `pattern`, which the lane's own matcher reads.
    PermissionGranted {
        permission_id: uuid::Uuid,
        action: crate::engine::thread_events::permission::Action,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        breadth: Option<String>,
        lifetime: crate::engine::thread_events::permission::Lifetime,
        holder: crate::engine::thread_events::permission::Holder,
        request_tool_use_id: String,
        picked_by: crate::engine::thread_events::permission::PickedBy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
    },
    PermissionSpent {
        permission_id: uuid::Uuid,
    },
    /// INVARIANT: The gateway explicitly refused without changing anything. Copy the spent
    /// once grant under a new id, keeping its original request and turn boundary.
    /// A unique index permits at most one restoration of each spend.
    PermissionRestored {
        spent: uuid::Uuid,
        restored: uuid::Uuid,
    },
    PermissionRevoked {
        permission_id: uuid::Uuid,
    },
    /// INVARIANT: An agent prepared a *permission request* on its own thread, through
    /// `POST /api/v1/permission-requests`. The engine wrote `card`. It shows
    /// when the agent asks its question tool with `request_id` as its only
    /// question. A request grants nothing until the owner picks an option.
    PermissionRequestPrepared {
        request_id: String,
        request: crate::engine::thread_events::permission::PermissionRequest,
        card: crate::engine::thread_events::permission::PreparedCard,
    },
    BatchItemRan {
        permission_id: uuid::Uuid,
        index: u32,
        item: crate::engine::thread_events::permission::batch::BatchItem,
        outcome: crate::engine::thread_events::permission::batch::BatchItemOutcome,
    },
    ThreadStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovered_compose: Option<RecoveredCompose>,
        /// INVARIANT: Set only by the agent-draft service, never beside `recovered_compose`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_draft: Option<AgentDraft>,
        mode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ThreadDiscarded {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    HomeThreadCreated,
    /// INVARIANT: A user attached an image to this thread's compose draft. Emitted by
    /// POST /api/v1/threads/:id/blobs after the bytes are content-addressed
    /// to disk under `data/blobs/<hh>/<hash>.<ext>`. The `hash` is the sole
    /// identity used by every downstream consumer (compose payload, message
    /// payload, LLM call). `mime` and `byte_size` are convenience fields, so
    /// SSE subscribers can render the upload entry without fetching the
    /// blob. One blob attached to two threads leaves two events, one per
    /// thread. The disk write is a no-op the second time, and the per-thread
    /// fact stays distinct.
    ImageUploaded {
        hash: String,
        mime: String,
        byte_size: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    TriggerStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_of: Option<RetryOf>,
        #[serde(alias = "task_id")]
        trigger_id: String,
        #[serde(default, alias = "task_name", skip_serializing_if = "Option::is_none")]
        trigger_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation: Option<TriggerInvocation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        go_to_review: bool,
        /// INVARIANT: The model this fire actually ran on, resolved from the trigger's own
        /// pin or the account chat preference. `TriggerStarted` is a trigger
        /// thread's STARTER event (it has no `MessageReceived`), so this is
        /// where the *per-thread model memory* reads from: without it the
        /// thread's in-thread picker would show the account model while the run
        /// used a different one, and a human follow-up would silently switch.
        /// `None` on legacy rows persisted before the field existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        continue_key: Option<String>,
    },
    TriggerRunInterrupted {
        trigger_id: String,
        trigger_name: String,
        fire_event_id: uuid::Uuid,
        retry: FireRetry,
    },
    TriggerCompleted {
        #[serde(alias = "task_id")]
        trigger_id: String,
        #[serde(default, alias = "task_name", skip_serializing_if = "Option::is_none")]
        trigger_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_summary: Option<String>,
    },

    /// INVARIANT: Planning began, with an explicit purpose supplied by the planning skill.
    /// Does not record approval or open the source edit gate.
    PlanStarted {
        kind: super::PlanKind,
    },

    PlanProposed {
        kind: super::PlanKind,
        plan_path: String,
        plan_sha: String,
        branch_name: String,
    },
    PlanApproved {
        kind: super::PlanApprovalKind,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        plan_path: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        plan_sha: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch_name: Option<String>,
        scope: super::PlanScope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo_root: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan_version: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inherited_from_event_id: Option<uuid::Uuid>,
    },

    ChangeApplyRequested {
        change_id: String,
        branch_name: String,
        repo_root: String,
    },
    ChangeProposed {
        #[serde(default)]
        content: super::ChangeContent,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        requires_restart: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit_sha: Option<String>,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        branch_name: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        repo_root: String,
        /// INVARIANT: `true` if the originating commit landed on a hardened HEAD; surfaced
        /// in the projection so the apply UI can short-circuit re-hardening.
        /// Stamped from `has_full_harden_marker` at end-of-turn aggregate
        /// emit; engine-internal recovery emits resolve it the same way.
        #[serde(default, skip_serializing_if = "is_false")]
        hardened: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        incomplete: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        set_aside: bool,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        path: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        diff: String,
    },
    ChangeApplied {
        #[serde(default)]
        content: super::ChangeContent,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "is_false")]
        requires_restart: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        client_update: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        commits: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_merge_sha: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        post_merge_sha: Option<String>,
        /// INVARIANT: The *landing merge* the landing lane made for this change alone.
        /// Revert undoes exactly this commit. `None` for an app change, and
        /// for every apply made before the lane.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        landing_merge_sha: Option<String>,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        path: String,
    },
    ChangeDiscarded {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        path: String,
    },
    ChangeSetAside {
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hold_id: Option<uuid::Uuid>,
    },
    ChangeBroughtBack {
        change_id: String,
    },
    ChangeWithdrawn {
        change_id: String,
    },
    ProposalWithheld {
        branch_name: String,
        files: Vec<String>,
        reason: UnproposedReason,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        agent_returns: bool,
    },
    ChangeQueued {
        change_id: String,
        snapshot_sha: String,
    },
    ChangeHandoffStarted {
        change_id: String,
    },
    ChangeUnqueued {
        change_id: String,
        reason: UnqueueReason,
    },
    ChangeReverted {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
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
        /// INVARIANT: A merge already landed, but recording its content failed. Retries use
        /// this range instead of treating the landing as an empty no-op.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_merge_sha: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        post_merge_sha: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        landing_merge_sha: Option<String>,
    },
    ApplyNowStarted,
    /// INVARIANT: The Apply Now that `ApplyNowStarted` began is over, whatever happened.
    /// Sets `apply_in_flight` FALSE. The apply task emits it on every exit, and
    /// engine startup emits it as `interrupted` for an apply a stop cut off.
    ApplyNowEnded {
        outcome: ApplyNowOutcome,
    },

    MergeConflictDetected {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    /// INVARIANT: A merge-resolution worktree was set up for a change with conflicts.
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
    MergeResolutionCleared {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
    },
    ChangeHardened {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    ChangeSummarized {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        summary: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        description: String,
    },

    /// INVARIANT: Coding-agent session settings changed mid-session (model or reasoning
    /// effort). Persisted per-thread so settings survive idle exit and respawn.
    /// Older stored rows may also carry a `permission_mode` key, so this variant
    /// must keep ignoring unknown fields.
    ///
    /// Also carries `cc_session_id`: the agent emits this event at `Init` (the
    /// first moment CC reports its session id) with `cc_session_id: Some(..)`,
    /// so the resume/recovery lookups can find the id even when the session is
    /// interrupted by an engine restart *before* it ever reaches a
    /// `CodingAgentIdled` boundary (the only other event that carries it). The
    /// id is CC's authoritative Init fact, exactly like `model`. Settings-only
    /// emits (startup persist, mid-session model/effort changes) pass
    /// `None`; the lookups read the most recent non-null value across both
    /// event types.
    #[serde(alias = "CCSettingsChanged")]
    CodingAgentSettingsChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cc_session_id: Option<String>,
        /// INVARIANT: Absolute `CLAUDE_CONFIG_DIR` the session was created under, stamped
        /// alongside `cc_session_id` at the Init emit. It is the authoritative
        /// session↔config-dir pairing. CC keys each session's transcript on this
        /// dir (`$CLAUDE_CONFIG_DIR/projects/<cwd>/<sid>.jsonl`). So a follow-up
        /// resume re-injects it (see `lookup_pinned_cc_config_dir` and
        /// `SpawnArgs::account_pin`), and a mid-flight user toggle of the
        /// env var cannot strand the session. `None` on legacy rows,
        /// on the pre-Init settings emit, and on mid-session settings-only emits
        /// (model/effort changes). The Init emit is the carrier.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claude_config_dir: Option<String>,
        /// INVARIANT: Whether `CLAUDE_CONFIG_DIR` was actually set for that session, stamped
        /// beside `claude_config_dir`. `false` means it was unset and the dir is
        /// Claude Code's default. The two are different logins to Claude Code, so
        /// a resume must replay which one it was (`AccountPin`). `None` wherever
        /// `claude_config_dir` is, and on rows written before it existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claude_config_dir_explicit: Option<bool>,
    },

    ChatSettingsChanged {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },

    ChatTurnSettingsResolved {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },

    #[serde(alias = "UserPromptInjected")]
    PromptInjected {
        text: String,
        #[serde(default = "default_inject_mode")]
        mode: ActorMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        injected_message_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivered_event_id: Option<uuid::Uuid>,
    },

    // INVARIANT: Interactive (persisted)
    //
    // *Form requests*: the agent asked the user to act on something it put in
    // front of them. Each carries a `request_id` and stays open until one
    // `FormRequestResolved` names it, so a request survives a reload or a lost
    // stream frame. `payload` is the JSON the client renders from, and it
    // never holds a secret.
    #[serde(alias = "CredentialPromptRequested", alias = "CredentialRequest")]
    CredentialRequested {
        request_id: uuid::Uuid,
        payload: String,
    },
    #[serde(alias = "PluginInstallRequest")]
    PluginInstallRequested {
        request_id: uuid::Uuid,
        payload: String,
    },
    #[serde(alias = "PluginUninstallRequest")]
    PluginUninstallRequested {
        request_id: uuid::Uuid,
        payload: String,
    },
    #[serde(alias = "EmailConfirmRequest")]
    EmailConfirmRequested {
        request_id: uuid::Uuid,
        payload: String,
    },
    OAuthAuthorizationRequested {
        request_id: uuid::Uuid,
        payload: String,
    },
    FormRequestResolved {
        request_id: uuid::Uuid,
        outcome: FormRequestOutcome,
    },
    McpConsentRequested {
        tool: String,
        args: Value,
    },

    // INVARIANT: Interactive question card with selectable options. Emitted by both
    // CC's built-in `AskUserQuestion` tool and the chat agent's
    // `ask_user_question` LLM tool. `meta.channel` distinguishes which
    // agent raised it (`claude_code` / `chat`). Resume goes through
    // POST /api/v1/threads/{thread_id}/answer-question, which emits
    // UserQuestionAnswered and dispatches the channel-specific resume:
    // CC respawns its subprocess with --resume + a matching tool_result;
    // chat wakes its in-process tool waiting on the question wait registry.
    //
    // `cc_session_id` is the Claude Code session id at intercept time, used to pin
    // `--resume` to the right CLI conversation. **Empty string** when the
    // chat agent raises the question — chat is in-process and has no
    // resume token.
    //
    // `worktree_path` is the absolute path of the CC worktree at intercept
    // time. CC stores session JSONLs keyed by CWD
    // (`~/.claude/projects/<encoded-cwd>/<sid>.jsonl`), so resume MUST
    // start the new subprocess in the same directory or `--resume` returns
    // "No conversation found". Branch lookup is unreliable here because CC
    // is free to `git checkout -b ...` inside the worktree — see
    // `engine/agent_session/run_session.rs` for how this is used on resume.
    // **`None`** for chat-channel questions.
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner_approval: Option<OwnerApproval>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_request: Option<crate::engine::thread_events::permission::PermissionRequest>,
    },
    /// INVARIANT: A *queued card*: a `UserQuestionAsked` that arrived while the thread
    /// already had an open card. The EventBus records it under this type, so
    /// no client, push or voice path sees it. `card_queue` posts it later as a
    /// `UserQuestionAsked` with the same `tool_use_id`. Same fields as that
    /// event.
    UserQuestionQueued {
        tool_use_id: String,
        cc_session_id: String,
        question: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<QuestionOption>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_path: Option<String>,
        #[serde(default)]
        multi_select: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner_approval: Option<OwnerApproval>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_request: Option<crate::engine::thread_events::permission::PermissionRequest>,
    },
    UserQuestionAnswered {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compose: Option<Box<crate::core::store::ComposeReport>>,
        tool_use_id: String,
        answer: AnswerKind,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        quiet: bool,
    },
    OwnerApprovalRequested {
        request_id: String,
        approval: OwnerApproval,
        question: String,
    },
    OwnerApprovalSpent {
        tool_use_id: String,
        verb: Action,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_thread_id: Option<uuid::Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_workspace_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snapshot_handle: Option<String>,
    },
    /// INVARIANT: An agent acted on a *machine-made workspace* with no card: the
    /// workspace is the agent's. A stop, start, rename or autostart change
    /// comes from `api::workspace_broker`, and a snapshot restore from
    /// `api::snapshots`, which names the snapshot. Emitted before the act,
    /// which may still be refused.
    MachineMadeWorkspaceChanged {
        verb: Action,
        workspace_id: String,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snapshot_handle: Option<String>,
    },

    CodingAgentPermissionRequest {
        request_id: String,
        tool_use_id: String,
        tool_name: String,
        input: Value,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grant_offer: Option<crate::engine::thread_events::GrantOffer>,
    },
    CodingAgentPermissionResolved {
        request_id: String,
        allowed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        persist_scope: Option<crate::engine::thread_events::AllowScope>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        standing_deny: Option<String>,
    },

    CommandPermissionRequested {
        /// INVARIANT: Deciding layer, static evidence and judge distribution. No command text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision_trace: Option<String>,
        request_id: String,
        tool_use_id: String,
        tool_name: String,
        command: String,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grant_head: Option<String>,
        /// INVARIANT: The grants the card offers, which a click stores exactly. Absent on
        /// a card raised before the engine stamped its offer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grant_offer: Option<crate::engine::thread_events::GrantOffer>,
    },
    CommandPermissionResolved {
        request_id: String,
        allowed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        persist_scope: Option<crate::engine::thread_events::AllowScope>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        standing_deny: Option<String>,
    },

    McpPermissionRequested {
        request_id: String,
        tool_use_id: String,
        server_id: String,
        server_name: String,
        tool_name: String,
        arguments_summary: String,
    },
    McpPermissionResolved {
        request_id: String,
        allowed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        persist_scope: Option<crate::engine::thread_events::AllowScope>,
    },

    /// INVARIANT: The command guard bracketed a `ReversibleDanger` command (in-workspace
    /// destruction) with a snapshot of the workspace's git-visible content: ADR
    /// 0002, Phase 4 and its counting addendum. The pre image lives on
    /// `refs/lucidos/command-checkpoints/<checkpoint_id>` and the post image on
    /// `refs/lucidos/command-post-images/<checkpoint_id>`; `command` is the
    /// command that ran and `summary` is the one-line card text. Persisted so
    /// the one-click Undo affordance and the card's diff viewer survive reload.
    ///
    /// Emitted **after** the command returns, and only when the two images
    /// actually differ. A command that changed nothing git-visible (the usual
    /// cause is destruction inside a gitignored path) emits no event, because
    /// its Undo could neither restore nor remove anything. A failed snapshot
    /// likewise emits nothing and lets the command run unguarded, the
    /// pre-Phase-4 behavior.
    ///
    /// `restores` counts the files Undo would put back (deleted or overwritten)
    /// and `removes` the files it would delete (created by the command); both
    /// default to 0 so events written before the addendum still deserialize,
    /// where they mean "counts unknown, restore only".
    CommandCheckpointed {
        checkpoint_id: String,
        command: String,
        summary: String,
        #[serde(default)]
        restores: u32,
        #[serde(default)]
        removes: u32,
    },
    CommandCheckpointReverted {
        checkpoint_id: String,
    },

    WorktreeCleaned {
        tier: u8,
        freed_bytes: u64,
        #[serde(default, skip_serializing_if = "is_false")]
        branch_deleted: bool,
    },

    /// INVARIANT: A child thread spawned by `run_thread` / `run_coding_agent` finished its turn.
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
        summary: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_change_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sub_thread_pending_changes: Vec<super::SubThreadPendingChange>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        integrated_branch: Option<super::IntegratedBranch>,
    },

    ChildReportFiled {
        child_thread_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_thread_title: Option<String>,
        status: ChildCompletionStatus,
        summary: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_change_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sub_thread_pending_changes: Vec<super::SubThreadPendingChange>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        integrated_branch: Option<super::IntegratedBranch>,
    },

    ChildThreadStopped {
        child_thread_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_thread_title: Option<String>,
    },

    ChildThreadDetached {
        child_thread_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_thread_title: Option<String>,
    },

    ContextDismissed {
        dismissed_event_id: uuid::Uuid,
    },

    /// INVARIANT: The agent asked to set one tool result's clock back to zero, by writing
    /// its address under `[KEEP OPEN]` in its working understanding.
    ///
    /// It names a `ToolCalled` on the same thread, that being the one event
    /// whose body reaches the model as a `tool_result` block. The keep is
    /// applied where the span is parsed. So this event is the durable record
    /// the eval counts dispositions from, never the mechanism.
    ///
    /// A keep moves the clock and nothing else. It exempts the item from no
    /// pass, so the trimmer at the wall can always cut.
    ContextKeptOpen {
        kept_open_event_id: uuid::Uuid,
    },

    WorkingUnderstandingWritten {
        document: String,
    },

    ImageDescribed {
        source_event_id: uuid::Uuid,
        hash: String,
        description: String,
        model: String,
    },

    ConversationSummarized {
        summary: String,
        covers_through_event_id: uuid::Uuid,
        covered_count: u32,
        model: String,
    },

    EventWaitStarted {
        wait_id: uuid::Uuid,
        tool_use_id: String,
        on: Vec<EventSubscription>,
        reason: String,
        #[serde(default = "chrono::Utc::now")]
        armed_at: chrono::DateTime<chrono::Utc>,
        expires_at: chrono::DateTime<chrono::Utc>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        background_timing: Option<BackgroundWaitTiming>,
        /// INVARIANT: The event `sequence` at registration. Both the registration path and
        /// the boot rebuild scan forward from here. One mechanism therefore
        /// closes the restart gap and the live race between this emit and
        /// the cache insert.
        watermark: i64,
    },

    EventWaitTimingUpdated {
        wait_id: uuid::Uuid,
        reason: String,
        expires_at: chrono::DateTime<chrono::Utc>,
        background_timing: BackgroundWaitTiming,
    },

    EventWaitDelivered {
        wait_id: uuid::Uuid,
        event_id: uuid::Uuid,
        event_type: String,
        payload: Value,
        matched_index: usize,
    },

    /// INVARIANT: The wait passed its deadline. **Wakes the thread** with an explanatory
    /// message rather than dropping it: a silently dropped wait is a
    /// permanently stalled thread, which is strictly worse than the polling it
    /// replaces.
    EventWaitExpired {
        wait_id: uuid::Uuid,
    },

    /// INVARIANT: The subscription was stopped short of its own resolution. Note what is
    /// NOT a cause: neither an ordinary message into a subscribed thread nor a
    /// thread-level Stop disturbs a subscription, so neither a passing "how's
    /// it going?" nor stopping an unrelated turn can silently throw away a long
    /// watch.
    ///
    /// Carries what it stopped, so the transcript entry is self-contained on
    /// replay, exactly as `EventWaitDelivered` carries its matched event. A
    /// cancel is the one resolution with no wake, so it renders at its own
    /// position in the timeline, and the `EventWaitStarted` it resolves is
    /// routinely a day older and outside the loaded window. Absent on rows
    /// written before 2026-08-07, which fall back to the in-window lookup.
    EventWaitCanceled {
        wait_id: uuid::Uuid,
        cause: EventWaitCancelCause,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        on: Vec<EventSubscription>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
    },

    CacheKeepAliveStatusChanged {
        status: Option<CacheKeepAliveStatus>,
    },

    CacheKeepAliveTurnedOff,

    CacheKeepAliveTurnedOn,

    WakeExpected {
        at: chrono::DateTime<chrono::Utc>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        distribution: Vec<WakeBucket>,
        source: WakeSource,
        reason: String,
    },

    VoiceSessionStarted {
        session_id: uuid::Uuid,
    },

    /// INVARIANT: The voice session closed, and why.
    ///
    /// Every start reaches one of these. A killed engine cannot emit its own,
    /// so the boot sweep settles the survivors with
    /// [`VoiceSessionEndReason::EngineShutdown`].
    ///
    /// Carries no audio and no transcript. Audio is never persisted (parent
    /// plan, decision 12), and a spoken turn is its own event.
    VoiceSessionEnded {
        session_id: uuid::Uuid,
        reason: VoiceSessionEndReason,
        duration_secs: u64,
    },

    SpokenReplyGenerated {
        session_id: uuid::Uuid,
        text: String,
        interrupted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spoken_secs_before: Option<f64>,
    },

    SpokenMessageReceived {
        session_id: uuid::Uuid,
        text: String,
    },

    WorkDelegated {
        session_id: uuid::Uuid,
        reason: String,
    },

    SideQuestionAsked {
        side_question_id: uuid::Uuid,
        question: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_hashes: Vec<String>,
    },
    SideQuestionAnswered {
        side_question_id: uuid::Uuid,
        answer: String,
    },
    SideQuestionFailed {
        side_question_id: uuid::Uuid,
        error: String,
    },
    SideQuestionDismissed {
        side_question_id: uuid::Uuid,
    },
    WidgetShown {
        app_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<BTreeMap<String, Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    #[serde(alias = "WidgetRestored")]
    WidgetPinned {
        app_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<BTreeMap<String, Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shown_event_id: Option<uuid::Uuid>,
    },
    #[serde(alias = "WidgetHidden")]
    WidgetUnpinned {
        app_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<BTreeMap<String, Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    InlineWidgetSized {
        app_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<BTreeMap<String, Value>>,
        size: crate::core::apps::WidgetSize,
    },
    ThreadMcpServerEnabled {
        server_id: String,
        by: McpEnablementSource,
    },
    ThreadMcpServerDisabled {
        server_id: String,
        by: McpEnablementSource,
    },
    HomeBoardArranged {
        layout: Dashboard,
        tiles: Vec<LegacyArrangedTile>,
    },

    // INVARIANT: ---- Transient — never persisted ----
    // Every transient variant is past-tense (events-only model — no command
    // concept). Aliases preserve replay of any legacy persisted rows that
    // slipped in before the projection's transient match arm caught them.
    // Streaming state
    #[serde(alias = "TextStreaming")]
    CumulativeTextUpdated {
        text: String,
    },
    #[serde(alias = "Retrying")]
    LlmCallRetried {
        reason: String,
    },
    #[serde(alias = "PreambleCompleting")]
    PreambleCompleted,
    // INVARIANT: Request events: past-tense framing (a request was made; the frontend
    // chooses whether to act). A request the user must answer is a persisted
    // *form request* instead, above.
    #[serde(alias = "PushNotificationRequest")]
    PushNotificationRequested,
    #[serde(alias = "RefreshAppUI")]
    AppUiRefreshRequested {
        app_id: String,
    },
    #[serde(alias = "CaptureAppUI")]
    AppUiCaptureRequested {
        app_id: String,
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        save_format: Option<CaptureFormat>,
    },
    WidgetFitRequested {
        app_id: String,
        request_id: String,
        sizes: Vec<crate::core::apps::WidgetSize>,
        ui_scale: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale_device_id: Option<String>,
        /// INVARIANT: Public selection handle, never the renderer credential. Its actor
        /// is the verified source thread, rather than an impersonated Device.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        renderer_session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        renderer_device_id: Option<String>,
    },
    NavigationRequested {
        payload: String,
    },
    #[serde(alias = "CcThreadSpawned")]
    CodingAgentThreadSpawned {
        cc_thread_id: String,
        title: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
    },
    CodingAgentDiffChanged {
        has_diff: bool,
    },
    ChildrenCountChanged {
        active: i64,
        total: i64,
    },
}
