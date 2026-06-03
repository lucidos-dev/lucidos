use crate::core::Step;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Re-exported so callers can use the bare names through `engine::*`
// (chained via `engine/mod.rs::pub use types::*`). Engine code references
// `ConversationSnapshot` etc. directly without the `core::` path.
pub use crate::core::{ConversationMessage, ConversationSnapshot, SessionMessage};

pub use crate::api::AppContext;

#[derive(Clone, Debug, Serialize)]
pub struct ProcessResult {
    /// Response text in raw markdown (frontend converts to HTML)
    pub response: String,
    pub steps: Vec<Step>,
    /// Paths to images created during this response (relative to artifacts/)
    pub images: Vec<String>,
    /// The request ID this result belongs to
    #[serde(skip)]
    pub request_id: Uuid,
    /// The thread ID this result belongs to (groups related request/response exchanges)
    #[serde(skip)]
    pub thread_id: Uuid,
    /// Whether a pending change was proposed during this request (triggers SSE broadcast)
    #[serde(skip)]
    pub proposed_change: bool,
    /// Whether the user requested auto-apply before the session ended
    #[serde(skip)]
    pub auto_apply: bool,
    /// Injections orphaned by the loop-exit / guard-drop race — caller
    /// must re-submit them as follow-up messages.
    #[serde(skip)]
    pub orphaned_injections: Vec<OrphanedInjection>,
}

/// A follow-up message that was injected into a thread after the agentic loop
/// finished but before the ThreadGuard dropped — the race window where the
/// thread appears active but nobody reads the injection channel.
pub type OrphanedInjection = super::InjectedPrompt;

/// `content` is `Option` so the modal can render section *shape* (name +
/// `char_count`) without persisting the body when the `capture_context`
/// preference is off. Old DB rows always have content and deserialize as
/// `Some(_)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextSection {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub char_count: usize,
    /// API role this section is sent under. Defaults to "user" for backward
    /// compat with persisted ContextCaptured events from before this change —
    /// every previous section was actually part of either the system prompt
    /// (System Instructions) or the user message; the system row gets
    /// migrated by name in the viewer fallback.
    #[serde(default = "default_context_role")]
    pub role: ContextRole,
    /// Inner-group label used by the viewer to nest sections within the
    /// user-message role. None for system-role sections, prior-message rows,
    /// and legacy events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// API role bucket a `ContextSection` belongs to. Mirrors the three buckets
/// in the LLM API call: the system prompt, prior messages (verbatim resume
/// tool blocks), and this turn's user message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRole {
    System,
    PriorMessage,
    User,
}

fn default_context_role() -> ContextRole {
    ContextRole::User
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_section_legacy_payload_deserializes_with_default_role_and_no_group() {
        // Pre-change ContextCaptured events have no `role` or `group` fields.
        // They must still deserialize cleanly: `role` defaults to `User`, and
        // `group` stays `None`.
        let json = r#"{"name":"X","char_count":42}"#;
        let section: ContextSection =
            serde_json::from_str(json).expect("legacy payload must deserialize");
        assert_eq!(section.name, "X");
        assert_eq!(section.char_count, 42);
        assert!(section.content.is_none());
        assert_eq!(section.role, ContextRole::User);
        assert!(section.group.is_none());
    }
}

/// CC can't expose its system prompt body or tool schemas via the
/// stream-json envelope, only their token cost — the frontend uses this
/// discriminant to know whether to expect a section breakdown body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextProducer {
    MainLlm,
    ClaudeCode,
}

/// `cache_*_tokens` are Anthropic-only (zero elsewhere). `output_tokens`
/// may be zero on a snapshot emitted mid-stream before the final delta.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ApiUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
}

/// Result of an App UI capture from the frontend
pub struct CaptureResult {
    pub screenshot: String, // base64-encoded image (JPEG from html2canvas; format sniffed downstream)
    pub dom: String,        // condensed DOM text
}

/// A user message sent to a running Claude Code session, with optional image attachments.
/// `origin_event_id` is the UUID of the already-emitted `MessageReceived` (or other
/// exchange-starter like `ChildThreadCompleted`) event — used to re-process lost
/// follow-ups without emitting a duplicate exchange boundary.
pub struct AgentUserInput {
    pub text: String,
    pub images: Option<Vec<crate::api::ChatImage>>,
    /// UUID of the exchange-starter event already emitted before routing
    /// (`MessageReceived` for user follow-ups; `ChildThreadCompleted` for
    /// child-wake follow-ups). `None` for auto-harden and other internally
    /// generated messages where no caller-side event exists.
    pub origin_event_id: Option<uuid::Uuid>,
    /// What kind of input this is. CC's `run_session` reads this to decide
    /// whether to emit `CodingAgentPromptSent` — `User` does, `WakeFromChild`
    /// doesn't (the `ChildThreadCompleted` event already on the parent's
    /// history is the exchange-starter; emitting another start event would
    /// split the response into a duplicate exchange).
    pub kind: AgentInputKind,
}

/// Discriminates a user-typed follow-up from an engine-synthesized child-wake.
/// CC's `run_session` and the chat fast-paths use this to suppress duplicate
/// exchange-starter events for wakes (`ChildThreadCompleted` is the start).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentInputKind {
    /// User-typed follow-up. Emit `CodingAgentPromptSent` as the audit trail.
    User,
    /// Engine-synthesized wake from a completed child thread. Suppress
    /// `CodingAgentPromptSent` so the response groups under
    /// `ChildThreadCompleted` (the real exchange-starter).
    WakeFromChild,
}

/// Cached CC slash commands (builtin + skill).
/// Serializable so it can be cached to `.lucidos/cc-commands.json` and survive engine restarts.
/// Model/effort are NOT cached here — they are per-thread via CodingAgentSettingsChanged events.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CcCommandsInfo {
    pub builtin_commands: Vec<String>,
    pub skill_commands: Vec<String>,
}

/// Response from cc_categorized_commands / cc_commands_for_repo.
/// Commands come from the repo-level cache; model/effort come from per-thread events.
pub struct CcCommandsResult {
    pub info: CcCommandsInfo,
    pub has_active_session: bool,
    pub current_model: Option<String>,
    pub current_reasoning_effort: Option<String>,
}

/// Why `stop_agent` fired the stop signal. Read by the run_session loop's
/// stop arm to decide whether to emit `ResponseCanceled` (only `UserStop`
/// does — the others have their own lifecycle terminator), and by the
/// post-loop cleanup to drive Apply / Discard side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// User clicked the Cancel/Stop button on an in-flight CC turn. Emits
    /// `ResponseCanceled(UserStop)` if CC was actively working; nothing if
    /// CC had already gone idle (race window — previous turn already done).
    UserStop,
    /// User clicked Apply Now — the resulting change auto-applies after
    /// cleanup. Post-loop reads this to set `ProcessResult.auto_apply=true`.
    /// `ChangeApplied` is the terminator; no `ResponseCanceled`.
    Apply,
    /// User clicked Discard — branch is deleted instead of proposed.
    /// `ChangeDiscarded` is the terminator; no `ResponseCanceled`.
    Discard,
    /// User clicked Archive — `ThreadArchived` is the terminator; no
    /// `ResponseCanceled`.
    Archive,
}

/// State for a single active coding-agent session.
///
/// **Single agent per thread.** The owning HashMap is keyed by `Uuid` (thread_id)
/// alone, so a thread can only host one live agent backend at a time. When a
/// second backend (Codex) is wired in, this constraint must be relaxed —
/// re-key as `(Uuid, CodingAgent)` and store `agent_kind: CodingAgent` on this
/// struct so callers can disambiguate.
pub struct AgentSession {
    pub msg_tx: tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    pub is_waiting: bool,
    pub has_changes: bool,
    pub requires_restart: bool,
    /// Reason the most recent `stop_agent` call fired, or `None` when the
    /// session has never been stopped (or the stop signal came from the
    /// engine-shutdown direct-notify path, which sets `shutting_down` instead).
    /// Set by `stop_agent` BEFORE `stop.notify_one()`; read by the run_session
    /// stop arm to decide whether to suppress `ResponseCanceled`, and by the
    /// post-loop cleanup to drive `Apply` / `Discard` side effects. Stays
    /// mutually exclusive by construction — three `bool` fields here used to
    /// drift apart and the `auto_apply || discard || archiving` shape was a
    /// repeated source of bugs.
    pub pending_stop: Option<StopReason>,
    /// Generic stop signal for the run_session loop. Fired by `stop_agent` for
    /// every user-driven termination (Cancel, Apply, Discard, Archive) and by
    /// the engine shutdown timeout. The stop arm reads `pending_stop` and
    /// `shutting_down` to decide what (if anything) to emit — only a real
    /// `UserStop` on an actively-working CC produces `ResponseCanceled`.
    pub stop: std::sync::Arc<tokio::sync::Notify>,
    /// Interrupt signal — sends control_request:interrupt to stop current work
    /// without killing the session (like pressing Esc in Claude Code terminal).
    pub interrupt: std::sync::Arc<tokio::sync::Notify>,
    /// Notified when the CC process enters idle/waiting state.
    /// Used by `apply_now` to wait for review/conflict resolution to complete.
    pub idle_notify: std::sync::Arc<tokio::sync::Notify>,
    /// When true, an `apply_now` task is already running for this thread.
    /// Prevents concurrent apply_now calls from causing duplicate merges.
    pub apply_now_in_progress: bool,
    /// Set to true when the CC process exits. Checked by `apply_now_inner`
    /// after waking from `idle_notify` to detect CC death vs normal idle.
    pub process_exited: bool,
    /// Path to the worktree this Claude Code session is working in.
    pub worktree_path: Option<std::path::PathBuf>,
    /// Branch name this Claude Code session is working on.
    pub branch_name: Option<String>,
    /// Root of the repo (for git operations during apply).
    pub repo_root: Option<std::path::PathBuf>,
    /// Claude Code's session ID (from the "system" init event).
    /// Used for `--resume` on follow-ups and engine restart.
    pub cc_session_id: Option<String>,
    /// Epoch millis of the last event received from this Claude Code session.
    /// Used by `apply_now` for liveness-based timeout instead of fixed wall-clock.
    pub last_event_at: std::sync::Arc<std::sync::atomic::AtomicI64>,
    /// Set during engine shutdown so CC cleanup emits SessionEnded with
    /// reason SessionEndReason::Shutdown — the frontend uses this to show "Aborted"
    /// instead of "Done" for engine-interrupted exchanges.
    pub shutting_down: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Set by `abort_in_flight_for_restart` when it pre-emits a `ResponseAborted`
    /// on `/api/v1/restart` for this thread. The run_session loop's Result-classify
    /// and safety-net paths read this flag and skip their own terminal emit so
    /// the user only sees ONE "Response interrupted" panel (with the device
    /// actor) instead of two (one device, one system). Without this flag, CC's
    /// graceful interrupt → Result event → classify_result(is_shutdown=true)
    /// path emits a duplicate `ResponseAborted` 3s after the pre-emit.
    pub external_terminal_emitted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Channel for sending control requests (set_model, set_permission_mode, etc.)
    /// from outside the event loop. The event loop forwards them to the runtime.
    pub control_tx: tokio::sync::mpsc::UnboundedSender<crate::runtime::ControlRequest>,
    /// Built-in CC commands (compact, clear, cost, etc.) — slash_commands minus skills.
    pub builtin_commands: Vec<String>,
    /// Skill commands (from plugins and user .claude/skills/).
    pub skill_commands: Vec<String>,
    /// Current model reported by CC (from the system init event's `model` field).
    pub current_model: Option<String>,
    /// Current reasoning effort level (low/medium/high).
    /// Not reported in CC's init event — only set via control request.
    pub current_reasoning_effort: Option<String>,
    /// Number of user-input messages routed to CC since the last `Result`.
    /// Incremented when a message is queued for CC (initial spawn input +
    /// each fast-path follow-up via msg_tx); reset to zero by `swap(0)`
    /// inside the Result handler because every Result is a turn boundary
    /// (CC merges back-to-back stdin inputs into a single Result, so a 1:1
    /// decrement leaks the counter). The pre-swap value drives only the
    /// idle-exit cancel: > 1 means a follow-up was inflight (queued in
    /// msg_rx OR forwarded but merged) so CC stays alive to avoid killing
    /// the subprocess between a routed follow-up and its consumption by
    /// `msg_rx.recv`. Without this guard, a follow-up routed via msg_tx
    /// during the cancel + follow-up race would get killed mid-second-task
    /// because the previous turn's Result triggers `agent_cancel.cancel()`
    /// while CC is still processing the new prompt.
    pub pending_followups: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// Paired ToolCalled / ToolResult counter, mirrored from the local
    /// counter inside `run_session`. Read by the external watchdog
    /// (`external_watchdog::tick`) — which lives outside `run_session`'s
    /// `select!` and cannot see local variables. The in-loop watchdog still
    /// reads the local copy; both observe the same `Arc<AtomicI32>` so the
    /// value can't drift. > 0 means a tool is mid-execution (Bash, Read,
    /// AskUserQuestion, …) — legitimate silence, no fire from either
    /// watchdog.
    pub tools_in_flight: std::sync::Arc<std::sync::atomic::AtomicI32>,
}

impl AgentSession {
    /// Snapshot current slash commands for cache persistence.
    pub fn to_commands_info(&self) -> CcCommandsInfo {
        CcCommandsInfo {
            builtin_commands: self.builtin_commands.clone(),
            skill_commands: self.skill_commands.clone(),
        }
    }

    /// True when the session has an in-flight response: process alive and
    /// not waiting at a turn boundary.
    pub fn is_in_flight(&self) -> bool {
        !self.process_exited && !self.is_waiting
    }
}

/// Outcome class of an apply attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    /// Branch merged into main (or external-repo handoff completed).
    Applied,
    /// Branch had nothing to merge — already applied, or no commits exist.
    /// Distinct from `Applied` so callers don't infer a fresh merge happened.
    Noop,
    /// Hardening recovery session was spawned. The change will auto-apply
    /// after hardening completes — `review_thread_id` points at it.
    Hardening,
    /// Merge conflict — a Claude Code session was spawned (`conflict_thread_id`)
    /// or an in-place merge failed and the original session stays alive.
    Conflict,
}

impl Default for ApplyStatus {
    /// `Noop` is the only safe "no-info" default. Defaulting to `Applied`
    /// would let a forgotten field silently claim a successful merge —
    /// the very ambiguity this struct exists to prevent.
    fn default() -> Self {
        Self::Noop
    }
}

/// Result of applying a pending change. Serialized directly as the HTTP
/// response body — see `docs/apply-change-api.md`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ApplyResult {
    pub status: ApplyStatus,
    pub change_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub restart_required: bool,
    pub message: String,
    /// Set on `Conflict` — frontend should focus this thread to resolve.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_thread_id: Option<Uuid>,
    /// Set on `Hardening` — frontend should track as "applying" until done.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_thread_id: Option<Uuid>,
    /// SHA of main HEAD after the merge — absent when no real merge happened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_commit: Option<String>,
    pub commits_applied: usize,
    pub files_changed: usize,
}

fn applied_message(restart_required: bool) -> String {
    if restart_required {
        "Change applied. This change requires the engine to restart.".to_string()
    } else {
        "Change applied.".to_string()
    }
}

impl ApplyResult {
    pub fn applied(change_id: Uuid, thread_id: Option<Uuid>, restart_required: bool) -> Self {
        Self {
            status: ApplyStatus::Applied,
            change_id,
            thread_id,
            restart_required,
            message: applied_message(restart_required),
            ..Self::default()
        }
    }

    pub fn applied_with_merge(
        change_id: Uuid,
        thread_id: Option<Uuid>,
        restart_required: bool,
        previous_commit: String,
        applied_commit: String,
        commits: &[String],
        files_changed: usize,
    ) -> Self {
        Self {
            status: ApplyStatus::Applied,
            change_id,
            thread_id,
            restart_required,
            message: applied_message(restart_required),
            applied_commit: Some(applied_commit),
            previous_commit: Some(previous_commit),
            commits_applied: commits.len(),
            files_changed,
            ..Self::default()
        }
    }

    pub fn noop(
        change_id: Uuid,
        thread_id: Option<Uuid>,
        files_changed: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status: ApplyStatus::Noop,
            change_id,
            thread_id,
            message: message.into(),
            files_changed,
            ..Self::default()
        }
    }

    pub fn hardening(
        change_id: Uuid,
        review_thread_id: Uuid,
        files_changed: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status: ApplyStatus::Hardening,
            change_id,
            thread_id: Some(review_thread_id),
            message: message.into(),
            review_thread_id: Some(review_thread_id),
            files_changed,
            ..Self::default()
        }
    }

    pub fn conflict(
        change_id: Uuid,
        conflict_thread_id: Uuid,
        files_changed: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status: ApplyStatus::Conflict,
            change_id,
            thread_id: Some(conflict_thread_id),
            message: message.into(),
            conflict_thread_id: Some(conflict_thread_id),
            files_changed,
            ..Self::default()
        }
    }
}
