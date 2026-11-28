use std::collections::{HashMap, HashSet, VecDeque};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::agent_runtime::{
    AgentEvent, AgentExitFacts, AgentInput, AgentPermissionRequest, ControlRequest,
    InputWithdrawal, McpServerStatus, SessionContextDue, WithdrawRequest,
};
use super::billing_provider::{CodexEndpointConfig, CodexProviderEndpoints};
use super::claude_code::{format_exit_status, EXIT_AFTER_STDOUT_EOF_GRACE};
use super::codex::{
    answer_withdraw, lucidos_mcp_server_config_json, write_image_files, CodexConfig,
};
use super::codex_app_server_parse::{
    parse_app_server_line, parse_approval_request, AppServerLine, AppServerTracker,
};
use super::spawn_env::StderrTail;
use crate::engine::thread_events::{TurnEndFacts, TurnEndStatus};

fn parse_mcp_server_statuses(
    result: &serde_json::Value,
) -> Option<(Vec<McpServerStatus>, Option<&str>)> {
    let cursor = match result.get("nextCursor") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(value.as_str().filter(|cursor| !cursor.is_empty())?),
    };
    let servers = result
        .get("data")
        .and_then(|data| data.as_array())?
        .iter()
        .map(|server| {
            let name = server.get("name")?.as_str()?.to_owned();
            let auth = server.get("authStatus").and_then(|value| value.as_str());
            let connection = server
                .get("connectionStatus")
                .and_then(|value| value.as_str());
            Some(McpServerStatus {
                name,
                status: connection.or(auth)?.to_owned(),
                needs_sign_in: auth == Some("notLoggedIn")
                    || connection == Some("authenticationRequired"),
            })
        })
        .collect::<Option<_>>()?;
    Some((servers, cursor))
}

const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingRequest {
    Initialize,
    SteerProbe,
    TurnSteer,
    Thread,
    CommandDenyHook,
    McpStatus,
    BillingConfig(u64),
    BillingAccount(u64),
    BillingProviders(u64),
    TurnStart(uuid::Uuid),
    TurnInterrupt,
}

#[derive(Default)]
struct BillingRead {
    generation: u64,
    ready: bool,
    deadline: Option<tokio::time::Instant>,
    account: Option<super::billing_provider::BillingAttribution>,
    endpoints: Option<CodexEndpointConfig>,
    providers: Option<CodexProviderEndpoints>,
}

enum DriverAction {
    SendLine(String),
    ApprovalResolved {
        line: String,
        item_id: String,
        allowed: bool,
    },
}

fn request_line(id: u64, method: &str, params: serde_json::Value) -> String {
    let mut line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string();
    line.push('\n');
    line
}

fn notification_line(method: &str) -> String {
    let mut line = serde_json::json!({ "jsonrpc": "2.0", "method": method }).to_string();
    line.push('\n');
    line
}

fn response_line(id: &serde_json::Value, result: serde_json::Value) -> String {
    let mut line = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string();
    line.push('\n');
    line
}

fn error_response_line(id: &serde_json::Value, code: i64, message: &str) -> String {
    let mut line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
    .to_string();
    line.push('\n');
    line
}

/// INVARIANT: Build the `thread/start` (fresh) or `thread/resume` params. Pure so the
/// unit tests can pin the shape without a child.
///
/// `developerInstructions` carries the engine's system prompt on BOTH paths
/// (matching CC, which re-passes `--append-system-prompt-file` on every resume).
/// The `config` object mirrors the exec driver's `-c` overrides: sandbox
/// network on, the same extra writable roots the exec driver passes as
/// `--add-dir` (see `codex::sandbox_writable_roots`), and the `lucidos` MCP
/// server for `ask_user_question`.
fn build_thread_request(
    config: &CodexConfig,
    resume_session_id: Option<&str>,
) -> (&'static str, serde_json::Value) {
    let mut sandbox_ww = serde_json::Map::new();
    sandbox_ww.insert("network_access".to_string(), true.into());
    if !config.sandbox_writable_roots.is_empty() {
        sandbox_ww.insert(
            "writable_roots".to_string(),
            serde_json::Value::Array(
                config
                    .sandbox_writable_roots
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned().into())
                    .collect(),
            ),
        );
    }
    let mut config_obj = serde_json::json!({
        "sandbox_workspace_write": sandbox_ww,
        "mcp_servers": { "lucidos": lucidos_mcp_server_config_json(&config.env) },
        "model_reasoning_summary": super::codex::CODEX_REASONING_SUMMARY,
        "project_doc_fallback_filenames": super::codex::CODEX_PROJECT_DOC_FALLBACKS,
        "project_doc_max_bytes": super::codex::CODEX_PROJECT_DOC_MAX_BYTES,
    });
    config_obj["features"]["hooks"] = true.into();
    config_obj["hooks"] = super::codex::command_deny_hooks();
    if !config.login_shell {
        config_obj["allow_login_shell"] = false.into();
    }

    let mut params = serde_json::Map::new();
    params.insert(
        "cwd".to_string(),
        config.worktree_path.to_string_lossy().into_owned().into(),
    );
    params.insert(
        "sandbox".to_string(),
        super::codex::codex_sandbox_mode(config).into(),
    );
    // INVARIANT: The point of this driver: commands and writes raise a card instead of
    // failing silently (the exec escape hatch keeps `never`).
    params.insert(
        "approvalPolicy".to_string(),
        super::codex::codex_approval_policy(config).into(),
    );
    params.insert("config".to_string(), config_obj);
    if let Some(sp) = config.system_prompt.as_deref().filter(|s| !s.is_empty()) {
        params.insert("developerInstructions".to_string(), sp.into());
    }
    if let Some(m) = config
        .model
        .as_deref()
        .filter(|m| !m.is_empty() && *m != "default")
    {
        params.insert("model".to_string(), m.into());
    }
    match resume_session_id {
        Some(sid) => {
            params.insert("threadId".to_string(), sid.into());
            ("thread/resume", serde_json::Value::Object(params))
        }
        None => ("thread/start", serde_json::Value::Object(params)),
    }
}

fn command_deny_hook_is_active(result: &serde_json::Value) -> bool {
    use super::codex_side_question::{is_trusted_pre_tool_hook, listed_hooks};
    listed_hooks(result).is_ok_and(|hooks| {
        hooks
            .into_iter()
            .any(|hook| is_trusted_pre_tool_hook(hook, "Bash", super::codex::COMMAND_DENY_HOOK))
    })
}

pub(super) fn build_turn_start_params(
    thread_id: &str,
    text: &str,
    image_paths: &[std::path::PathBuf],
    model: Option<&str>,
    effort: Option<&str>,
) -> serde_json::Value {
    let mut input: Vec<serde_json::Value> = Vec::new();
    if !text.is_empty() {
        input.push(serde_json::json!({ "type": "text", "text": text }));
    }
    for img in image_paths {
        input.push(serde_json::json!({
            "type": "localImage",
            "path": img.to_string_lossy(),
        }));
    }
    let mut params = serde_json::Map::new();
    params.insert("threadId".to_string(), thread_id.into());
    params.insert("input".to_string(), serde_json::Value::Array(input));
    if let Some(m) = model.filter(|m| !m.is_empty() && *m != "default") {
        params.insert("model".to_string(), m.into());
    }
    if let Some(e) = super::codex::validate_codex_effort(model, effort) {
        params.insert("effort".to_string(), e.into());
    }
    serde_json::Value::Object(params)
}

fn spawn_app_server_child(
    config: &CodexConfig,
) -> std::io::Result<(tokio::process::Child, Vec<String>)> {
    let mut cmd = config.sandbox.command(&config.codex_bin)?;
    cmd.arg("app-server")
        .current_dir(&config.worktree_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for k in &config.env_removed {
        cmd.env_remove(k);
    }
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    let secrets = super::spawn_env::command_secret_values(&cmd);
    Ok((super::spawn_env::spawn_below_engine(&mut cmd)?, secrets))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn app_server_driver_task(
    config: CodexConfig,
    resume_session_id: Option<String>,
    events_tx: mpsc::UnboundedSender<AgentEvent>,
    mut input_rx: mpsc::UnboundedReceiver<AgentInput>,
    mut control_rx: mpsc::UnboundedReceiver<ControlRequest>,
    mut withdraw_rx: mpsc::UnboundedReceiver<WithdrawRequest>,
    permission_tx: mpsc::UnboundedSender<AgentPermissionRequest>,
    cancel: CancellationToken,
) {
    let (mut child, secrets) = match spawn_app_server_child(&config) {
        Ok(spawned) => spawned,
        Err(e) => {
            log_legacy!("[CodexAppServer] failed to spawn codex app-server: {}", e);
            let _ = events_tx.send(AgentEvent::Result {
                text: String::new(),
                duration_ms: 0,
                error: Some(format!("Failed to start Codex: {e}")),
                provider_error: None,
                turn_end: Some(TurnEndFacts::of_status(TurnEndStatus::Failed)),
            });
            let _ = events_tx.send(AgentEvent::Exited {
                exit: AgentExitFacts::default(),
            });
            return;
        }
    };
    let child_pid = child.id();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    // INVARIANT: Read while the child runs: a process that lives for a whole turn-set
    // would block mid-write on an undrained 64 KB pipe, which reads as a hang.
    let stderr = StderrTail::spawn(child.stderr.take().expect("piped stderr"), secrets);

    // INVARIANT: Cancel-safe stdout: a dedicated reader task feeds whole lines into an
    // mpsc. `read_line` directly in a `select!` arm is NOT cancel-safe —
    // tokio's ReadLine future owns the partially-read bytes, so any other
    // arm winning a poll cycle mid-line discards the prefix irrecoverably,
    // and the next read returns an unparseable tail (a lost turn/completed
    // wedges the whole turn). The reader task never cancels mid-line;
    // `recv()` on the channel IS cancel-safe.
    let (lines_tx, mut lines_rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if lines_tx.send(std::mem::take(&mut buf)).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let (driver_action_tx, mut driver_action_rx) = mpsc::unbounded_channel::<DriverAction>();
    // INVARIANT: Approval waiter tasks live in a JoinSet so driver wind-down ABORTS
    // them: aborting drops their `respond_rx`, which fires the engine
    // waiter's `respond.closed()` arm, which drops the broadcast receiver,
    // which lets `gc_dead_entries` evict the pending card entry. A bare
    // `tokio::spawn` would leave the waiter pair alive forever after a child
    // crash (circular wait: task waits on engine, engine waits on user).
    let mut approval_tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    let mut tracker = AppServerTracker::new(resume_session_id.clone());
    let mut next_id: u64 = 1;
    let mut pending: HashMap<u64, PendingRequest> = HashMap::new();
    let mut queue: VecDeque<AgentInput> = VecDeque::new();
    let mut can_steer = false;
    let mut probe_deadline = None;
    let mut probe_done = false;
    let mut pending_init = None;
    let mut mcp_servers = Vec::new();
    let mut mcp_cursors = HashSet::new();
    let mut steer_input: Option<AgentInput> = None;
    let mut withdrawn_steer = None;
    let mut steer_deadline = None;
    let mut steer_blocked_for_turn = false;
    let mut deferred_terminal = None;
    let mut thread_ready = false;
    let mut billing = BillingRead::default();
    let mut billing_model_provider: Option<String> = None;
    let mut tried_fresh_thread_fallback = false;
    // INVARIANT: A fresh thread owes its first turn the session context. The fallback
    // below re-arms it, and a rejected turn leaves it owed.
    let mut session_context = SessionContextDue::for_spawn(
        resume_session_id.as_deref(),
        config.session_context.as_ref(),
    );
    let mut turn_input: Option<AgentInput> = None;
    let mut turn_in_flight = false;
    let mut turn_start = std::time::Instant::now();
    let mut turn_image_guards: Vec<tempfile::TempPath> = Vec::new();
    let mut shutdown = false;
    // INVARIANT: Set when child.wait() fires: lines may still be buffered in lines_rx —
    // keep processing them through the NORMAL arm until the reader task hits
    // EOF (recv = None) or this bounded deadline passes (a grandchild
    // holding the pipe open would otherwise stall the wind-down forever).
    let mut child_death_drain_deadline: Option<tokio::time::Instant> = None;
    // INVARIANT: True once the `child.wait()` arm below has reaped the child. It gates the
    // teardown's process-group kill, so it names the reaping rather than the
    // logging it also drives.
    let mut child_reaped = false;
    let mut natural_exit_status: Option<std::process::ExitStatus> = None;
    let mut stdout_closed = false;
    let handshake_deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;

    // INVARIANT: All stdin writes go through this macro: bounded so a wedged child
    // (stopped reading stdin, pipe full) can't block the loop beyond the
    // reach of the cancellation token — the engine's interrupt escalation
    // depends on this loop staying responsive.
    macro_rules! send_frame {
        ($line:expr) => {
            let line = $line;
            match tokio::time::timeout(std::time::Duration::from_secs(10), async {
                stdin.write_all(line.as_bytes()).await?;
                stdin.flush().await
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    log_legacy!("[CodexAppServer] stdin write failed — child gone? {}", e)
                }
                Err(_) => log_legacy!(
                    "[CodexAppServer] stdin write timed out — child wedged with a full pipe?"
                ),
            }
        };
    }

    {
        let id = next_id;
        next_id += 1;
        pending.insert(id, PendingRequest::Initialize);
        let line = request_line(
            id,
            "initialize",
            serde_json::json!({
                "clientInfo": {
                    "name": "lucidos-engine",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": { "experimentalApi": true },
            }),
        );
        send_frame!(line);
    }

    'session: while !shutdown {
        if !turn_in_flight && queue.is_empty() {
            billing.ready = false;
            billing.deadline = None;
        }
        if billing.deadline.is_some() {
            if let (Some(account), Some(endpoints), Some(providers)) =
                (&billing.account, billing.endpoints, billing.providers)
            {
                let attribution = account
                    .clone()
                    .codex_endpoint_checked(endpoints.is_default(providers));
                let _ = events_tx.send(AgentEvent::BillingObserved { attribution });
                billing.deadline = None;
                billing.ready = true;
            }
        }
        if thread_ready
            && !turn_in_flight
            && !queue.is_empty()
            && !billing.ready
            && billing.deadline.is_none()
        {
            pending.retain(|_, request| {
                !matches!(
                    request,
                    PendingRequest::BillingConfig(_)
                        | PendingRequest::BillingAccount(_)
                        | PendingRequest::BillingProviders(_)
                )
            });
            billing.generation += 1;
            billing.account = None;
            billing.endpoints = None;
            billing.providers = None;
            billing.deadline =
                Some(tokio::time::Instant::now() + std::time::Duration::from_secs(2));
            let id = next_id;
            next_id += 1;
            pending.insert(id, PendingRequest::BillingConfig(billing.generation));
            send_frame!(request_line(
                id,
                "config/read",
                serde_json::json!({"includeLayers": false})
            ));
            let id = next_id;
            next_id += 1;
            pending.insert(id, PendingRequest::BillingAccount(billing.generation));
            send_frame!(request_line(
                id,
                "account/read",
                serde_json::json!({"refreshToken": false})
            ));
            let id = next_id;
            next_id += 1;
            pending.insert(id, PendingRequest::BillingProviders(billing.generation));
            send_frame!(request_line(
                id,
                "modelProvider/list",
                serde_json::json!({})
            ));
        }
        if probe_done {
            if let Some(mut init) = pending_init.take() {
                if let AgentEvent::Init {
                    can_steer: capability,
                    ..
                } = &mut init
                {
                    *capability = can_steer;
                }
                if events_tx.send(init).is_err() {
                    break 'session;
                }
            }
        }
        if steer_input.is_none() {
            if let Some(params) = deferred_terminal.take() {
                for event in tracker.map_notification(
                    "turn/completed",
                    &params,
                    turn_start.elapsed().as_millis() as u64,
                ) {
                    if events_tx.send(event).is_err() {
                        shutdown = true;
                    }
                }
                turn_image_guards.clear();
                if shutdown {
                    break 'session;
                }
            }
        }
        if can_steer && turn_in_flight && !steer_blocked_for_turn && steer_input.is_none() {
            if let (Some(turn_id), Some(input)) = (tracker.current_turn_id.clone(), queue.front()) {
                let (paths, guards) = write_image_files(&input.images);
                let mut params = build_turn_start_params(
                    tracker.session_id.as_deref().unwrap_or_default(),
                    &input.text,
                    &paths,
                    None,
                    None,
                );
                params
                    .as_object_mut()
                    .expect("turn params object")
                    .retain(|key, _| key == "threadId" || key == "input");
                params["expectedTurnId"] = turn_id.into();
                turn_image_guards.extend(guards);
                let id = next_id;
                next_id += 1;
                pending.insert(id, PendingRequest::TurnSteer);
                steer_input = queue.pop_front();
                steer_deadline = Some(tokio::time::Instant::now() + HANDSHAKE_TIMEOUT);
                send_frame!(request_line(id, "turn/steer", params));
            }
        }
        // INVARIANT: A terminal arriving before its steer reply waits for the receipt.
        if thread_ready && probe_done && !turn_in_flight && steer_input.is_none() && billing.ready {
            if let Some(input) = queue.pop_front() {
                billing.ready = false;
                let input = session_context.led(input);
                let thread_id = tracker.session_id.clone().unwrap_or_default();
                let turn_effort = super::codex::validate_codex_effort(
                    config.model.as_deref(),
                    config.reasoning_effort.as_deref(),
                )
                .map(String::from);
                tracker.begin_turn(turn_effort.clone());
                steer_blocked_for_turn = false;
                turn_start = std::time::Instant::now();
                let (image_paths, guards) = write_image_files(&input.images);
                turn_image_guards = guards;
                let params = build_turn_start_params(
                    &thread_id,
                    &input.text,
                    &image_paths,
                    config.model.as_deref(),
                    turn_effort.as_deref(),
                );
                let id = next_id;
                next_id += 1;
                pending.insert(id, PendingRequest::TurnStart(input.uuid));
                turn_input = Some(input);
                let line = request_line(id, "turn/start", params);
                send_frame!(line);
                turn_in_flight = true;
            }
        }

        let billing_deadline = billing.deadline;
        tokio::select! {
            _ = async move {
                match billing_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            }, if billing_deadline.is_some() => {
                billing.deadline = None;
                billing.ready = true;
                let _ = events_tx.send(AgentEvent::BillingObserved {
                    attribution: super::billing_provider::BillingAttribution::default(),
                });
            }
            maybe_line = lines_rx.recv() => {
                let Some(line) = maybe_line else {
                    log_legacy!("[CodexAppServer] stdout EOF — child gone");
                    stdout_closed = true;
                    break 'session;
                };
                match parse_app_server_line(&line) {
                            AppServerLine::Response { id, result, error } => {
                                let Some(kind) = pending.remove(&id) else {
                                    log_legacy!("[CodexAppServer] response for unknown request id {}", id);
                                    continue;
                                };
                                match (kind, error) {
                                    (PendingRequest::BillingConfig(generation), error) => {
                                        if generation == billing.generation && billing.deadline.is_some() {
                                            billing.endpoints = Some(if error.is_none() {
                                                CodexEndpointConfig::read(&result)
                                            } else {
                                                CodexEndpointConfig::Unknown
                                            });
                                        }
                                    }
                                    (PendingRequest::BillingProviders(generation), error) => {
                                        if generation == billing.generation && billing.deadline.is_some() {
                                            billing.providers = Some(if error.is_none() {
                                                CodexProviderEndpoints::read(&result)
                                            } else {
                                                CodexProviderEndpoints::Unavailable
                                            });
                                        }
                                    }
                                    (PendingRequest::BillingAccount(generation), error) => {
                                        if generation == billing.generation && billing.deadline.is_some() {
                                            billing.account = Some(if error.is_none() {
                                                super::billing_provider::BillingAttribution::codex_account(
                                                    billing_model_provider.as_deref(), &result,
                                                )
                                            } else {
                                                super::billing_provider::BillingAttribution::default()
                                            });
                                        }
                                    }
                                    (PendingRequest::Initialize, None) => {
                                        log_legacy!(
                                            "[CodexAppServer] initialized: {}",
                                            result.get("userAgent").and_then(|v| v.as_str()).unwrap_or("?")
                                        );
                                        send_frame!(notification_line("initialized"));
                                        let probe_id = next_id;
                                        next_id += 1;
                                        pending.insert(probe_id, PendingRequest::SteerProbe);
                                        probe_deadline = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(2));
                                        // INVARIANT: Never probe a real thread: this cannot modify a live turn.
                                        send_frame!(request_line(probe_id, "turn/steer", serde_json::json!({
                                            "threadId": uuid::Uuid::new_v4().to_string(),
                                            "expectedTurnId": uuid::Uuid::new_v4().to_string(), "input": []
                                        })));
                                        let id = next_id;
                                        next_id += 1;
                                        pending.insert(id, PendingRequest::Thread);
                                        let (method, params) = build_thread_request(
                                            &config,
                                            resume_session_id.as_deref(),
                                        );
                                        send_frame!(request_line(id, method, params));
                                    }
                                    (PendingRequest::McpStatus, None) => {
                                        let Some((page, cursor)) = parse_mcp_server_statuses(&result) else {
                                            mcp_servers.clear();
                                            continue;
                                        };
                                        mcp_servers.extend(page);
                                        if let Some(cursor) = cursor {
                                            if !mcp_cursors.insert(cursor.to_owned()) {
                                                mcp_servers.clear();
                                                continue;
                                            }
                                            let id = next_id;
                                            next_id += 1;
                                            pending.insert(id, PendingRequest::McpStatus);
                                            send_frame!(request_line(id, "mcpServerStatus/list", serde_json::json!({
                                                "threadId": tracker.session_id, "detail": "toolsAndAuthOnly", "cursor": cursor,
                                            })));
                                        } else {
                                            let _ = events_tx.send(AgentEvent::McpServerStatuses(std::mem::take(&mut mcp_servers)));
                                        }
                                    }
                                    (PendingRequest::McpStatus, Some(_)) => mcp_servers.clear(),
                                    (PendingRequest::Thread, None) => {
                                        let thread_id = result
                                            .get("thread")
                                            .and_then(|t| t.get("id"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string();
                                        if thread_id.is_empty() {
                                            log_legacy!("[CodexAppServer] thread response carried no id — failing session");
                                            fail_session(&events_tx, &mut tracker, &mut input_rx, &mut queue, turn_in_flight,
                                                "codex app-server returned no thread id".to_string(),
                                                turn_start.elapsed().as_millis() as u64);
                                            break 'session;
                                        }
                                        let response_model = result
                                            .get("model")
                                            .and_then(|v| v.as_str())
                                            .map(str::to_string);
                                        tracker.note_resume_response(&result);
                                        let check_hook = config.env.iter().any(|(key, _)| key == "LUCIDOS_THREAD_ID");
                                        if check_hook {
                                            let id = next_id;
                                            next_id += 1;
                                            pending.insert(id, PendingRequest::CommandDenyHook);
                                            send_frame!(request_line(id, "hooks/list", serde_json::json!({"threadId": thread_id})));
                                        }
                                        let id = next_id;
                                        next_id += 1;
                                        pending.insert(id, PendingRequest::McpStatus);
                                        send_frame!(request_line(id, "mcpServerStatus/list", serde_json::json!({
                                            "threadId": thread_id, "detail": "toolsAndAuthOnly",
                                        })));
                                        for ev in tracker.note_thread_started(thread_id, response_model) {
                                            pending_init = Some(ev);
                                        }
                                        billing_model_provider = result.get("modelProvider")
                                            .or_else(|| result.get("thread").and_then(|t| t.get("modelProvider")))
                                            .and_then(|v| v.as_str()).map(String::from);
                                        thread_ready = !check_hook;
                                    }
                                    (PendingRequest::CommandDenyHook, error) => {
                                        if error.is_some() || !command_deny_hook_is_active(&result) {
                                            fail_session(&events_tx, &mut tracker, &mut input_rx, &mut queue, turn_in_flight,
                                                "Codex could not verify the standing deny hook. Update Codex and try again.".into(),
                                                turn_start.elapsed().as_millis() as u64);
                                            break 'session;
                                        }
                                        thread_ready = true;
                                    }
                                    (PendingRequest::TurnStart(input), None) => {
                                        if let Some(sent) =
                                            turn_input.take().filter(|sent| sent.uuid == input)
                                        {
                                            session_context.delivered(&sent);
                                        }
                                        // INVARIANT: Codex took the input. Reported only
                                        // now: a read is the turn gap's proof
                                        // of delivery, so it must not precede
                                        // the child accepting the turn.
                                        let _ = events_tx.send(AgentEvent::InputRead { input });
                                        if turn_in_flight && tracker.current_turn_id.is_none() {
                                            if let Some(turn_id) = result
                                                .get("turn")
                                                .and_then(|t| t.get("id"))
                                                .and_then(|v| v.as_str())
                                            {
                                                tracker.current_turn_id = Some(turn_id.to_string());
                                            }
                                        }
                                    }
                                    (PendingRequest::SteerProbe, error) => {
                                        if !probe_done {
                                            // INVARIANT: A handler-specific refusal proves the method exists.
                                            // Unknown methods (including old -32600 unknown-variant
                                            // replies) and inconclusive failures keep legacy routing.
                                            can_steer = error.as_deref().is_some_and(|message| message.to_ascii_lowercase().starts_with("thread not found"));
                                            probe_done = true;
                                            probe_deadline = None;
                                        }
                                    }
                                    (PendingRequest::TurnSteer, error) => {
                                        steer_deadline = None;
                                        if let Some(input) = steer_input.take() {
                                            if let Some(error) = error {
                                                log_legacy!("[CodexAppServer] turn/steer rejected: {}", error);
                                                let interrupting = turn_in_flight && !input.defer_on_steer_failure;
                                                if withdrawn_steer.take() == Some(input.uuid) {
                                                    let _ = events_tx.send(AgentEvent::InputRejected { input: input.uuid });
                                                } else {
                                                    queue.push_front(input);
                                                }
                                                steer_blocked_for_turn = true;
                                                let lower = error.to_ascii_lowercase();
                                                let mismatch = (lower.contains("expected") && lower.contains("turn")) || lower.contains("no active turn");
                                                if !mismatch {
                                                    can_steer = false;
                                                    let _ = events_tx.send(AgentEvent::SteerFallback { can_steer, interrupting });
                                                    if interrupting {
                                                        if let Some(turn_id) = tracker.current_turn_id.as_deref() {
                                                            let id = next_id;
                                                            next_id += 1;
                                                            pending.insert(id, PendingRequest::TurnInterrupt);
                                                            send_frame!(request_line(id, "turn/interrupt", serde_json::json!({"threadId": tracker.session_id, "turnId": turn_id})));
                                                            tracker.note_interrupt_requested();
                                                        }
                                                    }
                                                }
                                            } else {
                                                tracker.note_steer_accepted();
                                                let _ = events_tx.send(AgentEvent::InputRead { input: input.uuid });
                                            }
                                        }
                                    }
                                    (PendingRequest::TurnInterrupt, None) => {}
                                    (PendingRequest::Thread, Some(msg))
                                        if resume_session_id.is_some()
                                            && !tried_fresh_thread_fallback =>
                                    {
                                        tried_fresh_thread_fallback = true;
                                        session_context = SessionContextDue::for_spawn(
                                            None,
                                            config.session_context.as_ref(),
                                        );
                                        log_legacy!(
                                            "[CodexAppServer] thread/resume failed ({}) — falling back to a fresh thread",
                                            msg
                                        );
                                        let id = next_id;
                                        next_id += 1;
                                        pending.insert(id, PendingRequest::Thread);
                                        let (method, params) = build_thread_request(&config, None);
                                        send_frame!(request_line(id, method, params));
                                    }
                                    (PendingRequest::Initialize, Some(msg))
                                    | (PendingRequest::Thread, Some(msg)) => {
                                        log_legacy!("[CodexAppServer] handshake failed: {}", msg);
                                        fail_session(&events_tx, &mut tracker, &mut input_rx, &mut queue, turn_in_flight,
                                            format!("Codex app-server handshake failed: {msg}"),
                                            turn_start.elapsed().as_millis() as u64);
                                        break 'session;
                                    }
                                    (PendingRequest::TurnStart(input), Some(msg)) => {
                                        log_legacy!("[CodexAppServer] turn/start rejected: {}", msg);
                                        turn_input = None;
                                        // INVARIANT: Named before the `Result`, so the idle
                                        // that follows owes nothing for it.
                                        let _ = events_tx.send(AgentEvent::InputRejected { input });
                                        for ev in tracker.close_open_tools() {
                                            let _ = events_tx.send(ev);
                                        }
                                        let _ = events_tx.send(AgentEvent::Result {
                                            text: String::new(),
                                            duration_ms: turn_start.elapsed().as_millis() as u64,
                                            error: Some(format!("Codex rejected the turn: {msg}")),
                                            provider_error: None,
                                            turn_end: Some(tracker.turn_end_facts(TurnEndStatus::Failed)),
                                        });
                                        turn_in_flight = false;
                                        turn_image_guards.clear();
                                    }
                                    (PendingRequest::TurnInterrupt, Some(msg)) => {
                                        log_legacy!("[CodexAppServer] turn/interrupt rejected: {}", msg);
                                    }
                                }
                            }
                            AppServerLine::Notification { method, params } => {
                                if method == "turn/completed" && tracker.owns(&method, &params) && steer_input.is_some() {
                                    turn_in_flight = false;
                                    deferred_terminal = Some(params);
                                    continue;
                                }
                                let events = tracker.map_notification(
                                    &method,
                                    &params,
                                    turn_start.elapsed().as_millis() as u64,
                                );
                                for ev in events {
                                    if matches!(ev, AgentEvent::Init { .. }) {
                                        pending_init = Some(ev);
                                        continue;
                                    }
                                    if events_tx.send(ev).is_err() {
                                        shutdown = true;
                                        break;
                                    }
                                }
                                if shutdown {
                                    break 'session;
                                }
                                if method == "turn/completed" && tracker.owns(&method, &params) {
                                    turn_in_flight = false;
                                    turn_image_guards.clear();
                                }
                            }
                            AppServerLine::ServerRequest { id, method, params } => {
                                handle_server_request(
                                    id,
                                    &method,
                                    &params,
                                    &permission_tx,
                                    &driver_action_tx,
                                    &mut approval_tasks,
                                    &mut tracker,
                                );
                            }
                            AppServerLine::Other => {}
                }
            }
            Some(action) = driver_action_rx.recv() => {
                match action {
                    DriverAction::SendLine(line) => {
                        send_frame!(line);
                    }
                    DriverAction::ApprovalResolved { line, item_id, allowed } => {
                        send_frame!(line);
                        let events = tracker.note_approval_resolved(&item_id, allowed);
                        for ev in events {
                            if events_tx.send(ev).is_err() {
                                shutdown = true;
                                break;
                            }
                        }
                        if shutdown {
                            break 'session;
                        }
                    }
                }
            }
            input = input_rx.recv() => {
                match input {
                    Some(i) => queue.push_back(i),
                    None => {
                        shutdown = true;
                        break 'session;
                    }
                }
            }
            Some(withdraw) = withdraw_rx.recv() => {
                if steer_input.as_ref().is_some_and(|input| input.uuid == withdraw.input_uuid && input.defer_on_steer_failure) {
                    // INVARIANT: Already in the RPC: it may have joined the active turn.
                    // If refused later, it must not become a new turn after Stop.
                    withdrawn_steer = Some(withdraw.input_uuid);
                    let _ = withdraw.reply.send(InputWithdrawal::AlreadyRead);
                } else {
                    answer_withdraw(&mut queue, &mut input_rx, withdraw);
                }
            }
            req = control_rx.recv() => {
                match req {
                    Some(ControlRequest::Interrupt) => {
                        match (turn_in_flight, tracker.current_turn_id.clone(), tracker.session_id.clone()) {
                            (true, Some(turn_id), Some(thread_id)) => {
                                log_legacy!("[CodexAppServer] interrupt — sending turn/interrupt for turn {}", turn_id);
                                let id = next_id;
                                next_id += 1;
                                pending.insert(id, PendingRequest::TurnInterrupt);
                                tracker.note_interrupt_requested();
                                let line = request_line(id, "turn/interrupt", serde_json::json!({
                                    "threadId": thread_id,
                                    "turnId": turn_id,
                                }));
                                send_frame!(line);
                                // INVARIANT: The turn ends via `turn/completed {status:
                                // interrupted}` → tracker synthesizes the
                                // error-free Result the engine's stop latch
                                // turns into Canceled. If codex ignores the
                                // request, the engine's 8s escalation cancels
                                // our token and the hard-kill path below runs.
                            }
                            (true, None, _) => {
                                log_legacy!("[CodexAppServer] interrupt requested but no turn id known yet — engine escalation will hard-stop");
                            }
                            _ => {}
                        }
                    }
                    None => {
                        shutdown = true;
                        break 'session;
                    }
                }
            }
            _ = cancel.cancelled() => {
                log_legacy!("[CodexAppServer] cancellation signalled — killing codex app-server");
                shutdown = true;
                break 'session;
            }
            wait_result = child.wait(), if child_death_drain_deadline.is_none() => {
                // INVARIANT: Always-on child exit handler — same lesson as the CC
                // driver: a grandchild that inherited stdout can hold the
                // pipe open after the child dies, so EOF alone can't be the
                // only death signal. Don't break yet: lines the child
                // flushed before dying (a final turn/completed) are still
                // buffered in lines_rx and flow through the normal arm;
                // wind down when the reader hits EOF or the bounded drain
                // deadline below passes.
                log_legacy!(
                    "[CodexAppServer] app-server child died (status={}) — draining remaining stdout",
                    format_exit_status(&wait_result),
                );
                natural_exit_status = wait_result.ok();
                child_reaped = true;
                child_death_drain_deadline = Some(
                    tokio::time::Instant::now() + std::time::Duration::from_millis(500),
                );
            }
            _ = async {
                match child_death_drain_deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                break 'session;
            }
            _ = async {
                match probe_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                probe_done = true;
                probe_deadline = None;
                log_legacy!("[CodexAppServer] steer probe timed out; retaining interrupt-and-redirect");
            }
            _ = async {
                match steer_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                if steer_input.as_ref().is_some_and(|input| input.defer_on_steer_failure) {
                    if let Some(input) = steer_input.take() {
                        let _ = events_tx.send(AgentEvent::InputRejected { input: input.uuid });
                    }
                    pending.retain(|_, request| !matches!(request, PendingRequest::TurnSteer));
                    steer_deadline = None;
                    can_steer = false;
                    let _ = events_tx.send(AgentEvent::SteerFallback { can_steer, interrupting: false });
                    continue 'session;
                }
                fail_session(&events_tx, &mut tracker, &mut input_rx, &mut queue, true,
                    "Codex did not acknowledge the follow-up steer".to_string(), turn_start.elapsed().as_millis() as u64);
                break 'session;
            }
            _ = tokio::time::sleep_until(handshake_deadline), if !thread_ready => {
                log_legacy!("[CodexAppServer] handshake timed out after {:?}", HANDSHAKE_TIMEOUT);
                fail_session(&events_tx, &mut tracker, &mut input_rx, &mut queue, turn_in_flight,
                    "Codex app-server did not complete its handshake".to_string(),
                    turn_start.elapsed().as_millis() as u64);
                break 'session;
            }
        }
    }

    drop(approval_tasks);

    if stdout_closed && !child_reaped {
        if let Ok(Ok(status)) =
            tokio::time::timeout(EXIT_AFTER_STDOUT_EOF_GRACE, child.wait()).await
        {
            natural_exit_status = Some(status);
            child_reaped = true;
        }
    }

    // INVARIANT: Reap the child. The persistent process has no clean-exit path of its
    // own, so the driver kills it on wind-down (a no-op if it already died).
    // It is its own process-group leader (`spawn_below_engine` at spawn),
    // so signalling only the leader would orphan everything the session
    // spawned; tear the group down first, as the CC driver does.
    //
    // Only while the child is unreaped, which is the kill helper's own
    // precondition. It waits out its grace before the SIGKILL. A pid freed by
    // the reap can be recycled inside that grace, so the signal could land on
    // an unrelated process group. The CC driver guards the same call the same
    // way.
    //
    // The cost is real, so do not "fix" it by dropping the guard. A descendant
    // outliving the reaped leader is left orphaned, and the drain deadline
    // above only stops us waiting on it. We take that over a stray SIGKILL,
    // which on this host can reach another workspace's engine.
    #[cfg(unix)]
    if !child_reaped {
        if let Some(pid) = child_pid {
            super::spawn_env::graceful_kill_child_process_group(
                pid,
                super::claude_code::GROUP_TEARDOWN_GRACE,
            )
            .await;
        }
    }
    let _ = child.start_kill();
    let wait_result =
        match tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await {
            Ok(r) => r,
            Err(_) => {
                let _ = child.kill().await;
                child.wait().await
            }
        };

    let stderr_text = stderr.finish().await;
    if !stderr_text.is_empty() {
        log_legacy!("[CodexAppServer] codex stderr: {}", stderr_text);
    }
    if !child_reaped {
        log_legacy!(
            "[CodexAppServer] app-server child exited (pid={} status={})",
            child_pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "?".to_string()),
            format_exit_status(&wait_result),
        );
    }

    if (turn_in_flight || deferred_terminal.is_some()) && !tracker.turn_terminal_seen && !shutdown {
        for ev in tracker.close_open_tools() {
            let _ = events_tx.send(ev);
        }
        let reason = tracker
            .last_error
            .clone()
            .or_else(|| (!stderr_text.is_empty()).then(|| stderr_text.clone()))
            .unwrap_or_else(|| {
                format!(
                    "codex app-server exited unexpectedly ({})",
                    format_exit_status(&wait_result)
                )
            });
        let _ = events_tx.send(AgentEvent::Result {
            text: tracker.turn_text(),
            duration_ms: turn_start.elapsed().as_millis() as u64,
            error: Some(reason),
            provider_error: tracker.last_provider_error.clone(),
            turn_end: Some(tracker.turn_end_facts(TurnEndStatus::Synthesized)),
        });
    }

    // INVARIANT: Codex's app-server interrupt is graceful, so the stray-SIGTERM
    // auto-resume path stays CC-only: `killed_by_signal` is never set here.
    let _ = events_tx.send(AgentEvent::Exited {
        exit: AgentExitFacts {
            killed_by_signal: false,
            code: natural_exit_status.and_then(|status| status.code()),
            stderr_tail: stderr_text,
        },
    });
}

/// INVARIANT: Synthesize the failed Result for a session that died before serving its
/// accepted input (handshake failure / timeout). Drains any inputs still
/// sitting un-read in `input_rx` into `queue` first, so an input racing the
/// handshake failure still counts as accepted — the engine expects a Result
/// for it and would otherwise classify the turn with a generic abort
/// instead of the actionable handshake reason. Skipped entirely when no
/// input was accepted (the engine isn't waiting on a Result then).
fn fail_session(
    events_tx: &mpsc::UnboundedSender<AgentEvent>,
    tracker: &mut AppServerTracker,
    input_rx: &mut mpsc::UnboundedReceiver<AgentInput>,
    queue: &mut VecDeque<AgentInput>,
    turn_in_flight: bool,
    reason: String,
    duration_ms: u64,
) {
    while let Ok(i) = input_rx.try_recv() {
        queue.push_back(i);
    }
    if !turn_in_flight && queue.is_empty() {
        return;
    }
    for ev in tracker.close_open_tools() {
        let _ = events_tx.send(ev);
    }
    tracker.turn_terminal_seen = true;
    let _ = events_tx.send(AgentEvent::Result {
        text: tracker.turn_text(),
        duration_ms,
        error: Some(reason),
        provider_error: None,
        turn_end: Some(tracker.turn_end_facts(TurnEndStatus::Synthesized)),
    });
}

/// INVARIANT: Answer one server→client request. Approvals bridge to the engine's
/// permission machinery via `permission_tx`; a per-request waiter task
/// (spawned into the driver's JoinSet so wind-down aborts it) awaits the
/// user's decision and queues the JSON-RPC response on `driver_action_tx` so the
/// driver loop never blocks on the user. Unknown methods get a JSON-RPC
/// error immediately — leaving them unanswered would wedge codex.
fn handle_server_request(
    id: serde_json::Value,
    method: &str,
    params: &serde_json::Value,
    permission_tx: &mpsc::UnboundedSender<AgentPermissionRequest>,
    driver_action_tx: &mpsc::UnboundedSender<DriverAction>,
    approval_tasks: &mut tokio::task::JoinSet<()>,
    tracker: &mut AppServerTracker,
) {
    let Some(mut approval) = parse_approval_request(method, params) else {
        log_legacy!("[CodexAppServer] unsupported server request: {}", method);
        let _ = driver_action_tx.send(DriverAction::SendLine(error_response_line(
            &id,
            -32601,
            &format!("Method not supported by Lucidos: {method}"),
        )));
        return;
    };
    tracker.attach_known_file_changes(&mut approval);
    tracker.note_approval_request(&approval);

    let (respond_tx, respond_rx) = tokio::sync::oneshot::channel::<bool>();
    let item_id = approval.item_id.clone();
    let forwarded = permission_tx.send(AgentPermissionRequest {
        id: approval.item_id,
        tool_name: approval.tool_name,
        input: approval.input,
        respond: respond_tx,
    });
    if forwarded.is_err() {
        let _ = driver_action_tx.send(DriverAction::ApprovalResolved {
            line: response_line(&id, serde_json::json!({ "decision": "decline" })),
            item_id,
            allowed: false,
        });
        return;
    }

    let driver_action_tx = driver_action_tx.clone();
    approval_tasks.spawn(async move {
        let allowed = respond_rx.await.unwrap_or(false);
        let decision = if allowed { "accept" } else { "decline" };
        let _ = driver_action_tx.send(DriverAction::ApprovalResolved {
            line: response_line(&id, serde_json::json!({ "decision": decision })),
            item_id,
            allowed,
        });
    });
}

#[cfg(test)]
#[path = "codex_app_server_tests/parsing.rs"]
mod parsing_tests;

#[cfg(test)]
#[path = "codex_app_server_tests/requests.rs"]
mod requests_tests;

#[cfg(test)]
#[path = "codex_app_server_tests/driver.rs"]
mod driver_tests;
