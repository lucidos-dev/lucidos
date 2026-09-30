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
use std::path::{Path, PathBuf};
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
        /// The agent CLI's own version, when its handshake names one.
        agent_version: Option<String>,
    },
    /// Streamed assistant text fragment. `opens_block` is true when this text
    /// starts a new content block rather than continuing the open one. The
    /// consumer starts a new paragraph there, so two blocks never run together.
    Message {
        role: String,
        text: String,
        opens_block: bool,
    },
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
    /// The agent took the oldest forwarded inputs it had not taken yet.
    ///
    /// Claude Code reports it by replaying the input (`--replay-user-messages`).
    /// The replay comes mid-turn when it folds the input in at a tool result,
    /// and at the next turn's start otherwise. Inputs queued behind a busy turn
    /// share one replay, which carries what they said. Codex reports one input
    /// when the driver starts the turn that carries it, and carries `None`.
    InputRead(Option<ReplayedInput>),
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

/// What one Claude Code replay carried, which may be several inputs.
///
/// Claude Code joins queued plain-text inputs into one text with newlines. Once
/// an image is among them it keeps every input's blocks apart instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayedInput {
    /// The text blocks, in order.
    pub texts: Vec<String>,
    pub images: usize,
}

/// User input sent to a running agent.
#[derive(Debug, Clone)]
pub struct AgentInput {
    pub text: String,
    pub images: Vec<crate::api::ChatImage>,
    /// The name the agent knows this write by, which a withdraw uses. Fresh
    /// for every write: Claude Code skips a user message whose uuid it has
    /// already seen, so reusing an event id would drop a resent message.
    pub uuid: uuid::Uuid,
}

/// A request to take back a user input the agent has not read yet.
#[derive(Debug)]
pub struct WithdrawRequest {
    /// [`AgentInput::uuid`] of the input to take back.
    pub input_uuid: uuid::Uuid,
    /// A dropped `reply` means the process ended before it answered.
    pub reply: tokio::sync::oneshot::Sender<InputWithdrawal>,
}

/// How a withdraw ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputWithdrawal {
    /// The agent dropped the input. It will never run.
    Withdrawn,
    /// The agent read the input into a turn, so it can no longer be taken back.
    AlreadyRead,
    /// The withdraw could not be made. The text tells the user why.
    Refused(String),
}

/// Runtime control request — set parameters or interrupt the current turn.
/// Implementors translate to their CLI's protocol; unsupported variants may
/// be no-ops on a given backend.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum ControlRequest {
    Interrupt,
    SetModel { model: String },
    SetReasoningEffort { effort: String },
}

/// The Claude Code profile a thread is bound to: its *account pin*.
///
/// Claude Code treats an unset `CLAUDE_CONFIG_DIR` as a different profile from
/// one set to its default path. The two read different keychain entries and
/// different `.claude.json` files, so only one of them holds the user's login.
/// A pinned spawn therefore replays exactly what turn 1 ran with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountPin {
    /// `CLAUDE_CONFIG_DIR` was unset. `dir` is Claude Code's default,
    /// `$HOME/.claude`, where the transcripts live.
    DefaultConfigDir { dir: String },
    /// `CLAUDE_CONFIG_DIR` was set to `dir`.
    ExplicitConfigDir { dir: String },
}

impl AccountPin {
    /// The pin a thread's first session establishes: the live
    /// `CLAUDE_CONFIG_DIR` when set, else Claude Code's default dir. `None`
    /// only when there is neither, which means `$HOME` is unset.
    pub fn for_first_session(live: Option<String>, default_dir: Option<String>) -> Option<Self> {
        match live {
            Some(dir) => Some(Self::ExplicitConfigDir { dir }),
            None => default_dir.map(|dir| Self::DefaultConfigDir { dir }),
        }
    }

    /// Resolve a recorded pin.
    ///
    /// `explicit` is `None` on rows written before the engine recorded it.
    /// Those rows stored `default_dir` for an unset variable too, so that one
    /// path is ambiguous. It counts as explicit only while the live env still
    /// sets exactly that path. Any other path can only have come from the
    /// variable.
    pub fn from_recorded(
        dir: String,
        explicit: Option<bool>,
        live: Option<&str>,
        default_dir: Option<&str>,
    ) -> Self {
        let explicit = explicit
            .unwrap_or_else(|| Some(dir.as_str()) != default_dir || live == Some(dir.as_str()));
        if explicit {
            Self::ExplicitConfigDir { dir }
        } else {
            Self::DefaultConfigDir { dir }
        }
    }

    /// Where Claude Code keeps this profile's transcripts.
    pub fn dir(&self) -> &str {
        match self {
            Self::DefaultConfigDir { dir } | Self::ExplicitConfigDir { dir } => dir,
        }
    }

    pub fn is_explicit(&self) -> bool {
        matches!(self, Self::ExplicitConfigDir { .. })
    }
}

/// Parameters for spawning an agent. Borrowed for the duration of `spawn`.
#[derive(Clone)]
pub struct SpawnArgs<'a> {
    pub worktree_path: &'a Path,
    /// Which repo the session edits. Decides who owns the lucidos-cli skill
    /// file (see `lucidos_cli::place_lucidos_cli_skill`). Codex ignores it.
    pub coding_agent_kind: crate::engine::agent_session::CodingAgentKind,
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
    /// User-managed non-secret environment variables (`(NAME, value)` pairs from
    /// `EnvironmentVariableStore::env_pairs`). Applied FIRST in
    /// `runtime::spawn_env::apply_lucidos_env`, before every engine-owned var, so
    /// the engine always wins a collision (e.g. a user `LUCIDOS_REPO` is
    /// overridden by the spawn's repo context). Fetched by the spawn
    /// orchestration (which has the pool); empty for callers that don't inject
    /// (tests, engine-internal spawns with no user env).
    pub user_env_vars: &'a [(String, String)],
    /// The thread's *account pin*, replayed on every spawn after turn 1.
    /// `build_command` applies it AFTER `apply_lucidos_env`. So neither a live
    /// user toggle nor the engine's own env can move the thread to another
    /// profile or strand its transcript. `None` on a thread's first turn, which
    /// leaves the env untouched and so establishes the pin. Codex ignores it.
    /// See `lookup_pinned_cc_config_dir`.
    pub account_pin: Option<&'a AccountPin>,
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
    /// The `coding_agent_claude_permission_mode` preference, verbatim, resolved
    /// by `claude_code::resolve_permission_mode` at the point of use.
    ///
    /// Claude Code only. Its CLI `--permission-mode` outranks every settings
    /// file, so a user cannot pick a mode any other way. `None`, and anything
    /// the resolver does not recognise, means `acceptEdits`: the mode every
    /// session ran before the preference existed. Codex has no equivalent and
    /// ignores this field, as it ignores `account_pin`.
    pub permission_mode: Option<&'a str>,
    /// Directories the repo's own Claude Code settings grant outside the repo,
    /// resolved against the main checkout by
    /// `engine::repo_directory_grants::resolve`. Each becomes one
    /// `--add-dir`. Claude Code only: Codex does not read those settings.
    pub additional_directories: &'a [PathBuf],
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
    /// Withdraws (see [`WithdrawRequest`]). `None` for backends that cannot
    /// take back an input: both Codex drivers.
    pub withdraw_tx: Option<mpsc::UnboundedSender<WithdrawRequest>>,
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
    use super::{AccountPin, CodingAgent};

    const HOME_CLAUDE: &str = "/home/u/.claude";

    fn default_pin(dir: &str) -> AccountPin {
        AccountPin::DefaultConfigDir {
            dir: dir.to_string(),
        }
    }

    fn explicit_pin(dir: &str) -> AccountPin {
        AccountPin::ExplicitConfigDir {
            dir: dir.to_string(),
        }
    }

    #[test]
    fn a_first_session_pins_what_it_ran_with() {
        assert_eq!(
            AccountPin::for_first_session(None, Some(HOME_CLAUDE.into())),
            Some(default_pin(HOME_CLAUDE)),
            "an unset CLAUDE_CONFIG_DIR is the default profile, not an explicit ~/.claude"
        );
        assert_eq!(
            AccountPin::for_first_session(Some(HOME_CLAUDE.into()), Some(HOME_CLAUDE.into())),
            Some(explicit_pin(HOME_CLAUDE)),
            "a variable set to the default path is still a set variable"
        );
        assert_eq!(AccountPin::for_first_session(None, None), None);
    }

    #[test]
    fn a_recorded_marker_is_final() {
        let live = Some("/home/u/.claude-personal");
        assert_eq!(
            AccountPin::from_recorded(HOME_CLAUDE.into(), Some(false), live, Some(HOME_CLAUDE)),
            default_pin(HOME_CLAUDE)
        );
        assert_eq!(
            AccountPin::from_recorded(HOME_CLAUDE.into(), Some(true), None, Some(HOME_CLAUDE)),
            explicit_pin(HOME_CLAUDE)
        );
    }

    /// Rows written before the marker stored `$HOME/.claude` for an unset
    /// variable. Reading them as explicit is the "Please run /login" bug.
    #[test]
    fn a_legacy_default_path_is_the_default_profile_unless_the_live_env_sets_it() {
        for (live, expected) in [
            (None, default_pin(HOME_CLAUDE)),
            (Some("/home/u/.claude-personal"), default_pin(HOME_CLAUDE)),
            (Some(HOME_CLAUDE), explicit_pin(HOME_CLAUDE)),
        ] {
            assert_eq!(
                AccountPin::from_recorded(HOME_CLAUDE.into(), None, live, Some(HOME_CLAUDE)),
                expected,
                "live CLAUDE_CONFIG_DIR = {live:?}"
            );
        }
    }

    #[test]
    fn a_legacy_non_default_path_was_always_explicit() {
        let dir = "/home/u/.claude-personal";
        assert_eq!(
            AccountPin::from_recorded(dir.into(), None, None, Some(HOME_CLAUDE)),
            explicit_pin(dir)
        );
        assert_eq!(
            AccountPin::from_recorded(dir.into(), None, None, None),
            explicit_pin(dir),
            "with no $HOME there is no default path to confuse it with"
        );
    }

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
