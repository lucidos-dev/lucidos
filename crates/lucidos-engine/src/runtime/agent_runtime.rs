//! Pluggable coding-agent runtime layer.
//!
//! `AgentRuntime` wraps a CLI coding agent (Claude Code, Codex, …) behind a
//! channel-based interface. Each implementor parses its CLI's stdout into the
//! canonical `AgentEvent` enum, accepts user inputs on `AgentInput` and
//! control requests on `ControlRequest`, and watches a `CancellationToken`
//! to know when to kill the process.
//!
//! Engine code that drives a coding-agent session uses only this trait; the
//! agent-specific JSON formats and process management stay inside the
//! `runtime::*` modules.

use async_trait::async_trait;
use std::path::Path;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Identifier for the coding-agent backend. The engine maps each kind to a
/// concrete `AgentRuntime` implementation in its agent registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    ClaudeCode,
    Codex,
}

/// Canonical event emitted by a coding agent. Implementors translate their
/// CLI's output format into this enum before sending it over `events_rx`.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Initial handshake — session id, model, available commands.
    Init {
        session_id: String,
        model: Option<String>,
        slash_commands: Vec<String>,
        skills: Vec<String>,
    },
    /// Streamed assistant text fragment.
    Message { role: String, text: String },
    /// Tool invocation. `id` is the agent's tool-use identifier — persisted on
    /// `UserQuestionAsked` so a reply can be matched back to its question.
    ToolUse {
        name: String,
        input: serde_json::Value,
        id: String,
    },
    /// Tool result returned to the agent.
    ToolResult { output: String, status: String },
    /// Turn-complete marker. The agent is now idle.
    Result { text: String, duration_ms: u64 },
    /// Process exited. Always the last event before `events_rx` closes.
    /// Stderr is logged inside the runtime — consumers don't need to handle it.
    Exited,
}

/// User input sent to a running agent.
#[derive(Debug, Clone)]
pub struct AgentInput {
    pub text: String,
    pub images: Vec<crate::api::ChatImage>,
}

/// Runtime control request — set parameters or interrupt the current turn.
/// Implementors translate to their CLI's protocol; unsupported variants may
/// be no-ops on a given backend.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum ControlRequest {
    Interrupt,
    SetModel { model: String },
    SetPermissionMode { mode: String },
    SetReasoningEffort { effort: String },
}

/// Parameters for spawning an agent. Borrowed for the duration of `spawn`.
#[derive(Clone)]
pub struct SpawnArgs<'a> {
    pub worktree_path: &'a Path,
    /// Forwarded as `LUCIDOS_WORKSPACE` so subprocess tooling (e.g. the
    /// `lucidos` CLI) can resolve back to the right engine.
    pub workspace_path: &'a Path,
    pub allowed_tools: Option<&'a str>,
    pub system_prompt: Option<&'a str>,
    pub resume_session_id: Option<&'a str>,
    pub model: Option<&'a str>,
    pub reasoning_effort: Option<&'a str>,
    pub thread_id: Uuid,
    /// Event in the *parent* thread that triggered this spawn. Forwarded as
    /// `LUCIDOS_EVENT_ID` so subprocess tooling (e.g. `lucidos send-thread`)
    /// can stamp `caller_event_id` on outbound cross-workspace POSTs.
    ///
    /// `None` ⇒ env var unset ⇒ `lucidos send-thread` omits `caller_event_id`
    /// from outbound POSTs. Use `None` for recovery, hardening, and other
    /// engine-internal spawns where no parent event exists; pass the
    /// originating event id otherwise.
    pub spawning_event_id: Option<Uuid>,
}

/// A spawned agent. The runtime owns the child process and an internal driver
/// task; the engine consumes from `events_rx` and produces on the senders.
///
/// Lifecycle: cancellation is signalled by the `CancellationToken` passed to
/// `spawn`. When cancelled, the driver kills the child, drains stderr, sends
/// `AgentEvent::Exited`, and closes `events_rx`. EOF on `events_rx` is also
/// the canonical "process gone" signal for natural exits.
pub struct RunningAgent {
    pub kind: AgentKind,
    pub events_rx: mpsc::UnboundedReceiver<AgentEvent>,
    pub input_tx: mpsc::UnboundedSender<AgentInput>,
    pub control_tx: mpsc::UnboundedSender<ControlRequest>,
}

#[async_trait]
pub trait AgentRuntime: Send + Sync {
    fn kind(&self) -> AgentKind;

    async fn spawn(
        &self,
        args: SpawnArgs<'_>,
        cancel: CancellationToken,
    ) -> Result<RunningAgent, Box<dyn std::error::Error + Send + Sync>>;
}
