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
pub enum CodingAgent {
    ClaudeCode,
    Codex,
}

impl CodingAgent {
    /// Wire/DB string for this backend — same kebab-case values serde uses
    /// (`claude-code`, `codex`), so the `thread_summaries.coding_agent`
    /// column, event payloads, and HTTP request bodies all share one root.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }

    /// Parse a stored backend string. Unknown / legacy NULL-ish values fall
    /// back to `ClaudeCode` — every thread persisted before the column
    /// existed was a Claude Code thread.
    pub fn parse(s: &str) -> Self {
        match s {
            "codex" => Self::Codex,
            _ => Self::ClaudeCode,
        }
    }
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
    /// Streamed reasoning/thinking fragment — human-readable extended-thinking
    /// text the agent emitted before (or between) its visible output. CC sends it
    /// as a `stream_event` → `content_block_delta` with `delta.type:
    /// "thinking_delta"`; Codex sends it as `item/reasoning/{textDelta,
    /// summaryTextDelta}` (app-server) or a `reasoning` item (exec). Surfaced
    /// separately from `Message` so the consumer can emit
    /// `CodingAgentThoughtStreamed` and render a live "Thinking" step, instead of
    /// leaving a long reasoning pass as a silent "Working" gap. The *persisted* CC
    /// session JSONL keeps only the encrypted thinking signature, so this plaintext
    /// exists only on the live stream — capture it here or it is lost.
    Thought { text: String },
    /// Liveness ping — the agent emitted an intermediate streaming delta (CC's
    /// `stream_event` wrapper) that carries no content to persist: the complete
    /// text and tool calls arrive separately as `Message` / `ToolUse`. Its sole
    /// purpose is to prove the subprocess is alive and actively producing output
    /// so the watchdog's inactivity clock (`AgentSession::last_event_at`, bumped
    /// for every received event) can tell a long-but-active step (extended
    /// thinking, a large generation) apart from a genuinely hung process. Without
    /// it the clock only ticks at step boundaries (a completed message or tool
    /// result), and a single step longer than `WATCHDOG_INACTIVITY_LIMIT_MS` is
    /// killed mid-work. Consumers persist nothing for this variant. Backends
    /// whose deltas already arrive as `Message` events (Codex) never emit it.
    StreamActivity,
    /// Tool invocation. `id` is the agent's tool-use identifier — persisted on
    /// `UserQuestionAsked` so a reply can be matched back to its question.
    ToolUse {
        name: String,
        input: serde_json::Value,
        id: String,
    },
    /// Tool result returned to the agent. `id` matches the originating
    /// `ToolUse.id` so the engine can pair calls and results across event
    /// boundaries (e.g. a permission prompt that lands between them).
    /// Empty when the underlying CLI omits the id (legacy tool_result frames).
    ToolResult {
        output: String,
        status: String,
        id: String,
    },
    /// Turn-complete marker. The agent is now idle.
    /// `error` is `Some` when the agent reported the turn ended in failure
    /// (CC's `subtype: "error_during_execution"` etc., `is_error: true`) —
    /// the consumer emits `ResponseFailed` instead of `ResponseGenerated` so
    /// the partial response renders as a failed exchange, not a complete one.
    Result {
        text: String,
        duration_ms: u64,
        error: Option<String>,
    },
    /// Per-LLM-call token usage reported by the agent. CC emits one
    /// `message.usage` block per assistant message in its stream-json
    /// output; the engine forwards these as `Usage` events so a
    /// `ContextCaptured` can surface real input/output/cache counts in
    /// the StepDetailModal — same event the main-LLM agentic loop emits,
    /// just with `producer: ClaudeCode`. Cache fields are Anthropic-only
    /// and stay zero on agents that don't expose them.
    Usage {
        model: Option<String>,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_creation_tokens: u32,
    },
    /// Process exited. Always the last event before `events_rx` closes.
    /// Stderr is logged inside the runtime — consumers don't need to handle it.
    ///
    /// `killed_by_signal` is `true` when the agent process died from a signal
    /// the engine did NOT initiate — a stray `SIGTERM`/`SIGKILL` (e.g. the
    /// `exit=143` Node-caught-SIGTERM case) observed as the process's *natural*
    /// exit status, before any engine-side teardown kill. The safety net reads
    /// it to auto-resume an unexpectedly-killed mid-stream turn (via
    /// `ContinuationRequested`) instead of surfacing a red-dot abort. A clean
    /// exit, an EOF close, or an engine-initiated cancel all report `false`.
    Exited { killed_by_signal: bool },
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
    /// `LUCIDOS_EVENT_ID` so subprocess tooling (e.g. `lucidos spawn-thread`)
    /// can stamp `caller_event_id` on outbound cross-workspace POSTs.
    ///
    /// `None` ⇒ env var unset ⇒ `lucidos spawn-thread` omits `caller_event_id`
    /// from outbound POSTs. Use `None` for recovery, hardening, and other
    /// engine-internal spawns where no parent event exists; pass the
    /// originating event id otherwise.
    pub spawning_event_id: Option<Uuid>,
    /// Name of the repository this Claude Code session is running in (e.g.
    /// `"example-repo"`, `"Lucidos"`). Forwarded as `LUCIDOS_REPO` so
    /// `lucidos spawn-thread` defaults `--repo` to it — a CC sidequest stays
    /// in the same repo as its caller without the model having to thread the
    /// value through every invocation.
    ///
    /// `None` ⇒ env var unset ⇒ `lucidos spawn-thread` falls back to the
    /// target workspace's default repo. Always pass the resolved repo name
    /// when one is known; only legacy/early-startup callers should pass `None`.
    pub repo_name: Option<&'a str>,
    /// True when this spawn is an interactive session — chat, recovery, or
    /// external-repo work where the user is at the keyboard. False for
    /// unattended sessions (conflict-resolution) that run autonomously.
    ///
    /// Forwarded as `LUCIDOS_SESSION_KIND=interactive` so the
    /// `cc-stop-reminder` hook knows whether it can safely block CC with an
    /// AskUserQuestion redirect (which would hang an unattended session
    /// waiting for an answer that's not coming).
    pub interactive: bool,
    /// True when the engine resumes a mid-turn-interrupted session with NO
    /// new user input (the `ContinuationRequested` recovery path) and expects
    /// the agent to pick up where it left off on its own.
    ///
    /// Claude Code ignores this: `claude --print --resume` auto-injects
    /// "Continue from where you left off." before reading stdin. Runtimes
    /// whose CLI only acts on an explicit prompt (Codex) use it to start the
    /// first turn with an equivalent continuation prompt instead of waiting
    /// for an input that will never arrive.
    pub continuation: bool,
    /// User-managed non-secret environment variables (`(NAME, value)` pairs from
    /// `EnvironmentVariableStore::env_pairs`). Applied FIRST in
    /// `runtime::spawn_env::apply_lucidos_env`, before every engine-owned var, so
    /// the engine always wins a collision (e.g. a user `LUCIDOS_REPO` is
    /// overridden by the spawn's repo context). Fetched by the spawn
    /// orchestration (which has the pool); empty for callers that don't inject
    /// (tests, engine-internal spawns with no user env).
    pub user_env_vars: &'a [(String, String)],
    /// Pinned Claude Code config dir for a RESUME — the `CLAUDE_CONFIG_DIR` the
    /// thread's session was created under. A resumed session must look up its
    /// transcript under the same dir it was written in
    /// (`$CLAUDE_CONFIG_DIR/projects/<cwd>/<sid>.jsonl`), so `build_command`
    /// sets this as an engine-owned override AFTER `apply_lucidos_env`'s user-env
    /// loop — a live user toggle of `CLAUDE_CONFIG_DIR` then can't strand the
    /// resume (dev/bf997e21). `None` only for a thread's FIRST turn (no pin yet —
    /// leaves the user's live env / CC's default untouched, which establishes the
    /// pin) and for backends that don't key transcripts on a config dir (Codex
    /// ignores it); every later spawn of a pinned thread re-injects the pin. See
    /// `lookup_pinned_cc_config_dir`.
    pub claude_config_dir: Option<&'a str>,
    /// User-configured absolute path to this agent's CLI binary — the
    /// `coding_agent_claude_path` / `coding_agent_codex_path` preference for
    /// the backend being spawned, resolved by the spawn orchestration.
    ///
    /// `Some` wins over every probe/PATH lookup, and a path that doesn't
    /// resolve to an executable file FAILS the spawn with an error naming the
    /// preference — a typo must surface, never silently fall back (see
    /// `spawn_env::resolve_binary_override`). `None` (unset — every install
    /// before the preference existed) keeps the probe → PATH auto-detection.
    pub binary_override: Option<&'a str>,
}

/// An in-band permission request raised by the agent's own protocol — the
/// Codex app-server's `item/commandExecution/requestApproval` /
/// `item/fileChange/requestApproval` JSON-RPC requests. The engine's run
/// loop consumes these from `RunningAgent::permission_rx`, drives the same
/// `CodingAgentPermissionRequest` → PermissionCard → broadcast machinery the
/// CC MCP HTTP path uses, and answers via `respond` (`true` = accept,
/// `false` = decline); the driver then replies to the JSON-RPC request.
///
/// Claude Code does NOT use this channel — its permission prompts arrive
/// out-of-band over HTTP (`lucidos mcp-permission-server` →
/// `/api/v1/internal/permission-prompt`), so its `RunningAgent` carries
/// `permission_rx: None`.
#[derive(Debug)]
pub struct AgentPermissionRequest {
    /// The agent's identifier for the tool call being approved (the Codex
    /// item id). Becomes `CodingAgentPermissionRequest.tool_use_id`.
    pub id: String,
    /// Backend-shaped tool name (`command_execution` / `file_change`) —
    /// same vocabulary the backend's `CodingAgentToolCalled` events use.
    pub tool_name: String,
    /// Tool input for the card's summary line + dedup key (e.g.
    /// `{"command": "...", "cwd": "..."}`).
    pub input: serde_json::Value,
    /// One-shot answer channel. Dropping the receiver (driver death) tells
    /// the engine-side waiter to abandon the prompt.
    pub respond: tokio::sync::oneshot::Sender<bool>,
}

/// A spawned agent. The runtime owns the child process and an internal driver
/// task; the engine consumes from `events_rx` and produces on the senders.
///
/// Lifecycle: cancellation is signalled by the `CancellationToken` passed to
/// `spawn`. When cancelled, the driver kills the child, drains stderr, sends
/// `AgentEvent::Exited`, and closes `events_rx`. EOF on `events_rx` is also
/// the canonical "process gone" signal for natural exits.
pub struct RunningAgent {
    pub kind: CodingAgent,
    pub events_rx: mpsc::UnboundedReceiver<AgentEvent>,
    pub input_tx: mpsc::UnboundedSender<AgentInput>,
    pub control_tx: mpsc::UnboundedSender<ControlRequest>,
    /// In-band permission requests (see [`AgentPermissionRequest`]). `None`
    /// for backends whose permissions flow out-of-band (Claude Code's MCP
    /// HTTP path, the Codex exec driver's sandbox-only model).
    pub permission_rx: Option<mpsc::UnboundedReceiver<AgentPermissionRequest>>,
}

#[async_trait]
pub trait AgentRuntime: Send + Sync {
    fn kind(&self) -> CodingAgent;

    async fn spawn(
        &self,
        args: SpawnArgs<'_>,
        cancel: CancellationToken,
    ) -> Result<RunningAgent, Box<dyn std::error::Error + Send + Sync>>;
}

#[cfg(test)]
mod tests {
    use super::CodingAgent;

    #[test]
    fn coding_agent_as_str_matches_serde_wire_values() {
        // The DB column, event payloads, and HTTP bodies must share one root —
        // as_str() and serde's kebab-case rename are the same contract.
        for agent in [CodingAgent::ClaudeCode, CodingAgent::Codex] {
            let wire = serde_json::to_value(agent).unwrap();
            assert_eq!(wire, serde_json::Value::String(agent.as_str().to_string()));
            assert_eq!(CodingAgent::parse(agent.as_str()), agent);
        }
    }

    #[test]
    fn coding_agent_parse_defaults_unknown_to_claude_code() {
        // Legacy NULL/garbage rows predate the column — all were Claude Code.
        assert_eq!(CodingAgent::parse(""), CodingAgent::ClaudeCode);
        assert_eq!(CodingAgent::parse("forgecode"), CodingAgent::ClaudeCode);
    }
}
