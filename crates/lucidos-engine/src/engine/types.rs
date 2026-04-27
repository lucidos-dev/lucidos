use crate::core::Step;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Re-export types from core::store for backwards compatibility
pub use crate::core::{ConversationMessage, ConversationSnapshot, SessionMessage};

/// App context sent when an app UI is open — tells the LLM which app is active.
#[derive(Debug, Clone, Deserialize)]
pub struct AppContext {
    pub app_id: String,
}

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
    /// Injections that arrived via inject_prompt() after the agentic loop exited
    /// but before the ThreadGuard dropped. Must be re-submitted as follow-up
    /// messages by the caller.
    #[serde(skip)]
    pub orphaned_injections: Vec<OrphanedInjection>,
}

/// A follow-up message that was injected into a thread after the agentic loop
/// finished but before the ThreadGuard dropped — the race window where the
/// thread appears active but nobody reads the injection channel.
pub type OrphanedInjection = super::InjectedPrompt;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextSection {
    pub name: String,
    pub content: String,
    pub char_count: usize,
}

/// A condensed view of a message in the agentic loop, for the context inspector.
#[derive(Clone, Debug, Serialize)]
pub struct ContextMessage {
    pub role: String,
    pub text: String,
    pub tool_calls: Vec<ContextToolCall>,
    pub tool_results: Vec<ContextToolResult>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContextToolCall {
    pub name: String,
    pub input_summary: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContextToolResult {
    pub tool_name: String,
    pub content_preview: String,
    pub success: bool,
}

/// Result of an App UI capture from the frontend
pub struct CaptureResult {
    pub screenshot: String, // base64 PNG
    pub dom: String,        // condensed DOM text
}

/// A user message sent to a running Claude Code session, with optional image attachments.
/// `origin_event_id` is the UUID of the already-emitted `MessageReceived` event — used
/// to re-process lost follow-ups without emitting a duplicate exchange boundary.
pub struct AgentUserInput {
    pub text: String,
    pub images: Option<Vec<crate::api::ChatImage>>,
    /// UUID of the MessageReceived event already emitted by chat.rs before routing.
    /// Set for user follow-ups routed via the fast-path; None for auto-harden and
    /// other internally-generated messages.
    pub origin_event_id: Option<uuid::Uuid>,
}

/// Cached CC slash commands (builtin + skill).
/// Serializable so it can be cached to `.lucidos/cc-commands.json` and survive engine restarts.
/// Model/effort are NOT cached here — they are per-thread via CodingAgentSettingsChanged events.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CcCommandsInfo {
    pub builtin_commands: Vec<String>,
    pub skill_commands: Vec<String>,
}

/// Response from cc_categorized_commands / cc_cached_commands.
/// Commands come from the repo-level cache; model/effort come from per-thread events.
pub struct CcCommandsResult {
    pub info: CcCommandsInfo,
    pub has_active_session: bool,
    pub current_model: Option<String>,
    pub current_reasoning_effort: Option<String>,
}

/// State for a single active coding-agent session.
///
/// **Single agent per thread.** The owning HashMap is keyed by `Uuid` (thread_id)
/// alone, so a thread can only host one live agent backend at a time. When a
/// second backend (Codex) is wired in, this constraint must be relaxed —
/// re-key as `(Uuid, AgentKind)` and store `agent_kind: AgentKind` on this
/// struct so callers can disambiguate.
pub struct AgentSession {
    pub msg_tx: tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    pub is_waiting: bool,
    pub has_changes: bool,
    pub requires_restart: bool,
    pub auto_apply: bool,
    /// When true, changes are discarded (branch deleted) instead of proposed.
    pub discard: bool,
    pub cancel: std::sync::Arc<tokio::sync::Notify>,
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
    /// Path to the worktree this CC session is working in.
    pub worktree_path: Option<std::path::PathBuf>,
    /// Branch name this CC session is working on.
    pub branch_name: Option<String>,
    /// Root of the repo (for git operations during apply).
    pub repo_root: Option<std::path::PathBuf>,
    /// Claude Code's session ID (from the "system" init event).
    /// Used for `--resume` on follow-ups and engine restart.
    pub cc_session_id: Option<String>,
    /// Epoch millis of the last event received from this CC session.
    /// Used by `apply_now` for liveness-based timeout instead of fixed wall-clock.
    pub last_event_at: std::sync::Arc<std::sync::atomic::AtomicI64>,
    /// Set during engine shutdown so CC cleanup emits SessionEnded with
    /// reason SessionEndReason::Shutdown — the frontend uses this to show "Aborted"
    /// instead of "Done" for engine-interrupted exchanges.
    pub shutting_down: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
}

impl AgentSession {
    /// Snapshot current slash commands for cache persistence.
    pub fn to_commands_info(&self) -> CcCommandsInfo {
        CcCommandsInfo {
            builtin_commands: self.builtin_commands.clone(),
            skill_commands: self.skill_commands.clone(),
        }
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
    /// Merge conflict — a CC session was spawned (`conflict_thread_id`)
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
