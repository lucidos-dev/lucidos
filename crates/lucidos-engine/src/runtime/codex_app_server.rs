//! Persistent-process Codex driver speaking the `codex app-server` JSON-RPC
//! protocol (ADR 0005). One `codex app-server` child per session; each
//! accepted `AgentInput` becomes one `turn/start` against the same thread.
//!
//! What this buys over the per-turn exec driver (`codex.rs`):
//! - **Permission cards** — `approvalPolicy: on-request` raises
//!   `item/commandExecution/requestApproval` / `item/fileChange/requestApproval`
//!   server requests, forwarded to the engine over
//!   `RunningAgent::permission_rx` and answered from the PermissionCard.
//! - **Per-token streaming** — `item/agentMessage/delta` maps onto
//!   `AgentEvent::Message`; the engine's buffer/flush loop already handles
//!   arbitrary chunk sizes.
//! - **Graceful interrupt** — `turn/interrupt` ends the turn with
//!   `turn/completed {status: interrupted}` instead of a child kill, so
//!   partial work survives. The engine's 8s escalation still hard-kills via
//!   the cancellation token if codex ignores it.
//!
//! Protocol mapping lives in `codex_app_server_parse.rs`. Lifecycle contract
//! honored here (see `agent_runtime.rs`), same as the exec driver:
//! - `Init` once, from the `thread/start` / `thread/resume` response (the
//!   Codex thread id is the engine's resume handle — same id space the exec
//!   driver resumes, so a thread survives a protocol flip).
//! - One `Result` per accepted input — synthesized on child death without a
//!   turn terminal.
//! - `Exited` exactly once, when the driver winds down.

use std::collections::{HashMap, VecDeque};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::agent_runtime::{AgentEvent, AgentInput, AgentPermissionRequest, ControlRequest};
use super::claude_code::format_exit_status;
use super::codex::{
    lucidos_mcp_server_config_json, write_image_files, CodexConfig, CONTINUATION_PROMPT,
};
use super::codex_app_server_parse::{
    parse_app_server_line, parse_approval_request, AppServerLine, AppServerTracker,
};

/// How long the handshake (initialize → thread established) may take before
/// the driver gives up and synthesizes a failure. Generous — a cold codex
/// start is a couple of seconds; only a wedged binary hits this.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Requests this driver sends, tracked by JSON-RPC id so the response can be
/// routed back to the right state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingRequest {
    Initialize,
    Thread,
    TurnStart,
    TurnInterrupt,
}

enum DriverAction {
    SendLine(String),
    ApprovalResolved {
        line: String,
        item_id: String,
        allowed: bool,
    },
}

/// Serialize one outbound JSON-RPC frame as a newline-terminated string.
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

/// Build the `thread/start` (fresh) or `thread/resume` params. Pure so the
/// unit tests can pin the shape without a child.
///
/// `developerInstructions` carries the engine's system prompt on BOTH paths
/// (matching CC, which re-passes `--append-system-prompt` on every resume).
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
    let config_obj = serde_json::json!({
        "sandbox_workspace_write": sandbox_ww,
        "mcp_servers": { "lucidos": lucidos_mcp_server_config_json(&config.env) },
        // Reasoning summaries + CLAUDE.md project-doc fallback — see the
        // CODEX_REASONING_SUMMARY / CODEX_PROJECT_DOC_FALLBACKS docs in
        // codex.rs (shared with the exec driver's `-c` overrides).
        "model_reasoning_summary": super::codex::CODEX_REASONING_SUMMARY,
        "project_doc_fallback_filenames": super::codex::CODEX_PROJECT_DOC_FALLBACKS,
        "project_doc_max_bytes": super::codex::CODEX_PROJECT_DOC_MAX_BYTES,
    });

    let mut params = serde_json::Map::new();
    params.insert(
        "cwd".to_string(),
        config.worktree_path.to_string_lossy().into_owned().into(),
    );
    params.insert("sandbox".to_string(), "workspace-write".into());
    // The point of this driver: sandbox-escaping commands raise a card
    // instead of failing silently (the exec escape hatch keeps `never`).
    params.insert("approvalPolicy".to_string(), "on-request".into());
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

/// Build `turn/start` params for one accepted input. Pure for tests.
fn build_turn_start_params(
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

/// Spawn the persistent `codex app-server` child.
fn spawn_app_server_child(config: &CodexConfig) -> std::io::Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new(&config.codex_bin);
    cmd.arg("app-server")
        .current_dir(&config.worktree_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    // Own process group so a group-wide signal to the engine can't kill the
    // app-server child — same isolation CC gets. (Codex's env is baked from a
    // probe, so this Command attribute must be set on the real command.)
    super::spawn_env::isolate_in_process_group(&mut cmd);
    cmd.spawn()
}

/// Drive one persistent app-server session. See module docs for the contract.
#[allow(clippy::too_many_arguments)]
pub(super) async fn app_server_driver_task(
    config: CodexConfig,
    resume_session_id: Option<String>,
    continuation: bool,
    events_tx: mpsc::UnboundedSender<AgentEvent>,
    mut input_rx: mpsc::UnboundedReceiver<AgentInput>,
    mut control_rx: mpsc::UnboundedReceiver<ControlRequest>,
    permission_tx: mpsc::UnboundedSender<AgentPermissionRequest>,
    cancel: CancellationToken,
) {
    let mut child = match spawn_app_server_child(&config) {
        Ok(c) => c,
        Err(e) => {
            log!("[CodexAppServer] failed to spawn codex app-server: {}", e);
            let _ = events_tx.send(AgentEvent::Result {
                text: String::new(),
                duration_ms: 0,
                error: Some(format!("Failed to start Codex: {e}")),
            });
            // Codex doesn't classify signal-kills — its app-server interrupt is
            // graceful, so the stray-SIGTERM auto-resume path is CC-only.
            let _ = events_tx.send(AgentEvent::Exited {
                killed_by_signal: false,
            });
            return;
        }
    };
    let child_pid = child.id();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // Cancel-safe stdout: a dedicated reader task feeds whole lines into an
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
        // lines_tx drops — the driver sees recv() = None as stdout EOF.
    });

    // The persistent child's stderr must be drained CONTINUOUSLY — unlike
    // the per-turn exec children, this process lives for a whole turn-set,
    // and an undrained 64KB pipe would eventually block codex mid-write,
    // silencing stdout and reading as a hang. Keep a bounded tail for the
    // wind-down diagnostic log.
    let stderr_tail: std::sync::Arc<std::sync::Mutex<VecDeque<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(VecDeque::new()));
    {
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            const TAIL_BUDGET: usize = 4096;
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let mut t = tail.lock().unwrap();
                        t.push_back(line.clone());
                        while t.iter().map(String::len).sum::<usize>() > TAIL_BUDGET && t.len() > 1
                        {
                            t.pop_front();
                        }
                    }
                }
            }
        });
    }

    // Driver action queue — approval-response tasks and the main loop share it
    // so stdin writes and approval-state updates stay ordered in one place.
    let (driver_action_tx, mut driver_action_rx) = mpsc::unbounded_channel::<DriverAction>();
    // Approval waiter tasks live in a JoinSet so driver wind-down ABORTS
    // them: aborting drops their `respond_rx`, which fires the engine
    // waiter's `respond.closed()` arm, which drops the broadcast receiver,
    // which lets `gc_dead_entries` evict the pending card entry. A bare
    // `tokio::spawn` would leave the waiter pair alive forever after a child
    // crash (circular wait: task waits on engine, engine waits on user).
    let mut approval_tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    let mut tracker = AppServerTracker::new(resume_session_id.clone());
    let mut next_id: u64 = 1;
    let mut pending: HashMap<u64, PendingRequest> = HashMap::new();
    let mut model = config.model.clone();
    let mut effort = config.reasoning_effort.clone();
    let mut queue: VecDeque<AgentInput> = VecDeque::new();
    if continuation {
        // Engine resumes a mid-turn-interrupted session with no new input —
        // same synthetic prompt the exec driver injects.
        queue.push_back(AgentInput {
            text: CONTINUATION_PROMPT.to_string(),
            images: Vec::new(),
        });
    }
    // True once the thread response has established the session (the id itself
    // lives on `tracker.session_id`). Turns can start only once this is true.
    let mut thread_ready = false;
    // One-shot guard for the thread/resume → thread/start fallback below.
    let mut tried_fresh_thread_fallback = false;
    let mut turn_in_flight = false;
    let mut turn_start = std::time::Instant::now();
    // Keep the current turn's temp image files alive until the turn ends.
    let mut turn_image_guards: Vec<tempfile::TempPath> = Vec::new();
    let mut shutdown = false;
    // Set when child.wait() fires: lines may still be buffered in lines_rx —
    // keep processing them through the NORMAL arm until the reader task hits
    // EOF (recv = None) or this bounded deadline passes (a grandchild
    // holding the pipe open would otherwise stall the wind-down forever).
    let mut child_death_drain_deadline: Option<tokio::time::Instant> = None;
    // True once the `child.wait()` arm below has reaped the child. It gates the
    // teardown's process-group kill, so it names the reaping rather than the
    // logging it also drives.
    let mut child_reaped = false;
    let handshake_deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;

    // All stdin writes go through this macro: bounded so a wedged child
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
                Ok(Err(e)) => log!("[CodexAppServer] stdin write failed — child gone? {}", e),
                Err(_) => log!(
                    "[CodexAppServer] stdin write timed out — child wedged with a full pipe?"
                ),
            }
        };
    }

    // Kick off the handshake.
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
                }
            }),
        );
        send_frame!(line);
    }

    'session: while !shutdown {
        // Start the next queued turn whenever the thread is ready and idle.
        if thread_ready && !turn_in_flight {
            if let Some(input) = queue.pop_front() {
                let thread_id = tracker.session_id.clone().unwrap_or_default();
                tracker.begin_turn();
                turn_start = std::time::Instant::now();
                let (image_paths, guards) = write_image_files(&input.images);
                turn_image_guards = guards;
                let params = build_turn_start_params(
                    &thread_id,
                    &input.text,
                    &image_paths,
                    model.as_deref(),
                    effort.as_deref(),
                );
                let id = next_id;
                next_id += 1;
                pending.insert(id, PendingRequest::TurnStart);
                let line = request_line(id, "turn/start", params);
                send_frame!(line);
                turn_in_flight = true;
            }
        }

        tokio::select! {
            maybe_line = lines_rx.recv() => {
                let Some(line) = maybe_line else {
                    // Reader task hit stdout EOF — child gone (or wound down
                    // after child.wait fired; everything buffered has been
                    // processed by this arm already).
                    log!("[CodexAppServer] stdout EOF — child gone");
                    break 'session;
                };
                match parse_app_server_line(&line) {
                            AppServerLine::Response { id, result, error } => {
                                let Some(kind) = pending.remove(&id) else {
                                    log!("[CodexAppServer] response for unknown request id {}", id);
                                    continue;
                                };
                                match (kind, error) {
                                    (PendingRequest::Initialize, None) => {
                                        // Log the negotiated handshake so a codex
                                        // upgrade that breaks the (experimental)
                                        // contract is diagnosable from logs.
                                        log!(
                                            "[CodexAppServer] initialized: {}",
                                            result.get("userAgent").and_then(|v| v.as_str()).unwrap_or("?")
                                        );
                                        send_frame!(notification_line("initialized"));
                                        let id = next_id;
                                        next_id += 1;
                                        pending.insert(id, PendingRequest::Thread);
                                        let (method, params) = build_thread_request(
                                            &config,
                                            resume_session_id.as_deref(),
                                        );
                                        send_frame!(request_line(id, method, params));
                                    }
                                    (PendingRequest::Thread, None) => {
                                        let thread_id = result
                                            .get("thread")
                                            .and_then(|t| t.get("id"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string();
                                        if thread_id.is_empty() {
                                            log!("[CodexAppServer] thread response carried no id — failing session");
                                            fail_session(&events_tx, &mut tracker, &mut input_rx, &mut queue, turn_in_flight,
                                                "codex app-server returned no thread id".to_string(),
                                                turn_start.elapsed().as_millis() as u64);
                                            break 'session;
                                        }
                                        let response_model = result
                                            .get("model")
                                            .and_then(|v| v.as_str())
                                            .map(str::to_string);
                                        for ev in tracker.note_thread_started(thread_id, response_model) {
                                            if events_tx.send(ev).is_err() {
                                                break 'session;
                                            }
                                        }
                                        thread_ready = true;
                                    }
                                    (PendingRequest::TurnStart, None) => {
                                        // The turn id for interrupts. Guarded:
                                        // a LATE response (turn already ended,
                                        // next turn possibly started) must not
                                        // overwrite the live turn's id with a
                                        // dead one — a subsequent interrupt
                                        // would then target the finished turn
                                        // and the real one would only stop via
                                        // the 8s hard-kill escalation.
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
                                    (PendingRequest::TurnInterrupt, None) => {}
                                    (PendingRequest::Thread, Some(msg))
                                        if resume_session_id.is_some()
                                            && !tried_fresh_thread_fallback =>
                                    {
                                        // Stale resume: the stored Codex thread id
                                        // no longer resolves (~/.codex pruned,
                                        // machine moved). Fall back to a FRESH
                                        // thread once — conversation context still
                                        // arrives via the THREAD HISTORY block the
                                        // engine appends to the system prompt, the
                                        // same recovery shape CC's stale-resume
                                        // path uses. Without this, a thread whose
                                        // rollout is gone wedges permanently on
                                        // "handshake failed" (the engine keeps
                                        // resolving the same dead sid from the
                                        // pending change's branch context).
                                        tried_fresh_thread_fallback = true;
                                        log!(
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
                                        log!("[CodexAppServer] handshake failed: {}", msg);
                                        fail_session(&events_tx, &mut tracker, &mut input_rx, &mut queue, turn_in_flight,
                                            format!("Codex app-server handshake failed: {msg}"),
                                            turn_start.elapsed().as_millis() as u64);
                                        break 'session;
                                    }
                                    (PendingRequest::TurnStart, Some(msg)) => {
                                        log!("[CodexAppServer] turn/start rejected: {}", msg);
                                        for ev in tracker.close_open_tools() {
                                            let _ = events_tx.send(ev);
                                        }
                                        let _ = events_tx.send(AgentEvent::Result {
                                            text: String::new(),
                                            duration_ms: turn_start.elapsed().as_millis() as u64,
                                            error: Some(format!("Codex rejected the turn: {msg}")),
                                        });
                                        turn_in_flight = false;
                                        turn_image_guards.clear();
                                    }
                                    (PendingRequest::TurnInterrupt, Some(msg)) => {
                                        // Turn may have completed before the
                                        // interrupt landed — benign race.
                                        log!("[CodexAppServer] turn/interrupt rejected: {}", msg);
                                    }
                                }
                            }
                            AppServerLine::Notification { method, params } => {
                                let events = tracker.map_notification(
                                    &method,
                                    &params,
                                    turn_start.elapsed().as_millis() as u64,
                                );
                                for ev in events {
                                    if events_tx.send(ev).is_err() {
                                        shutdown = true;
                                        break;
                                    }
                                }
                                if shutdown {
                                    break 'session;
                                }
                                if method == "turn/completed" {
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
                    // Queue — the pre-select block above starts it as soon as
                    // the thread is ready and no turn is in flight. No
                    // mid-turn injection (parity with the exec driver; every
                    // accepted input gets its own turn and its own Result). A
                    // user follow-up that should redirect a live turn is handled
                    // engine-side: the fast-path fires `turn/interrupt` first
                    // (ADR 0005 addendum), so by the time the queued input is
                    // dequeued the interrupted turn has ended.
                    Some(i) => queue.push_back(i),
                    None => {
                        shutdown = true;
                        break 'session;
                    }
                }
            }
            req = control_rx.recv() => {
                match req {
                    Some(ControlRequest::Interrupt) => {
                        match (turn_in_flight, tracker.current_turn_id.clone(), tracker.session_id.clone()) {
                            (true, Some(turn_id), Some(thread_id)) => {
                                log!("[CodexAppServer] interrupt — sending turn/interrupt for turn {}", turn_id);
                                let id = next_id;
                                next_id += 1;
                                pending.insert(id, PendingRequest::TurnInterrupt);
                                let line = request_line(id, "turn/interrupt", serde_json::json!({
                                    "threadId": thread_id,
                                    "turnId": turn_id,
                                }));
                                send_frame!(line);
                                // The turn ends via `turn/completed {status:
                                // interrupted}` → tracker synthesizes the
                                // error-free Result the engine's stop latch
                                // turns into Canceled. If codex ignores the
                                // request, the engine's 8s escalation cancels
                                // our token and the hard-kill path below runs.
                            }
                            (true, None, _) => {
                                log!("[CodexAppServer] interrupt requested but no turn id known yet — engine escalation will hard-stop");
                            }
                            _ => {} // idle — nothing to interrupt
                        }
                    }
                    Some(ControlRequest::SetModel { model: m }) => model = Some(m),
                    Some(ControlRequest::SetReasoningEffort { effort: e }) => effort = Some(e),
                    Some(ControlRequest::SetPermissionMode { .. }) => {
                        log!("[CodexAppServer] SetPermissionMode is a no-op for the Codex backend");
                    }
                    None => {
                        shutdown = true;
                        break 'session;
                    }
                }
            }
            _ = cancel.cancelled() => {
                log!("[CodexAppServer] cancellation signalled — killing codex app-server");
                shutdown = true;
                break 'session;
            }
            wait_result = child.wait(), if child_death_drain_deadline.is_none() => {
                // Always-on child exit handler — same lesson as the CC
                // driver: a grandchild that inherited stdout can hold the
                // pipe open after the child dies, so EOF alone can't be the
                // only death signal. Don't break yet: lines the child
                // flushed before dying (a final turn/completed) are still
                // buffered in lines_rx and flow through the normal arm;
                // wind down when the reader hits EOF or the bounded drain
                // deadline below passes.
                log!(
                    "[CodexAppServer] app-server child died (status={}) — draining remaining stdout",
                    format_exit_status(&wait_result),
                );
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
                // Grandchild kept stdout open past the child's death — stop
                // waiting for an EOF that isn't coming.
                break 'session;
            }
            _ = tokio::time::sleep_until(handshake_deadline), if !thread_ready => {
                log!("[CodexAppServer] handshake timed out after {:?}", HANDSHAKE_TIMEOUT);
                fail_session(&events_tx, &mut tracker, &mut input_rx, &mut queue, turn_in_flight,
                    "Codex app-server did not complete its handshake".to_string(),
                    turn_start.elapsed().as_millis() as u64);
                break 'session;
            }
        }
    }

    // Abort any approval waiter tasks still pending — dropping the JoinSet
    // drops their respond_rx, which releases the engine-side waiters (see
    // the JoinSet comment above).
    drop(approval_tasks);

    // Reap the child. The persistent process has no clean-exit path of its
    // own, so the driver kills it on wind-down (a no-op if it already died).
    // It is its own process-group leader (`isolate_in_process_group` at spawn),
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
                std::time::Duration::from_secs(3),
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

    let stderr_text: String = {
        let tail = stderr_tail.lock().unwrap();
        tail.iter().map(String::as_str).collect()
    };
    if !stderr_text.trim().is_empty() {
        log!("[CodexAppServer] codex stderr: {}", stderr_text.trim());
    }
    if !child_reaped {
        log!(
            "[CodexAppServer] app-server child exited (pid={} status={})",
            child_pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "?".to_string()),
            format_exit_status(&wait_result),
        );
    }

    // A turn that never saw its terminal (child died / stdout EOF mid-turn)
    // leaves the engine waiting on a Result — synthesize the failure. The
    // deliberate-shutdown paths (cancel token, closed channels) skip this:
    // the engine isn't waiting (idle termination / engine shutdown).
    if turn_in_flight && !tracker.turn_terminal_seen && !shutdown {
        for ev in tracker.close_open_tools() {
            let _ = events_tx.send(ev);
        }
        let reason = tracker
            .last_error
            .clone()
            .or_else(|| {
                let t = stderr_text.trim();
                (!t.is_empty()).then(|| t.to_string())
            })
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
        });
    }

    // Codex's app-server interrupt is graceful, so the stray-SIGTERM
    // auto-resume path stays CC-only — no signal classification here.
    let _ = events_tx.send(AgentEvent::Exited {
        killed_by_signal: false,
    });
    // events_tx drops here — channel closes, consumer sees None.
}

/// Synthesize the failed Result for a session that died before serving its
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
    // Mark the terminal as seen so the post-loop synthesis doesn't emit a
    // SECOND failed Result for the same accepted input.
    tracker.turn_terminal_seen = true;
    let _ = events_tx.send(AgentEvent::Result {
        text: tracker.turn_text(),
        duration_ms,
        error: Some(reason),
    });
}

/// Answer one server→client request. Approvals bridge to the engine's
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
        log!("[CodexAppServer] unsupported server request: {}", method);
        let _ = driver_action_tx.send(DriverAction::SendLine(error_response_line(
            &id,
            -32601,
            &format!("Method not supported by Lucidos: {method}"),
        )));
        return;
    };
    // A file-change approval arrives with no paths on it; the tracker saw them
    // on the item's `item/started` and hands them over here, so the permission
    // card can name what is being written.
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
        // Engine side gone (session loop ended) — decline so codex can wind
        // the item down instead of hanging.
        let _ = driver_action_tx.send(DriverAction::ApprovalResolved {
            line: response_line(&id, serde_json::json!({ "decision": "decline" })),
            item_id,
            allowed: false,
        });
        return;
    }

    let driver_action_tx = driver_action_tx.clone();
    approval_tasks.spawn(async move {
        // A dropped sender (engine loop ended without answering) reads as
        // deny — same default the engine's own broadcast path uses.
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
