use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::agent_runtime::{
    AgentEvent, AgentInput, AgentRuntime, CodingAgent, ControlRequest, RunningAgent, SpawnArgs,
};
use super::lucidos_cli::{
    ensure_workspace_bin_symlink, install_lucidos_cli_skill, lucidos_cli_dir,
    LUCIDOS_CLI_SKILL_REL_PATH,
};
use super::spawn_env::{apply_lucidos_env, drain_stderr};

/// Single source of truth for CC's `/model` and `/effort` picker entries.
/// The data lives in `cc_menu_options.json` next to this file — see that
/// file's `_note` for why it's hand-maintained and how to update it. The
/// JSON is `include_str!()`-baked at compile time and parsed once via
/// `LazyLock` so there's no runtime IO cost.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CcMenuOption {
    pub value: String,
    pub label: String,
    pub description: String,
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
/// CC strips the `[1m]` suffix when echoing the model in Init / per-message
/// Usage frames, so a naive `normalize_cc_model_id` loses the 1M-context
/// signal that `context_window_for` keys on. If the engine pinned `[1m]` and
/// CC reports the same base model, re-attach the suffix.
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
pub use parse::parse_line;

/// Render the menu of supported control commands for the frontend's `/model`
/// picker. CC-specific — Codex and other agents have their own menus.
pub fn cc_command_definitions() -> serde_json::Value {
    fn options_to_json(opts: &[CcMenuOption]) -> Vec<serde_json::Value> {
        opts.iter()
            .map(|m| {
                serde_json::json!({
                    "value": m.value,
                    "label": m.label,
                    "description": m.description,
                })
            })
            .collect()
    }
    let model_options = options_to_json(cc_model_options());
    let effort_options = options_to_json(cc_reasoning_effort_options());
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
        if cli_dir.is_none() {
            crate::log!(
                "[ClaudeCode] lucidos CLI binary not found near current_exe — \
                 spawned Claude Code sessions won't have the `lucidos` command on PATH \
                 (build with `cargo build -p lucidos-cli`)"
            );
        }
        if let Err(e) = install_lucidos_cli_skill(args.worktree_path, cli_dir) {
            crate::log!(
                "[ClaudeCode] failed to install lucidos-cli skill into {}: {}",
                args.worktree_path.display(),
                e
            );
        }
        // If the injected skill is *tracked* in this repo (an earlier session
        // auto-committed it before the deep-path exclude existed), overwriting
        // it on disk just now produces a phantom `M` that `.git/info/exclude`
        // can't hide. Skip-worktree it so this session never sees a change it
        // didn't author. No-op for the Lucidos repo, where the tracked copy is
        // identical and must stay editable.
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

/// Resolve the `claude` executable for spawn. The CC native installer (the
/// canonical install since 2026-01) symlinks `$HOME/.local/bin/claude` at the
/// active version, but a launchd-, IDE-, or any non-interactive-shell-launched
/// engine inherits a PATH that omits `~/.local/bin`. A bare
/// `Command::new("claude")` then ENOENTs even though the binary is installed —
/// surfacing as "Failed to start Claude Code: No such file or directory".
///
/// Prefer the native-installer path when it exists; otherwise return bare
/// `"claude"` so `Command::spawn` does its own PATH lookup (handles users who
/// installed via Homebrew, npm, a custom symlink, etc.). `home` is injected to
/// keep the function pure and testable; production callers pass
/// `std::env::var_os("HOME")`.
fn resolve_claude_binary(home: Option<&Path>) -> std::ffi::OsString {
    if let Some(home) = home {
        let native = home.join(".local/bin/claude");
        if native.exists() {
            return native.into_os_string();
        }
    }
    std::ffi::OsString::from("claude")
}

/// Build the `claude` Command with all flags and env vars. Extracted so unit
/// tests can inspect args/env without spawning. `cli_dir` is the directory
/// containing the `lucidos` binary, prepended to PATH; pass `None` to skip.
fn build_command(args: &SpawnArgs<'_>, cli_dir: Option<&Path>) -> tokio::process::Command {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let claude_bin = resolve_claude_binary(home.as_deref());
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
        .arg("mcp__lucidos_perm__approve")
        .arg("--mcp-config")
        .arg(permission_mcp_config_json())
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
    // Always request partial-message streaming — fresh AND resumed sessions.
    // The `stream_event` deltas it produces are turned into
    // `AgentEvent::StreamActivity` liveness pings that keep the watchdog's
    // inactivity clock fresh through a long single step (extended thinking on a
    // hard problem). Omitting it on `--resume` (the old fresh-only gate, a fossil
    // from when resume was a separate spawn path) left the heartbeat ticking only
    // at step boundaries, so the watchdog killed long unattended steps mid-work —
    // follow-ups, engine-restart recovery, merge-conflict resolution, hardening.
    // The flag is a streaming-output option, orthogonal to `--resume`.
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
    // Agent-independent Lucidos env contract (workspace, host protection,
    // PG*, subprocess origin, spawn metadata, RUSTC_WRAPPER, PATH) — shared
    // with every other AgentRuntime via `spawn_env::apply_lucidos_env`.
    apply_lucidos_env(&mut cmd, args, cli_dir, "ClaudeCode");
    // Root-cause fix for the stray-SIGTERM truncation bug: give CC its OWN
    // process group so a group-wide signal to the engine never reaches it.
    // The engine ignores SIGTERM but CC's Node runtime does not (exit=143).
    // See `spawn_env::isolate_in_process_group`.
    crate::runtime::spawn_env::isolate_in_process_group(&mut cmd);
    // The engine permission handler now waits indefinitely for the user
    // (matching `AskUserQuestion`'s "stay idle" behavior). CC has TWO
    // separate MCP timeouts that both have to be lifted, otherwise whichever
    // is shorter forces a retry that surfaces a duplicate prompt:
    //   * MCP_TOOL_TIMEOUT — per-tool-call cap (default 1e8 ms ≈ 28h)
    //   * MCP_TIMEOUT      — per-RPC cap (default 30_000 ms = 30s)
    // The 30-second `MCP_TIMEOUT` default is the one that historically caused
    // the "infinite loop of identical permission cards every ~30s" bug —
    // CC's MCP client would cancel the permission RPC, the engine would gc
    // the orphaned waiter, and CC's model would retry the original tool.
    // Set both to 24 hours — effectively "never time out" for any practical
    // session.
    cmd.env("MCP_TOOL_TIMEOUT", (86_400u64 * 1000).to_string());
    cmd.env("MCP_TIMEOUT", (86_400u64 * 1000).to_string());
    // NOTE: we deliberately do NOT attempt a post-fork macOS TCC "responsibility
    // disclaim" here (a prior version called
    // `responsibility_set_caller_responsible_for_self()` from a `pre_exec` hook).
    // It was a verified no-op: macOS captures TCC responsibility from the
    // `posix_spawnattr_t` at the `posix_spawn`/exec syscall, but Rust's
    // `pre_exec` forces the `fork()`+`execvp()` path (no `posix_spawn`), so the
    // only effective knob — `responsibility_spawnattrs_setdisclaim` — never gets
    // set. Empirically, `responsibility_set_caller_responsible_for_self()` and
    // `responsibility_set_pid_responsible_for_pid(self, self)` both leave the
    // child attributed to the engine in every context tested (forked child,
    // running process, parent-side reassignment). TCC grant stability for the
    // dev engine is handled instead by signing the engine binary with a stable
    // self-signed identity at build time (`scripts/lib/codesign.sh`), giving it
    // a rebuild-stable Designated Requirement so a single Allow click sticks.
    cmd
}

/// Build the `--mcp-config` JSON for the lucidos permission server. CC spawns
/// `lucidos mcp-permission-server` over stdio; the server reads `LUCIDOS_THREAD_ID`
/// + `LUCIDOS_WORKSPACE` from the inherited env to resolve the engine endpoint.
fn permission_mcp_config_json() -> String {
    serde_json::json!({
        "mcpServers": {
            "lucidos_perm": {
                "command": "lucidos",
                "args": ["mcp-permission-server"]
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

/// Decode a `child.wait()` result into a debuggable string. `{:?}` on
/// `ExitStatus` prints `unix_wait_status(36608)` — useless without
/// manual decoding when a session-ending CC death lands in production
/// logs. Three cases worth distinguishing:
///
/// * `exit=N` — clean exit with code `N` below the signal-convention
///   range. Most CC sessions end here (exit 0 / 1 / SDK-specific codes).
/// * `exit=N (probable SIGNAME)` — Node.js convention. A child that
///   installs a signal handler typically re-exits with `128 + signum`
///   after running cleanup. The hint converts the cryptic 143/137 codes
///   the Lucidos engine sees from Claude Code into "SIGTERM" / "SIGKILL"
///   at log-read time so future "who killed CC?" investigations don't
///   require manual arithmetic.
/// * `signal=NAME (N)` / `signal=N` — kernel delivered the signal as
///   cause-of-death directly (no handler ran). Distinct from the
///   exit=128+N path because the child never got to clean up.
///
/// `signal_name` is the bridge: keeps the named-signal table tiny
/// (Node.js and the macOS Jetsam stack between them cover the common
/// values we care about) and falls through to bare numbers for anything
/// we haven't mapped, so the log never silently drops information.
fn signal_name(sig: i32) -> Option<&'static str> {
    match sig {
        1 => Some("SIGHUP"),
        2 => Some("SIGINT"),
        3 => Some("SIGQUIT"),
        6 => Some("SIGABRT"),
        9 => Some("SIGKILL"),
        13 => Some("SIGPIPE"),
        15 => Some("SIGTERM"),
        _ => None,
    }
}

/// True when an exit status indicates the process was killed by a signal —
/// either delivered directly by the kernel (`status.signal()` is set) or via
/// the Node.js `128 + signum` convention (a handler caught the signal, ran
/// cleanup, and re-exited `exit=143`/`exit=137`). Used to distinguish a stray
/// external kill (auto-resumable) from a clean exit. Mirrors the case analysis
/// in `format_exit_status`; lives next to it so the two stay in lockstep.
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
/// the engine force-kills it (SIGKILL). Sized for a Playwright runner in the tree
/// to close the browsers it tracks — those `setsid`-escape the group, so only the
/// runner's own teardown reaps them; a bare SIGKILL orphaned them (the 2026-06-24
/// WebKit pile-up). Runs in the detached `driver_task`, off the cancel UX path,
/// so the wait costs no interactive latency. See
/// `spawn_env::graceful_kill_child_process_group`.
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
    // Capture the child pid before the wait arm consumes the Child —
    // `tokio::process::Child::id()` returns `None` after `wait()` resolves,
    // and the post-loop diagnostic log line is the one place we genuinely
    // need it (for correlating the engine's "CC process exited" line with
    // macOS unified-log entries / ps output during incident analysis).
    let child_pid = child.id();
    // The process's NATURAL exit status — captured only when the OS reports
    // the child gone on its own (the `child.wait()` arm, or a post-loop
    // `try_wait`), BEFORE any engine-side teardown kill. Drives
    // `killed_by_signal` so a stray external SIGTERM (exit=143) is told apart
    // from a clean exit or an engine-initiated cancel (whose SIGKILL would
    // otherwise masquerade as a stray signal). `None` = the engine tore the
    // child down (cancel/EOF) — not auto-resumable.
    let mut natural_exit_status: Option<std::process::ExitStatus> = None;
    // True once the child has been reaped (`wait()` resolved). Gates the
    // group-kill below: after reaping, the pid (hence the group id) may be
    // recycled, so signalling the group would risk unrelated processes.
    let mut child_reaped = false;
    let mut line_buf = String::new();
    loop {
        tokio::select! {
            read_result = stdout_reader.read_line(&mut line_buf) => {
                match read_result {
                    Ok(0) => {
                        log!("[ClaudeCode] driver stdout EOF — closing session");
                        break;
                    }
                    Ok(_) => {
                        for ev in parse_line(&line_buf) {
                            if let AgentEvent::Init { session_id: ref sid, .. } = ev {
                                session_id = Some(sid.clone());
                            }
                            if events_tx.send(ev).is_err() {
                                // Consumer dropped — abandon the process.
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
                // Catches the case where a grandchild (rustc forked by
                // cargo, a backgrounded Bash tool, anything that inherited
                // stdout) keeps the pipe open after CC's main process dies.
                // Without this arm, the previous 500ms `try_wait` poll was
                // starved by continuous grandchild noise: tokio::select!
                // re-creates futures each iteration, so a noise line every
                // ~100ms resets the 500ms timer before it can resolve. The
                // engine then wedged at status='running' forever — no
                // CodingAgentIdled, no ResponseAborted — until the
                // grandchild eventually died on its own.
                //
                // After the exit, drain remaining stdout with a bounded
                // timeout so a final `Result` line CC flushed before exiting
                // is still forwarded (clean-exit path) without blocking on
                // a noisy grandchild (silent-death path).
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
                    for ev in parse_line(&line_buf) {
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
                            for ev in parse_line(&line_buf) {
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
    // race (stdout EOF / error / a pre-captured line), reap it here BEFORE any
    // teardown kill so its true exit status still classifies the death. A
    // still-running child (genuine cancel) yields `None` and stays unreaped.
    if !child_reaped {
        if let Ok(Some(status)) = child.try_wait() {
            natural_exit_status = Some(status);
            child_reaped = true;
        }
    }

    // Deliberate teardown: tear down the whole process group (CC + every
    // descendant it spawned — Bash tools, cargo/rustc, an e2e Playwright runner)
    // so nothing is left orphaned holding the stdout pipe. Graceful-first: SIGTERM
    // the group + a short grace so a Playwright runner can close the browsers it
    // tracks (they `setsid`-escape the group and can't be reached directly — only
    // its own teardown reaps them), THEN SIGKILL. A bare SIGKILL orphaned those
    // browsers (the 2026-06-24 WebKit pile-up). Only while the child is unreaped —
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
