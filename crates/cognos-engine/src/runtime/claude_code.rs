use async_trait::async_trait;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::agent_runtime::{
    AgentEvent, AgentInput, AgentKind, AgentRuntime, ControlRequest, RunningAgent, SpawnArgs,
};
use super::cognos_cli::{
    cognos_cli_dir, ensure_workspace_bin_symlink, install_cognos_cli_skill, path_with_prefix,
};

/// Hardcoded list of models available in Claude Code's `/model` picker.
/// Maintained manually — use the `update-cognos-cc-models` skill to keep in sync.
/// Source: https://code.claude.com/docs/en/model-config
pub const CC_MODEL_OPTIONS: &[(&str, &str, &str)] = &[
    // (value, label, description)
    ("default", "Default", "Use the default model for your plan"),
    ("sonnet", "Sonnet 4.6", "Best for everyday tasks"),
    (
        "sonnet[1m]",
        "Sonnet 4.6 (1M)",
        "Sonnet 4.6 for long sessions",
    ),
    ("claude-opus-4-7", "Opus 4.7", "Latest and most capable"),
    ("claude-opus-4-1", "Opus 4.1", "Legacy"),
    ("opus", "Opus 4.6", "Most capable for complex work"),
    ("opus[1m]", "Opus 4.6 (1M)", "Opus 4.6 for long sessions"),
    ("haiku", "Haiku 4.5", "Fastest for quick answers"),
];

/// Map a full CC CLI model ID (e.g. `claude-sonnet-4-6`) back to the short
/// alias we originally sent (e.g. `sonnet`).  Returns the input unchanged if
/// it already is a known CC_MODEL_OPTIONS value or doesn't match any alias.
pub fn normalize_cc_model_id(full_id: &str) -> &str {
    if CC_MODEL_OPTIONS.iter().any(|(v, _, _)| *v == full_id) {
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

/// Reasoning effort levels for Claude Code's thinking budget.
pub const CC_REASONING_EFFORT_OPTIONS: &[(&str, &str, &str)] = &[
    // (value, label, description)
    ("low", "Low", "Minimal thinking \u{00b7} Fastest responses"),
    ("medium", "Medium", "Balanced thinking \u{00b7} Default"),
    ("high", "High", "Deep thinking \u{00b7} Most thorough"),
    (
        "xhigh",
        "Extra High",
        "Extra deep thinking \u{00b7} Higher than High",
    ),
    ("max", "Max", "Maximum thinking \u{00b7} Extended reasoning"),
];

/// Parse a single JSON line from Claude Code's stream output.
/// Returns all recognized events from the line. An assistant message with
/// multiple content blocks (text + tool_use) produces multiple events.
/// Never produces `AgentEvent::Exited` — that variant is emitted by the
/// driver task on process exit.
pub fn parse_line(line: &str) -> Vec<AgentEvent> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }

    let val: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match event_type {
        // Only parse subtype "init" — hook events (hook_started, hook_response,
        // hook_progress) also have type "system" + session_id but lack slash_commands.
        // Without this guard, hook events after init overwrite commands with empty arrays.
        "system" => {
            let subtype = val.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
            if subtype == "init" {
                if let Some(sid) = val.get("session_id").and_then(|v| v.as_str()) {
                    let slash_commands = val
                        .get("slash_commands")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    let skills = val
                        .get("skills")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    let model = val.get("model").and_then(|v| v.as_str()).map(String::from);
                    vec![AgentEvent::Init {
                        session_id: sid.to_string(),
                        model,
                        slash_commands,
                        skills,
                    }]
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        }
        "assistant" => {
            let mut events = Vec::new();
            if let Some(content) = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for block in content {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                events.push(AgentEvent::Message {
                                    role: "assistant".to_string(),
                                    text: text.to_string(),
                                });
                            }
                        }
                        "tool_use" => {
                            let name = block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let input = block
                                .get("input")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            let id = block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            events.push(AgentEvent::ToolUse { name, input, id });
                        }
                        _ => {}
                    }
                }
            }
            events
        }
        "tool_result" => {
            let content = val
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            let is_error = val
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let status = if is_error { "error" } else { "success" };
            vec![AgentEvent::ToolResult {
                output: content,
                status: status.to_string(),
            }]
        }
        // CC 2.1.76+ sends tool results as "type": "user" with tool_result content blocks
        "user" => {
            let mut events = Vec::new();
            if let Some(content) = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for block in content {
                    if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                        let output = block
                            .get("content")
                            .and_then(|c| c.as_str())
                            .unwrap_or("")
                            .to_string();
                        let is_error = block
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let status = if is_error { "error" } else { "success" };
                        events.push(AgentEvent::ToolResult {
                            output,
                            status: status.to_string(),
                        });
                    }
                }
            }
            events
        }
        "result" => {
            let text = val
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let duration = val.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);
            vec![AgentEvent::Result {
                text,
                duration_ms: duration,
            }]
        }
        // CC 2.1.76+ sends streaming deltas as "type": "stream_event" wrappers.
        // Silently ignore these — tool calls are captured from complete "assistant"
        // messages, and text from AgentEvent::Message. Stream events are just
        // intermediate deltas that don't need separate handling.
        "stream_event" => Vec::new(),
        // control_response is CC's reply to a control_request (e.g. interrupt).
        // We don't need to act on it — the interrupt itself triggers a Result event.
        "control_response" => Vec::new(),
        other => {
            if !other.is_empty() {
                log!("[ClaudeCode] Unrecognized event type: {}", other);
            }
            Vec::new()
        }
    }
}

/// Render the menu of supported control commands for the frontend's `/model`
/// picker. CC-specific — Codex and other agents have their own menus.
pub fn cc_command_definitions() -> serde_json::Value {
    fn options_to_json(opts: &[(&str, &str, &str)]) -> Vec<serde_json::Value> {
        opts.iter()
            .map(|(value, label, desc)| {
                serde_json::json!({
                    "value": value,
                    "label": label,
                    "description": desc,
                })
            })
            .collect()
    }
    let model_options = options_to_json(CC_MODEL_OPTIONS);
    let effort_options = options_to_json(CC_REASONING_EFFORT_OPTIONS);
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
    CC_REASONING_EFFORT_OPTIONS
        .iter()
        .any(|(v, _, _)| *v == value)
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
    fn kind(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }

    async fn spawn(
        &self,
        args: SpawnArgs<'_>,
        cancel: CancellationToken,
    ) -> Result<RunningAgent, Box<dyn std::error::Error + Send + Sync>> {
        let cli_dir = cognos_cli_dir();
        if cli_dir.is_none() {
            crate::log!(
                "[ClaudeCode] cognos CLI binary not found near current_exe — \
                 spawned CC sessions won't have the `cognos` command on PATH \
                 (build with `cargo build -p cognos-cli`)"
            );
        }
        if let Err(e) = install_cognos_cli_skill(args.worktree_path, cli_dir) {
            crate::log!(
                "[ClaudeCode] failed to install cognos-cli skill into {}: {}",
                args.worktree_path.display(),
                e
            );
        }
        ensure_workspace_bin_symlink(args.worktree_path, cli_dir);

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
            kind: AgentKind::ClaudeCode,
            events_rx,
            input_tx,
            control_tx,
        })
    }
}

/// Build the `claude` Command with all flags and env vars. Extracted so unit
/// tests can inspect args/env without spawning. `cli_dir` is the directory
/// containing the `cognos` binary, prepended to PATH; pass `None` to skip.
fn build_command(args: &SpawnArgs<'_>, cli_dir: Option<&Path>) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("claude");
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
        .arg("mcp__cognos_perm__approve")
        .arg("--mcp-config")
        .arg(permission_mcp_config_json())
        .arg("--strict-mcp-config")
        .current_dir(args.worktree_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env_remove("CLAUDECODE")
        .env("RUSTC_WRAPPER", "sccache");
    if args.resume_session_id.is_none() {
        cmd.arg("--include-partial-messages");
    }

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
    // Read by spawn-thread skill; see ChatRequest::parent_thread_id.
    cmd.env("COGNOS_THREAD_ID", args.thread_id.to_string());
    cmd.env("COGNOS_WORKSPACE", args.workspace_path);
    // The engine permission handler now waits indefinitely for the user
    // (matching `AskUserQuestion`'s "stay idle" behavior). Set CC's MCP tool
    // timeout to 24 hours so its MCP client doesn't give up and force a
    // retry that surfaces a duplicate prompt — effectively "never time out"
    // for any practical session.
    cmd.env("MCP_TOOL_TIMEOUT", (86_400u64 * 1000).to_string());
    if let Some(cli_dir) = cli_dir {
        match path_with_prefix(cli_dir) {
            Ok(p) => {
                cmd.env("PATH", p);
            }
            Err(e) => {
                crate::log!("[ClaudeCode] failed to join PATH for cognos CLI: {}", e);
            }
        }
    }
    cmd
}

/// Build the `--mcp-config` JSON for the cognos permission server. CC spawns
/// `cognos mcp-permission-server` over stdio; the server reads `COGNOS_THREAD_ID`
/// + `COGNOS_WORKSPACE` from the inherited env to resolve the engine endpoint.
fn permission_mcp_config_json() -> String {
    serde_json::json!({
        "mcpServers": {
            "cognos_perm": {
                "command": "cognos",
                "args": ["mcp-permission-server"]
            }
        }
    })
    .to_string()
}

/// Format a user input as the JSON line CC expects on stdin. `session_id` is
/// the CC session id (from the latest Init event, or the resumed id).
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

/// Drain remaining stderr from the CC child (up to 4 KB, 2 s timeout).
async fn drain_stderr(stderr_reader: &mut BufReader<ChildStderr>) -> String {
    let mut output = String::with_capacity(4096);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let mut line = String::new();
        while let Ok(n) = stderr_reader.read_line(&mut line).await {
            if n == 0 {
                break;
            }
            output.push_str(&line);
            line.clear();
            if output.len() > 4096 {
                break;
            }
        }
    })
    .await;
    output
}

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
    let mut line_buf = String::new();
    loop {
        tokio::select! {
            read_result = stdout_reader.read_line(&mut line_buf) => {
                match read_result {
                    Ok(0) => break, // EOF — child exiting
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
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                // Detect dead process whose stdout pipe didn't EOF (e.g. after
                // macOS sleep kills/hangs the process). Without this, the
                // session would stay running forever from the engine's view.
                if let Ok(Some(status)) = child.try_wait() {
                    log!("[ClaudeCode] CC process exited (status: {}) but stdout didn't EOF — closing session", status);
                    break;
                }
            }
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
    if !stderr_tail.is_empty() {
        log!("Claude Code stderr: {}", stderr_tail.trim());
    }
    let _ = events_tx.send(AgentEvent::Exited);
    // events_tx drops here — channel closes, consumer sees None.
}

#[cfg(test)]
#[path = "claude_code_tests.rs"]
mod tests;
