use async_trait::async_trait;
use std::path::{Path, PathBuf};
// One signal-name table for the whole engine — shared with `TaskOutcome`, which
// renders the same names for the bash tools. See `format_exit_status`.
use crate::core::shell::signal_name;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::agent_runtime::{
    AgentEvent, AgentInput, AgentRuntime, CodingAgent, ControlRequest, RunningAgent, SpawnArgs,
};
use super::lucidos_cli::{
    ensure_workspace_bin_symlink, install_lucidos_cli_skill, lucidos_cli_dir, LUCIDOS_BIN_NAME,
    LUCIDOS_CLI_SKILL_REL_PATH,
};
use super::spawn_env::{apply_lucidos_env, drain_stderr};

/// Single source of truth for CC's `/model` and `/effort` picker entries.
/// The data lives in `cc_menu_options.json` next to this file: see that file's
/// `_note` for why it is hand-maintained and how to update it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CcMenuOption {
    pub value: String,
    pub label: String,
    pub description: String,
    /// Optional model compatibility metadata used by Codex reasoning efforts.
    /// Claude Code and universally supported Codex options leave it absent.
    #[serde(default)]
    pub supported_models: Option<Vec<String>>,
    /// The window a session on this model actually runs under, in tokens.
    ///
    /// Absent on almost every row, because it is an OVERRIDE of what
    /// `llm::model_registry::context_window_for` infers from the id. Declare it
    /// only where the backend's window differs, and read it through
    /// [`crate::runtime::coding_agent_context_window`].
    ///
    /// The two answers differ because they describe different requests. The
    /// registry describes the one LUCIDOS makes, where 1M mode is gated on our
    /// own `[1m]` suffix. A coding agent makes its own request and picks its
    /// own context mode, so nothing here bounds a prompt the engine packs.
    #[serde(default)]
    pub context_window: Option<usize>,
}

#[derive(Debug, serde::Deserialize)]
struct CcMenuOptionsFile {
    models: Vec<CcMenuOption>,
    reasoning_efforts: Vec<CcMenuOption>,
}

const CC_MENU_OPTIONS_JSON: &str = include_str!("cc_menu_options.json");

static CC_MENU_OPTIONS: std::sync::LazyLock<CcMenuOptionsFile> = std::sync::LazyLock::new(|| {
    serde_json::from_str(CC_MENU_OPTIONS_JSON)
        .expect("cc_menu_options.json is malformed — see runtime/cc_menu_options.json")
});

pub fn cc_model_options() -> &'static [CcMenuOption] {
    &CC_MENU_OPTIONS.models
}

pub fn cc_reasoning_effort_options() -> &'static [CcMenuOption] {
    &CC_MENU_OPTIONS.reasoning_efforts
}

/// Map a full CC CLI model ID (e.g. `claude-sonnet-4-6`) back to the short
/// alias we originally sent (e.g. `sonnet`).  Returns the input unchanged if
/// it already is a known `cc_model_options()` value or doesn't match any alias.
pub fn normalize_cc_model_id(full_id: &str) -> &str {
    if cc_model_options().iter().any(|m| m.value == full_id) {
        return full_id;
    }
    if full_id.starts_with("claude-sonnet-4") {
        return "sonnet";
    }
    if full_id == "claude-opus-4-6" || full_id.starts_with("claude-opus-4-6-") {
        return "opus";
    }
    if full_id.starts_with("claude-haiku-4") {
        return "haiku";
    }
    full_id
}

/// Reconcile CC's stream-json model name with the engine-supplied alias.
/// CC strips the `[1m]` suffix when echoing the model in Init and Usage frames.
/// A naive `normalize_cc_model_id` therefore loses the 1M-context signal that
/// `context_window_for` keys on. Re-attach the suffix when the engine pinned
/// `[1m]` and CC reports the same base model.
pub fn reconcile_cc_model(original: Option<&str>, cc_reported: &str) -> String {
    let normalized = normalize_cc_model_id(cc_reported);
    if let Some(orig_base) = original.and_then(|o| o.strip_suffix("[1m]")) {
        if orig_base == normalized || orig_base == cc_reported {
            return format!("{}[1m]", normalized);
        }
    }
    normalized.to_string()
}

#[path = "claude_code_parse.rs"]
mod parse;
pub use parse::{parse_line, CcStreamState};

/// The reasoning tiers `model` is offered, read out of the effort rows.
///
/// An effort with no `supported_models` is universal; one that names models is
/// offered only to those. The `default` row matches nothing by name, so it
/// takes the universal set. That is the right answer for a model the backend
/// has not resolved yet.
pub(super) fn efforts_for_model(model: &str, efforts: &[CcMenuOption]) -> Vec<String> {
    efforts
        .iter()
        .filter(|e| {
            e.supported_models
                .as_ref()
                .is_none_or(|allowed| allowed.iter().any(|m| m == model))
        })
        .map(|e| e.value.clone())
        .collect()
}

/// Render one backend's `set_model` and `set_reasoning_effort` option lists.
///
/// The JSON files keep the hand-maintained effort-to-models shape, because that
/// is how upstream announces a tier: one line naming the models that accept it.
/// The wire carries the transpose, `reasoning_efforts` per MODEL row, matching
/// what `GET /api/v1/models` serves for the Lucidos Agent. One picker rule then
/// covers both surfaces: ask the model row what it offers.
pub(super) fn model_and_effort_options(
    models: &[CcMenuOption],
    efforts: &[CcMenuOption],
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let model_options = models
        .iter()
        .map(|m| {
            serde_json::json!({
                "value": m.value,
                "label": m.label,
                "description": m.description,
                "reasoning_efforts": efforts_for_model(&m.value, efforts),
            })
        })
        .collect();
    // No `supported_models` on the wire: the model rows now carry the same
    // information in the shape the picker reads.
    let effort_options = efforts
        .iter()
        .map(|e| {
            serde_json::json!({
                "value": e.value,
                "label": e.label,
                "description": e.description,
            })
        })
        .collect();
    (model_options, effort_options)
}

/// Render the menu of supported control commands for the frontend's `/model`
/// picker. CC-specific — Codex and other agents have their own menus.
pub fn cc_command_definitions() -> serde_json::Value {
    let (model_options, effort_options) =
        model_and_effort_options(cc_model_options(), cc_reasoning_effort_options());
    serde_json::json!([
        {
            "subtype": "set_model",
            "label": "Model",
            "params": [{ "key": "model", "label": "Model", "options": model_options }]
        },
        {
            "subtype": "set_reasoning_effort",
            "label": "Reasoning Effort",
            "params": [{ "key": "effort", "label": "Reasoning Effort", "options": effort_options }]
        },
        {
            "subtype": "set_permission_mode",
            "label": "Permission Mode",
            "params": [{ "key": "mode", "label": "Mode", "placeholder": "plan" }]
        }
    ])
}

/// Serialize a `ControlRequest` to the JSON line that CC expects on stdin.
pub fn cc_control_request_to_json(request: &ControlRequest, request_id: &str) -> String {
    let body = match request {
        ControlRequest::Interrupt => serde_json::json!({ "subtype": "interrupt" }),
        ControlRequest::SetModel { model } => {
            serde_json::json!({ "subtype": "set_model", "model": model })
        }
        ControlRequest::SetPermissionMode { mode } => {
            serde_json::json!({ "subtype": "set_permission_mode", "mode": mode })
        }
        ControlRequest::SetReasoningEffort { effort } => {
            serde_json::json!({ "subtype": "set_reasoning_effort", "effort": effort })
        }
    };
    serde_json::to_string(&serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": body,
    }))
    .expect("ControlRequest serialization cannot fail")
}

/// Cached default reasoning effort from CC settings. Read once per process.
static CC_DEFAULT_EFFORT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

fn is_valid_effort(value: &str) -> bool {
    cc_reasoning_effort_options()
        .iter()
        .any(|m| m.value == value)
}

fn effort_from_json(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    let v = parsed.get("effortLevel")?.as_str()?.to_lowercase();
    if is_valid_effort(&v) {
        Some(v)
    } else {
        None
    }
}

/// Read the effective reasoning effort from Claude Code's configuration.
/// Precedence (highest first): env var > local project > project > user local > user global.
/// Cached after first call.
pub fn read_cc_default_effort() -> Option<String> {
    CC_DEFAULT_EFFORT
        .get_or_init(|| {
            if let Ok(val) = std::env::var("CLAUDE_CODE_EFFORT_LEVEL") {
                let v = val.to_lowercase();
                if is_valid_effort(&v) {
                    return Some(v);
                }
            }
            // CC settings precedence: local project > project > user local > user global
            let project_files = [".claude/settings.local.json", ".claude/settings.json"];
            for f in &project_files {
                if let Some(v) = effort_from_json(std::path::Path::new(f)) {
                    return Some(v);
                }
            }
            if let Ok(home) = std::env::var("HOME") {
                let home = std::path::Path::new(&home);
                let user_files = [
                    home.join(".claude/settings.local.json"),
                    home.join(".claude/settings.json"),
                ];
                for f in &user_files {
                    if let Some(v) = effort_from_json(f) {
                        return Some(v);
                    }
                }
            }
            None
        })
        .clone()
}

/// `AgentRuntime` implementation backed by the `claude` CLI.
pub struct ClaudeCodeRuntime;

#[async_trait]
impl AgentRuntime for ClaudeCodeRuntime {
    fn kind(&self) -> CodingAgent {
        CodingAgent::ClaudeCode
    }

    async fn spawn(
        &self,
        args: SpawnArgs<'_>,
        cancel: CancellationToken,
    ) -> Result<RunningAgent, Box<dyn std::error::Error + Send + Sync>> {
        let cli_dir = lucidos_cli_dir();
        // Resolve the permission-prompt MCP server's binary up front so a
        // missing `lucidos` CLI fails the spawn rather than surfacing as
        // "Available MCP tools: none" mid-stream. See `resolve_lucidos_binary_in`.
        resolve_lucidos_binary(cli_dir)?;
        // A user-configured `claude` path must point at a real executable, so
        // fail the spawn naming the setting rather than probing past a typo.
        // See `spawn_env::resolve_binary_override`.
        if let Some(path) = args.binary_override {
            super::spawn_env::resolve_binary_override(
                path,
                "Claude Code (`claude`)",
                "coding_agent_claude_path",
            )?;
        }
        if let Err(e) = install_lucidos_cli_skill(args.worktree_path, cli_dir) {
            crate::log!(
                "[ClaudeCode] failed to install lucidos-cli skill into {}: {}",
                args.worktree_path.display(),
                e
            );
        }
        // A *tracked* copy of the injected skill turns the overwrite above into
        // a phantom `M` that `.git/info/exclude` cannot hide. Skip-worktree it
        // so this session never sees a change it did not author. No-op for the
        // Lucidos repo, where the tracked copy is identical and stays editable.
        crate::engine::git_ops::hide_phantom_tracked_skill(
            args.worktree_path,
            LUCIDOS_CLI_SKILL_REL_PATH,
        )
        .await;
        ensure_workspace_bin_symlink(args.worktree_path, cli_dir);

        // Materialize the PreToolUse hook config CC reads via --settings.
        // Log-and-continue on failure: a missing hook degrades AskUserQuestion
        // behavior but doesn't break the rest of the session.
        let cc_settings_path =
            crate::engine::cc_settings::cc_settings_path_for_workspace(args.workspace_path);
        if let Err(e) = crate::engine::cc_settings::write_cc_settings(&cc_settings_path).await {
            crate::log!(
                "[ClaudeCode] failed to write cc-settings.json at {}: {} — AskUserQuestion hook will not fire",
                cc_settings_path.display(),
                e
            );
        }

        let mut cmd = build_command(&args, cli_dir);
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().ok_or("Failed to capture stdin")?;
        let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
        let stderr = child.stderr.take().ok_or("Failed to capture stderr")?;

        let (events_tx, events_rx) = mpsc::unbounded_channel::<AgentEvent>();
        let (input_tx, input_rx) = mpsc::unbounded_channel::<AgentInput>();
        let (control_tx, control_rx) = mpsc::unbounded_channel::<ControlRequest>();

        let initial_session_id = args.resume_session_id.map(str::to_string);
        tokio::spawn(driver_task(
            child,
            stdin,
            BufReader::new(stdout),
            BufReader::new(stderr),
            events_tx,
            input_rx,
            control_rx,
            cancel,
            initial_session_id,
        ));

        Ok(RunningAgent {
            kind: CodingAgent::ClaudeCode,
            events_rx,
            input_tx,
            control_tx,
            // CC permissions flow out-of-band: its MCP permission-prompt
            // subprocess POSTs /api/v1/internal/permission-prompt directly.
            permission_rx: None,
        })
    }
}

/// Resolve the `claude` executable for spawn. The user-configured override wins
/// outright (the spawn path already validated it, see
/// `spawn_env::resolve_binary_override`), then the common install locations,
/// then a bare PATH lookup.
///
/// Probing is needed because the CC native installer symlinks
/// `$HOME/.local/bin/claude`, and an engine launched by launchd or an IDE
/// inherits a PATH without `~/.local/bin`. A bare `Command::new("claude")` then
/// ENOENTs even though the binary is installed. The probe list mirrors
/// `resolve_codex_binary`: native installer, the older `~/.claude/local`
/// install, then the Homebrew prefixes. Bare `"claude"` is last, so
/// `Command::spawn` does its own PATH lookup for npm globals and custom
/// symlinks. `home` is injected to keep the function pure and testable.
pub(crate) fn resolve_claude_binary(
    home: Option<&Path>,
    override_path: Option<&Path>,
) -> std::ffi::OsString {
    if let Some(p) = override_path {
        return p.as_os_str().to_os_string();
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = home {
        candidates.push(home.join(".local/bin/claude"));
        candidates.push(home.join(".claude/local/claude"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/claude"));
    candidates.push(PathBuf::from("/usr/local/bin/claude"));
    for c in candidates {
        if c.exists() {
            return c.into_os_string();
        }
    }
    std::ffi::OsString::from("claude")
}

/// Resolve the absolute path to the `lucidos` CLI binary that backs the CC
/// permission-prompt MCP server (`lucidos mcp-permission-server`).
///
/// Prefer the bundled binary next to the engine (`cli_dir`, found by
/// `find_lucidos_cli_dir`), else a `PATH` lookup. Returns a descriptive `Err`
/// when neither has it, so the spawn fails at once instead of surfacing as a
/// silent "Available MCP tools: none" mid-stream abort. The most common cause
/// is a packaged build that bundles `lucidos-engine` but forgets the sibling
/// `lucidos` CLI.
///
/// `path_env` is injected to keep the lookup pure and testable.
fn resolve_lucidos_binary_in(
    cli_dir: Option<&Path>,
    path_env: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(dir) = cli_dir {
        let candidate = dir.join(LUCIDOS_BIN_NAME);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    if let Some(found) =
        super::spawn_env::find_on_path(std::ffi::OsStr::new(LUCIDOS_BIN_NAME), path_env)
    {
        return Ok(found);
    }
    Err(format!(
        "the bundled `{bin}` CLI (required for the Claude Code permission-prompt \
         MCP server) was not found next to the engine binary nor on PATH — a \
         packaged build must ship `{bin}` alongside `lucidos-engine`",
        bin = LUCIDOS_BIN_NAME
    )
    .into())
}

fn resolve_lucidos_binary(
    cli_dir: Option<&Path>,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    resolve_lucidos_binary_in(cli_dir, std::env::var_os("PATH").as_deref())
}

/// Byte-idle deadline we hand Claude Code for its own streaming watchdog, in
/// milliseconds. 30 minutes, the maximum CC accepts (it clamps
/// `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS` to `[10_000, 1_800_000]`).
///
/// CC aborts a turn when no bytes arrive on the SSE body for its deadline,
/// which defaults to 300_000 ms. It does not recover: no non-streaming
/// fallback, and at most one retry. A large cache-cold prompt can be silent
/// on the wire for longer than that.
///
/// The point is to push CC's deadline PAST the engine's own silence detectors
/// (`agent_session::lifecycle::WATCHDOG_INACTIVITY_LIMIT_MS` and
/// `agent_session::external_watchdog::EXTERNAL_WATCHDOG_LIMIT_MS`), whose
/// response is a non-destructive kill plus auto-resume. The shorter deadline
/// decides the outcome, so CC's must be the outer one. Disabling CC's
/// watchdog outright would remove the backstop it was shipped to be.
///
/// Temporary measure, see `docs/temporary-measures.md`
/// ("CC byte-idle deadline raised past the engine watchdog") and
/// `docs/investigations/2026-08-02-cc-stream-idle-timeout.md`.
const CC_BYTE_STREAM_IDLE_TIMEOUT_MS: u64 = 30 * 60 * 1000;

/// Build the `claude` Command with all flags and env vars. Extracted so unit
/// tests can inspect args/env without spawning. `cli_dir` is the directory
/// containing the `lucidos` binary, prepended to PATH; pass `None` to skip.
fn build_command(args: &SpawnArgs<'_>, cli_dir: Option<&Path>) -> tokio::process::Command {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let claude_bin = resolve_claude_binary(home.as_deref(), args.binary_override.map(Path::new));
    let mut cmd = tokio::process::Command::new(claude_bin);
    if let Some(sid) = args.resume_session_id {
        cmd.arg("--print").arg("--resume").arg(sid);
    }
    cmd.arg("--input-format")
        .arg("stream-json")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        // acceptEdits keeps in-cwd writes auto-approved (matching the previous
        // --dangerously-skip-permissions behavior); out-of-cwd writes and
        // Bash route through --permission-prompt-tool to a PermissionCard.
        .arg("--permission-mode")
        .arg("acceptEdits")
        .arg("--permission-prompt-tool")
        .arg(CC_PERMISSION_PROMPT_TOOL)
        .arg("--mcp-config")
        .arg(permission_mcp_config_json(cli_dir))
        .arg("--strict-mcp-config")
        .arg("--settings")
        .arg(crate::engine::cc_settings::cc_settings_path_for_workspace(
            args.workspace_path,
        ))
        .current_dir(args.worktree_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env_remove("CLAUDECODE");
    // Always request partial-message streaming, on fresh AND resumed sessions.
    // The `stream_event` deltas become `AgentEvent::StreamActivity` liveness
    // pings that keep the watchdog's inactivity clock fresh through one long
    // step. Omit it on `--resume` and the heartbeat ticks only at step
    // boundaries, so the watchdog kills long unattended steps mid-work. The
    // flag is a streaming-output option, orthogonal to `--resume`.
    cmd.arg("--include-partial-messages");

    if let Some(tools) = args.allowed_tools {
        cmd.arg("--allowedTools").arg(tools);
    }
    if let Some(m) = args.model {
        cmd.arg("--model").arg(m);
    }
    if let Some(effort) = args.reasoning_effort {
        cmd.env("CLAUDE_CODE_EFFORT_LEVEL", effort);
    }
    if let Some(prompt) = args.system_prompt {
        cmd.arg("--append-system-prompt").arg(prompt);
    }
    // Push CC's own byte-idle streaming deadline out past the engine's
    // inactivity watchdog, so a provider stall auto-resumes instead of killing
    // the turn. See `CC_BYTE_STREAM_IDLE_TIMEOUT_MS` for the full reasoning.
    //
    // Set BEFORE `apply_lucidos_env`, which is load-bearing: that helper applies
    // the user's workspace env vars FIRST and lets engine-owned vars win a
    // collision, so anything written after it is unoverridable. This one is a
    // tunable default, not a contract, so it goes before and a workspace env var
    // of the same name still wins.
    cmd.env(
        "CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS",
        CC_BYTE_STREAM_IDLE_TIMEOUT_MS.to_string(),
    );
    // Agent-independent Lucidos env contract (workspace, host protection,
    // PG*, subprocess origin, spawn metadata, RUSTC_WRAPPER, PATH) — shared
    // with every other AgentRuntime via `spawn_env::apply_lucidos_env`.
    apply_lucidos_env(&mut cmd, args, cli_dir, "ClaudeCode");
    // Pin the session's CLAUDE_CONFIG_DIR on a RESUME. CC stores each session's
    // transcript at `$CLAUDE_CONFIG_DIR/projects/<escaped-cwd>/<sid>.jsonl`, so
    // a `--resume <sid>` MUST run under the config dir the session was created
    // in. Otherwise CC returns "No conversation found with session ID".
    // Set AFTER `apply_lucidos_env`, which applied any user-managed
    // `CLAUDE_CONFIG_DIR` first, so this engine-owned pin wins: a live toggle of
    // the env var cannot strand an in-flight thread's resume. `None` for a fresh
    // session leaves the user's value or CC's default in place.
    if let Some(dir) = args.claude_config_dir {
        cmd.env("CLAUDE_CONFIG_DIR", dir);
    }
    // Root-cause fix for the stray-SIGTERM truncation bug: give CC its OWN
    // process group so a group-wide signal to the engine never reaches it.
    // The engine ignores SIGTERM but CC's Node runtime does not (exit=143).
    // See `spawn_env::isolate_in_process_group`.
    crate::runtime::spawn_env::isolate_in_process_group(&mut cmd);
    // The engine permission handler waits indefinitely for the user, matching
    // `AskUserQuestion`. CC has TWO separate MCP timeouts that both have to be
    // lifted, otherwise whichever is shorter forces a retry that surfaces a
    // duplicate prompt:
    //   * `MCP_TOOL_TIMEOUT`, the per-tool-call cap, defaulting to about 28h.
    //   * `MCP_TIMEOUT`, the per-RPC cap, defaulting to 30s.
    // The 30-second `MCP_TIMEOUT` default is what produced an infinite loop of
    // identical permission cards: CC's MCP client cancelled the permission RPC,
    // the engine gc'd the orphaned waiter, and CC's model retried the original
    // tool. Both are set to 24 hours.
    cmd.env("MCP_TOOL_TIMEOUT", (86_400u64 * 1000).to_string());
    cmd.env("MCP_TIMEOUT", (86_400u64 * 1000).to_string());
    // No macOS TCC responsibility disclaim is attempted here, and adding one
    // back would be inert: a `pre_exec` hook forces the `fork()` path, where the
    // only effective knob is never consulted. See ADR 0075.
    cmd
}

/// MCP server name Claude Code mounts `lucidos mcp-permission-server` under
/// (the `mcpServers` key in [`permission_mcp_config_json`]). CC prefixes every
/// MCP tool with `mcp__<server>__`, so this is also the first half of both wire
/// names below. Codex mounts the SAME binary under the name `lucidos`, so its
/// question tool ([`super::CODEX_ASK_USER_QUESTION_TOOL`]) has a different wire
/// name for the same server-side tool.
const CC_PERMISSION_MCP_SERVER: &str = "lucidos_perm";

/// The tool CC is pointed at with `--permission-prompt-tool`: every gated tool
/// call arrives here and is forwarded to `/api/v1/internal/permission-prompt`.
pub const CC_PERMISSION_PROMPT_TOOL: &str = "mcp__lucidos_perm__approve";

/// The question tool as CC sees it. Same server-side `ask_user_question` Codex
/// calls, under CC's mount name, so it is a THIRD wire name for one flow.
///
/// It is reachable because CC's `--mcp-config` advertises every tool the server
/// lists, and the server lists both `approve` and `ask_user_question`. So a CC
/// session can raise a QuestionCard through here rather than through its native
/// `AskUserQuestion`. Everything keyed on the name has to know that:
/// [`super::is_user_question_tool`] is the one place that decides.
pub const CC_MCP_ASK_USER_QUESTION_TOOL: &str = "mcp__lucidos_perm__ask_user_question";

/// Claude Code's OWN built-in question tool, intercepted by the PreToolUse hook
/// in `crate::engine::cc_settings` rather than routed over MCP. Not prefixed,
/// because it is not an MCP tool.
pub const CC_NATIVE_ASK_USER_QUESTION_TOOL: &str = "AskUserQuestion";

/// Build the `--mcp-config` JSON for the lucidos permission server. CC spawns
/// `lucidos mcp-permission-server` over stdio; the server reads
/// `LUCIDOS_THREAD_ID` and `LUCIDOS_WORKSPACE` from the inherited env.
///
/// `--permission-only` narrows the server to its `approve` tool. The same
/// binary also serves Codex's `ask_user_question`, and CC has no per-server
/// tool filter. Without the flag, that tool lands in CC's list as a duplicate
/// of its native `AskUserQuestion`. Calling it then raises a permission card,
/// because CC routes every MCP tool through `--permission-prompt-tool`.
///
/// Asking the user a question is not a permission-worthy act: the tool does not
/// belong to this backend. See `mcp_permission_server::ToolSet` for the other
/// half of the split.
///
/// `command` is the ABSOLUTE path to the resolved `lucidos` binary, not the
/// bare name. The MCP server must not depend on the engine's modified `PATH`
/// surviving the spawn chain through `claude` into the server. `spawn()` has
/// already `?`-checked the same resolution, so the bare-name fallback here only
/// keeps this builder infallible.
fn permission_mcp_config_json(cli_dir: Option<&Path>) -> String {
    let command = resolve_lucidos_binary(cli_dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| LUCIDOS_BIN_NAME.to_string());
    serde_json::json!({
        "mcpServers": {
            CC_PERMISSION_MCP_SERVER: {
                "command": command,
                "args": ["mcp-permission-server", "--permission-only"]
            }
        }
    })
    .to_string()
}

/// Format a user input as the JSON line CC expects on stdin. `session_id` is
/// the Claude Code session id (from the latest Init event, or the resumed id).
fn format_user_input(input: &AgentInput, session_id: Option<&str>) -> String {
    let content = if input.images.is_empty() {
        serde_json::Value::String(input.text.clone())
    } else {
        let mut blocks = Vec::new();
        if !input.text.is_empty() {
            blocks.push(serde_json::json!({
                "type": "text",
                "text": input.text,
            }));
        }
        for img in &input.images {
            blocks.push(serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": img.mime_type,
                    "data": img.base64,
                },
            }));
        }
        serde_json::Value::Array(blocks)
    };
    let msg = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": content,
        },
        "session_id": session_id.unwrap_or("default"),
        "parent_tool_use_id": null,
    });
    let mut line = serde_json::to_string(&msg).expect("user input serializes");
    line.push('\n');
    line
}

/// True when an exit status indicates the process was killed by a signal.
/// Either the kernel delivered it (`status.signal()` is set), or the child
/// followed the Node.js `128 + signum` convention after handling it.
/// Distinguishes a stray external kill (auto-resumable) from a clean exit.
/// Mirrors the case analysis in `format_exit_status`, and lives next to it so
/// the two stay in lockstep.
#[cfg(unix)]
pub(crate) fn exit_indicates_signal_kill(status: &std::process::ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    if status.signal().is_some() {
        return true;
    }
    matches!(status.code(), Some(code) if (129..=159).contains(&code))
}

#[cfg(not(unix))]
pub(crate) fn exit_indicates_signal_kill(_status: &std::process::ExitStatus) -> bool {
    false
}

/// Decode a `child.wait()` result into a debuggable string. `{:?}` on
/// `ExitStatus` prints `unix_wait_status(36608)`, useless when a
/// session-ending CC death lands in production logs. Three cases:
///
/// * `exit=N`: a clean exit with code `N` below the signal-convention range.
/// * `exit=N (probable SIGNAME)`: the Node.js convention, where a child with a
///   signal handler re-exits `128 + signum` after cleanup. The hint reads the
///   cryptic 143 and 137 codes back as SIGTERM and SIGKILL at log-read time.
/// * `signal=NAME (N)` / `signal=N`: the kernel delivered the signal as
///   cause-of-death, so the child never got to clean up.
///
/// `signal_name` falls through to bare numbers for anything unmapped, so the
/// log never silently drops information.
pub(crate) fn format_exit_status(
    wait_result: &std::io::Result<std::process::ExitStatus>,
) -> String {
    match wait_result {
        Err(e) => format!("wait_err: {e}"),
        Ok(status) => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = status.signal() {
                    return match signal_name(sig) {
                        Some(name) => format!("signal={name} ({sig})"),
                        None => format!("signal={sig}"),
                    };
                }
                if let Some(code) = status.code() {
                    if (129..=159).contains(&code) {
                        if let Some(name) = signal_name(code - 128) {
                            return format!("exit={code} (probable {name})");
                        }
                    }
                    return format!("exit={code}");
                }
                "no_status".to_string()
            }
            #[cfg(not(unix))]
            {
                match status.code() {
                    Some(code) => format!("exit={code}"),
                    None => "no_status".to_string(),
                }
            }
        }
    }
}

/// Grace a cancelled CC process group gets to tear itself down (SIGTERM) before
/// the engine force-kills it (SIGKILL). Sized for a Playwright runner to close
/// the browsers it tracks: those `setsid`-escape the group, so only the runner's
/// own teardown reaps them and a bare SIGKILL leaves them orphaned. Runs in the
/// detached `driver_task`, off the cancel UX path, so the wait costs no
/// interactive latency. See `spawn_env::graceful_kill_child_process_group`.
const GROUP_TEARDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Drive the CC process: forward stdout → events_tx, input/control → stdin,
/// and react to cancellation. Always emits `AgentEvent::Exited` (best-effort)
/// before returning so consumers can distinguish a clean close from a panic.
#[allow(clippy::too_many_arguments)]
async fn driver_task(
    mut child: Child,
    mut stdin: ChildStdin,
    mut stdout_reader: BufReader<ChildStdout>,
    mut stderr_reader: BufReader<ChildStderr>,
    events_tx: mpsc::UnboundedSender<AgentEvent>,
    mut input_rx: mpsc::UnboundedReceiver<AgentInput>,
    mut control_rx: mpsc::UnboundedReceiver<ControlRequest>,
    cancel: CancellationToken,
    mut session_id: Option<String>,
) {
    // Capture the child pid before the wait arm consumes the Child:
    // `tokio::process::Child::id()` returns `None` once `wait()` resolves, and
    // the diagnostic log line below is the one place that needs it.
    let child_pid = child.id();
    // The process's NATURAL exit status, captured only when the OS reports the
    // child gone on its own, BEFORE any engine-side teardown kill. It drives
    // `killed_by_signal`, so a stray external SIGTERM (exit=143) is told apart
    // from a clean exit and from an engine-initiated cancel. `None` means the
    // engine tore the child down, which is not auto-resumable.
    let mut natural_exit_status: Option<std::process::ExitStatus> = None;
    // True once the child has been reaped (`wait()` resolved). Gates the
    // group-kill below: after reaping, the pid (hence the group id) may be
    // recycled, so signalling the group would risk unrelated processes.
    let mut child_reaped = false;
    let mut line_buf = String::new();
    // Parse state that spans lines of THIS stream, and only this one. Owned by
    // the driver task, so it lives and dies with the subprocess and no other
    // session can see it. Today it holds the per-message usage dedup.
    let mut stream_state = CcStreamState::default();
    loop {
        tokio::select! {
            read_result = stdout_reader.read_line(&mut line_buf) => {
                match read_result {
                    Ok(0) => {
                        log!("[ClaudeCode] driver stdout EOF — closing session");
                        break;
                    }
                    Ok(_) => {
                        for ev in parse_line(&mut stream_state, &line_buf) {
                            if let AgentEvent::Init { session_id: ref sid, .. } = ev {
                                session_id = Some(sid.clone());
                            }
                            if events_tx.send(ev).is_err() {
                                // Consumer dropped: stop forwarding this line's
                                // remaining events. This `break` leaves the
                                // `for` only; teardown is the `cancel` /
                                // `child.wait()` arms' job, and dropping the
                                // receiver goes hand in hand with cancelling.
                                line_buf.clear();
                                break;
                            }
                        }
                        line_buf.clear();
                    }
                    Err(e) => {
                        log!("[ClaudeCode] stdout read error: {}", e);
                        break;
                    }
                }
            }
            input = input_rx.recv() => {
                let Some(input) = input else { break };
                let line = format_user_input(&input, session_id.as_deref());
                if let Err(e) = stdin.write_all(line.as_bytes()).await {
                    log!("[ClaudeCode] failed to write user input to stdin: {}", e);
                    break;
                }
                if let Err(e) = stdin.flush().await {
                    log!("[ClaudeCode] failed to flush stdin after user input: {}", e);
                    break;
                }
            }
            req = control_rx.recv() => {
                let Some(req) = req else { break };
                let request_id = uuid::Uuid::new_v4().to_string();
                let mut line = cc_control_request_to_json(&req, &request_id);
                line.push('\n');
                if let Err(e) = stdin.write_all(line.as_bytes()).await {
                    log!("[ClaudeCode] failed to write control_request to stdin: {}", e);
                    break;
                }
                if let Err(e) = stdin.flush().await {
                    log!("[ClaudeCode] failed to flush stdin after control_request: {}", e);
                    break;
                }
            }
            _ = cancel.cancelled() => {
                log!("[ClaudeCode] cancellation signalled — terminating CC process");
                break;
            }
            wait_result = child.wait() => {
                // Always-on subprocess exit handler. Fires the instant the
                // OS reports the child is gone, regardless of stdout state.
                //
                // A grandchild that inherited stdout (rustc under cargo, a
                // backgrounded Bash tool) keeps the pipe open after CC's main
                // process dies. Polling `try_wait` on a timer instead is
                // starved by continuous grandchild noise, because
                // `tokio::select!` re-creates its futures each iteration. The
                // engine then wedges at status='running' forever.
                //
                // After the exit, drain remaining stdout with a bounded
                // timeout. A final `Result` line CC flushed before exiting is
                // still forwarded, without blocking on a noisy grandchild.
                log!(
                    "[ClaudeCode] CC process exited (pid={} status={}) — draining remaining stdout",
                    child_pid.map(|p| p.to_string()).unwrap_or_else(|| "?".to_string()),
                    format_exit_status(&wait_result),
                );
                // The child died on its own — record its true status before any
                // teardown so the safety net can auto-resume a stray-kill turn.
                natural_exit_status = wait_result.ok();
                child_reaped = true;
                // tokio's `read_line` is not cancel-safe: when the stdout
                // arm's read_line resolves Ready in the same poll cycle as
                // wait_result, select! drops the read_line value but
                // `*output = string` has already mutated line_buf. The
                // captured line would be silently lost — and the next
                // read_line would append fresh bytes to it, corrupting
                // parse_line. Forward any pre-captured line before draining.
                if !line_buf.is_empty() {
                    for ev in parse_line(&mut stream_state, &line_buf) {
                        let _ = events_tx.send(ev);
                    }
                    line_buf.clear();
                }
                let drain_deadline = tokio::time::Instant::now()
                    + std::time::Duration::from_millis(500);
                loop {
                    match tokio::time::timeout_at(
                        drain_deadline,
                        stdout_reader.read_line(&mut line_buf),
                    )
                    .await
                    {
                        Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                        Ok(Ok(_)) => {
                            for ev in parse_line(&mut stream_state, &line_buf) {
                                if events_tx.send(ev).is_err() {
                                    line_buf.clear();
                                    break;
                                }
                            }
                            line_buf.clear();
                        }
                    }
                }
                break;
            }
        }
    }

    // If the child already died on its own but a different select! arm won the
    // race, reap it here BEFORE any teardown kill. Its true exit status then
    // still classifies the death. A still-running child yields `None` and stays
    // unreaped.
    if !child_reaped {
        if let Ok(Some(status)) = child.try_wait() {
            natural_exit_status = Some(status);
            child_reaped = true;
        }
    }

    // Tear down the whole process group, so no descendant is left orphaned
    // holding the stdout pipe. Graceful-first: SIGTERM the group, wait out
    // `GROUP_TEARDOWN_GRACE`, then SIGKILL. Only while the child is unreaped,
    // see `signal_child_process_group`'s pid-recycle caveat.
    #[cfg(unix)]
    if !child_reaped {
        if let Some(pid) = child_pid {
            crate::runtime::spawn_env::graceful_kill_child_process_group(pid, GROUP_TEARDOWN_GRACE)
                .await;
        }
    }

    // Make sure the child is gone before draining stderr — otherwise stderr
    // could keep producing output and we'd block.
    let _ = child.start_kill();
    if tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }

    let stderr_tail = drain_stderr(&mut stderr_reader).await;
    if stderr_tail.is_empty() {
        // Absence-of-evidence is itself evidence. Without this log we
        // can't distinguish "stderr was empty" (e.g. SIGKILL or clean
        // SIGTERM-handled exit) from "we forgot to log stderr".
        log!("[ClaudeCode] Claude Code stderr: <empty>");
    } else {
        log!("[ClaudeCode] Claude Code stderr: {}", stderr_tail.trim());
    }
    let killed_by_signal = natural_exit_status
        .as_ref()
        .map(exit_indicates_signal_kill)
        .unwrap_or(false);
    let _ = events_tx.send(AgentEvent::Exited { killed_by_signal });
    // events_tx drops here — channel closes, consumer sees None.
}

#[cfg(test)]
#[path = "claude_code_tests/parsing.rs"]
mod parsing_tests;

#[cfg(test)]
#[path = "claude_code_tests/commands.rs"]
mod commands_tests;

#[cfg(test)]
#[path = "claude_code_tests/build_command.rs"]
mod build_command_tests;

#[cfg(test)]
#[path = "claude_code_tests/driver.rs"]
mod driver_tests;
