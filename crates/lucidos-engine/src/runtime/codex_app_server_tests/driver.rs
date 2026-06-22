use super::*;
use crate::runtime::agent_runtime::{
    AgentEvent, AgentInput, AgentPermissionRequest, ControlRequest,
};
use crate::runtime::codex::CodexConfig;
use std::path::{Path, PathBuf};

/// Stand up the app-server driver over a stub shell script that speaks just
/// enough line-delimited JSON-RPC: it logs every inbound request to
/// `requests.log` and answers by matching on the `method` substring,
/// echoing back the request's id. The same seam the exec driver's tests use,
/// upgraded from "print a canned body" to "respond per request".
struct StubSession {
    _tmp: tempfile::TempDir,
    requests_log: PathBuf,
    events_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    input_tx: tokio::sync::mpsc::UnboundedSender<AgentInput>,
    control_tx: tokio::sync::mpsc::UnboundedSender<ControlRequest>,
    permission_rx: tokio::sync::mpsc::UnboundedReceiver<AgentPermissionRequest>,
    cancel: CancellationToken,
}

/// `turn_body` is the shell snippet run when a `turn/start` request arrives
/// (after the response line is printed). `$id` holds the request id.
fn stub_script(turn_body: &str) -> String {
    format!(
        r##"#!/bin/sh
LOG="$STUB_LOG"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{{"id":%s,"result":{{"userAgent":"stub/0.139.0"}}}}\n' "$id"
      ;;
    *'"method":"thread/start"'*|*'"method":"thread/resume"'*)
      printf '{{"id":%s,"result":{{"thread":{{"id":"t-1"}},"model":"gpt-5.5"}}}}\n' "$id"
      ;;
    *'"method":"turn/start"'*)
      printf '{{"id":%s,"result":{{"turn":{{"id":"turn-1","items":[],"status":"inProgress"}}}}}}\n' "$id"
      printf '{{"method":"turn/started","params":{{"threadId":"t-1","turn":{{"id":"turn-1","items":[],"status":"inProgress"}}}}}}\n'
      {turn_body}
      ;;
    *'"method":"turn/interrupt"'*)
      printf '{{"id":%s,"result":{{}}}}\n' "$id"
      printf '{{"method":"turn/completed","params":{{"threadId":"t-1","turn":{{"id":"turn-1","items":[],"status":"interrupted"}}}}}}\n'
      ;;
    *'"result":{{"decision":'*)
      # Approval answered — finish the held turn.
      printf '{{"method":"turn/completed","params":{{"threadId":"t-1","turn":{{"id":"turn-1","items":[],"status":"completed"}}}}}}\n'
      ;;
  esac
done
"##
    )
}

fn stub_driver(turn_body: &str, resume: Option<&str>, continuation: bool) -> StubSession {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let requests_log = tmp.path().join("requests.log");
    let script = tmp.path().join("codex-stub.sh");
    std::fs::write(&script, stub_script(turn_body)).expect("write stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let config = CodexConfig {
        codex_bin: script.into_os_string(),
        worktree_path: tmp.path().to_path_buf(),
        system_prompt: Some("SYSPROMPT".into()),
        model: None,
        reasoning_effort: None,
        git_common_dir: None,
        env: vec![(
            std::ffi::OsString::from("STUB_LOG"),
            requests_log.clone().into_os_string(),
        )],
    };
    let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
    let (control_tx, control_rx) = tokio::sync::mpsc::unbounded_channel();
    let (permission_tx, permission_rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(app_server_driver_task(
        config,
        resume.map(str::to_string),
        continuation,
        events_tx,
        input_rx,
        control_rx,
        permission_tx,
        cancel.clone(),
    ));
    StubSession {
        _tmp: tmp,
        requests_log,
        events_rx,
        input_tx,
        control_tx,
        permission_rx,
        cancel,
    }
}

async fn next_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>) -> AgentEvent {
    // 30s, not 10s: each stub turn forks an `sh` child; under a fully loaded
    // full-suite run (3k+ tests sharing the host) the spawn + pipe round
    // trips have been observed to exceed 10s. The timeout only bounds
    // failure latency — passing runs never wait it out.
    tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .expect("event within 30s")
        .expect("events channel open")
}

fn logged_requests(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

const HAPPY_TURN_BODY: &str = r#"printf '{"method":"item/agentMessage/delta","params":{"threadId":"t-1","turnId":"turn-1","itemId":"i1","delta":"po"}}\n'
      printf '{"method":"item/agentMessage/delta","params":{"threadId":"t-1","turnId":"turn-1","itemId":"i1","delta":"ng"}}\n'
      printf '{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","completedAtMs":1,"item":{"id":"i1","type":"agentMessage","text":"pong"}}}\n'
      printf '{"method":"thread/tokenUsage/updated","params":{"threadId":"t-1","turnId":"turn-1","tokenUsage":{"last":{"inputTokens":10,"cachedInputTokens":4,"outputTokens":2,"reasoningOutputTokens":0,"totalTokens":12},"total":{"inputTokens":10,"cachedInputTokens":4,"outputTokens":2,"reasoningOutputTokens":0,"totalTokens":12}}}}\n'
      printf '{"method":"turn/completed","params":{"threadId":"t-1","turn":{"id":"turn-1","items":[],"status":"completed"}}}\n'"#;

#[tokio::test]
async fn handshake_turn_streams_deltas_and_results_then_exits_on_close() {
    let mut s = stub_driver(HAPPY_TURN_BODY, None, false);
    s.input_tx
        .send(AgentInput {
            text: "ping".into(),
            images: vec![],
        })
        .expect("send input");

    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Init { session_id, model: Some(m), .. }
            if session_id == "t-1" && m == "gpt-5.5"
    ));
    // Real streaming: the deltas arrive as separate Message events.
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Message { text, .. } if text == "po"
    ));
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Message { text, .. } if text == "ng"
    ));
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Usage {
            input_tokens: 6,
            cache_read_tokens: 4,
            output_tokens: 2,
            ..
        }
    ));
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Result { text, error: None, .. } if text == "pong"
    ));

    // Engine ends the session by dropping the senders — driver must wind
    // down with exactly one Exited.
    drop(s.input_tx);
    drop(s.control_tx);
    let exited = next_event(&mut s.events_rx).await;
    assert!(matches!(exited, AgentEvent::Exited { .. }));
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), s.events_rx.recv())
            .await
            .expect("channel should close")
            .is_none(),
        "events channel must close after Exited"
    );

    let requests = logged_requests(&s.requests_log);
    assert!(requests[0].contains(r#""method":"initialize""#));
    assert!(
        requests
            .iter()
            .any(|r| r.contains(r#""method":"initialized""#)),
        "initialized notification must follow the initialize response"
    );
    let thread_req = requests
        .iter()
        .find(|r| r.contains(r#""method":"thread/start""#))
        .expect("fresh session must thread/start");
    assert!(
        thread_req.contains("SYSPROMPT"),
        "developerInstructions must carry the engine system prompt"
    );
    assert!(
        thread_req.contains(r#""approvalPolicy":"on-request""#),
        "approval policy must be on-request"
    );
    let turn_req = requests
        .iter()
        .find(|r| r.contains(r#""method":"turn/start""#))
        .expect("turn/start sent");
    assert!(turn_req.contains("ping"));
}

#[tokio::test]
async fn resume_uses_thread_resume_with_stored_id() {
    let mut s = stub_driver(HAPPY_TURN_BODY, Some("sid-9"), false);
    s.input_tx
        .send(AgentInput {
            text: "follow up".into(),
            images: vec![],
        })
        .unwrap();
    // Drain: Init, Message x2, Usage, Result.
    for _ in 0..5 {
        let _ = next_event(&mut s.events_rx).await;
    }
    let requests = logged_requests(&s.requests_log);
    let resume_req = requests
        .iter()
        .find(|r| r.contains(r#""method":"thread/resume""#))
        .expect("resume must use thread/resume");
    assert!(resume_req.contains(r#""threadId":"sid-9""#));
    assert!(
        !requests
            .iter()
            .any(|r| r.contains(r#""method":"thread/start""#)),
        "resume must not start a fresh thread"
    );
    s.cancel.cancel();
}

#[tokio::test]
async fn continuation_starts_turn_without_any_input() {
    let mut s = stub_driver(HAPPY_TURN_BODY, Some("sid-9"), true);
    // No input sent — the continuation prompt must drive a full turn.
    let mut saw_result = false;
    for _ in 0..5 {
        if matches!(
            next_event(&mut s.events_rx).await,
            AgentEvent::Result { .. }
        ) {
            saw_result = true;
            break;
        }
    }
    assert!(saw_result, "continuation turn must complete without input");
    let requests = logged_requests(&s.requests_log);
    let turn_req = requests
        .iter()
        .find(|r| r.contains(r#""method":"turn/start""#))
        .expect("turn/start sent");
    assert!(turn_req.contains("Continue from where you left off."));
    s.cancel.cancel();
}

#[tokio::test]
async fn approval_round_trip_accept_reaches_the_child() {
    // turn/start raises a command approval and HOLDS the turn until the
    // decision arrives (the `"decision":` case arm completes it).
    let turn_body = r#"printf '{"id":100,"method":"item/commandExecution/requestApproval","params":{"threadId":"t-1","turnId":"turn-1","itemId":"i7","command":"sudo ls","cwd":"/wt","startedAtMs":1}}\n'"#;
    let mut s = stub_driver(turn_body, None, false);
    s.input_tx
        .send(AgentInput {
            text: "go".into(),
            images: vec![],
        })
        .unwrap();

    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Init { .. }
    ));

    // The bridge must surface the approval on permission_rx with the
    // backend-shaped tool name and the item id as the pairing key.
    let req = tokio::time::timeout(std::time::Duration::from_secs(30), s.permission_rx.recv())
        .await
        .expect("permission request within 30s")
        .expect("permission channel open");
    assert_eq!(req.id, "i7");
    assert_eq!(req.tool_name, "command_execution");
    assert_eq!(req.input["command"], "sudo ls");
    req.respond.send(true).expect("driver waits for decision");

    // Decision delivered → stub completes the turn → clean Result.
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Result { error: None, .. }
    ));
    let requests = logged_requests(&s.requests_log);
    let decision = requests
        .iter()
        .find(|r| r.contains(r#""decision":"accept""#))
        .expect("accept decision must reach the child");
    assert!(
        decision.contains(r#""id":100"#),
        "decision must answer the server request's id; got {decision}"
    );
    s.cancel.cancel();
}

#[tokio::test]
async fn approval_gated_item_started_does_not_emit_tool_use_until_acceptance() {
    let turn_body = r#"printf '{"id":100,"method":"item/commandExecution/requestApproval","params":{"threadId":"t-1","turnId":"turn-1","itemId":"i7","command":"sudo ls","cwd":"/wt","startedAtMs":1}}\n'
      printf '{"method":"item/started","params":{"threadId":"t-1","turnId":"turn-1","startedAtMs":2,"item":{"id":"i7","type":"commandExecution","command":"sudo ls","commandActions":[],"cwd":"/wt","status":"inProgress"}}}\n'"#;
    let mut s = stub_driver(turn_body, None, false);
    s.input_tx
        .send(AgentInput {
            text: "go".into(),
            images: vec![],
        })
        .unwrap();

    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Init { .. }
    ));
    let req = tokio::time::timeout(std::time::Duration::from_secs(30), s.permission_rx.recv())
        .await
        .expect("permission request within 30s")
        .expect("permission channel open");

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), s.events_rx.recv())
            .await
            .is_err(),
        "item/started while the approval card is pending must not emit ToolUse"
    );

    req.respond.send(true).expect("driver waits for decision");
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::ToolUse { name, id, .. } if name == "command_execution" && id == "i7"
    ));
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::ToolResult { status, id, .. } if status == "error" && id == "i7"
    ));
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Result { error: None, .. }
    ));
    s.cancel.cancel();
}

#[tokio::test]
async fn approval_deny_sends_decline() {
    let turn_body = r#"printf '{"id":100,"method":"item/commandExecution/requestApproval","params":{"threadId":"t-1","turnId":"turn-1","itemId":"i8","command":"rm -rf /","cwd":"/wt","startedAtMs":1}}\n'"#;
    let mut s = stub_driver(turn_body, None, false);
    s.input_tx
        .send(AgentInput {
            text: "go".into(),
            images: vec![],
        })
        .unwrap();
    let _ = next_event(&mut s.events_rx).await; // Init
    let req = tokio::time::timeout(std::time::Duration::from_secs(30), s.permission_rx.recv())
        .await
        .expect("permission request within 30s")
        .expect("permission channel open");
    req.respond.send(false).expect("driver waits");
    let _ = next_event(&mut s.events_rx).await; // Result from the stub's completion
    let requests = logged_requests(&s.requests_log);
    assert!(
        requests
            .iter()
            .any(|r| r.contains(r#""decision":"decline""#)),
        "deny must reach the child as decline"
    );
    s.cancel.cancel();
}

#[tokio::test]
async fn interrupt_sends_turn_interrupt_and_turn_ends_canceled() {
    // A turn that never completes on its own — only the interrupt path (the
    // stub's turn/interrupt arm) ends it.
    let turn_body = r#"printf '{"method":"item/started","params":{"threadId":"t-1","turnId":"turn-1","startedAtMs":1,"item":{"id":"i9","type":"commandExecution","command":"sleep 99","commandActions":[],"cwd":"/wt","status":"inProgress"}}}\n'"#;
    let mut s = stub_driver(turn_body, None, false);
    s.input_tx
        .send(AgentInput {
            text: "go".into(),
            images: vec![],
        })
        .unwrap();
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Init { .. }
    ));
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::ToolUse { .. }
    ));

    s.control_tx.send(ControlRequest::Interrupt).unwrap();

    // Graceful wind-down: the abandoned tool closes, then the interrupted
    // turn's error-free Result (engine's user_hit_stop → Canceled). No child
    // kill, no synthesized failure.
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::ToolResult { status, .. } if status == "error"
    ));
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Result { error: None, .. }
    ));
    let requests = logged_requests(&s.requests_log);
    let interrupt_req = requests
        .iter()
        .find(|r| r.contains(r#""method":"turn/interrupt""#))
        .expect("interrupt must ride the protocol, not a kill");
    assert!(interrupt_req.contains(r#""turnId":"turn-1""#));
    s.cancel.cancel();
}

#[tokio::test]
async fn child_death_mid_turn_synthesizes_failed_result() {
    // The stub starts a command then exits. The engine waits on a Result —
    // the driver must synthesize the failure.
    let turn_body = r#"printf '{"method":"item/started","params":{"threadId":"t-1","turnId":"turn-1","startedAtMs":1,"item":{"id":"i10","type":"commandExecution","command":"x","commandActions":[],"cwd":"/wt","status":"inProgress"}}}\n'
      exit 7"#;
    let mut s = stub_driver(turn_body, None, false);
    s.input_tx
        .send(AgentInput {
            text: "go".into(),
            images: vec![],
        })
        .unwrap();
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Init { .. }
    ));
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::ToolUse { .. }
    ));
    // Closing ToolResult for the abandoned command, then the synthesized
    // failed Result, then Exited.
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::ToolResult { status, .. } if status == "error"
    ));
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Result { error: Some(_), .. }
    ));
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Exited { .. }
    ));
}

#[tokio::test]
async fn cancellation_kills_session_and_emits_exited() {
    let mut s = stub_driver(HAPPY_TURN_BODY, None, false);
    s.cancel.cancel();
    assert!(matches!(
        next_event(&mut s.events_rx).await,
        AgentEvent::Exited { .. }
    ));
}

/// A stale Codex thread id (~/.codex pruned, machine moved) must fall back
/// to a FRESH thread instead of wedging the session permanently — the
/// engine keeps resolving the same dead sid from the thread's branch
/// context, so without this one-shot fallback every follow-up would fail
/// with "handshake failed" forever. Context still arrives via the THREAD
/// HISTORY block the engine appends to the system prompt.
#[tokio::test]
async fn stale_resume_falls_back_to_fresh_thread() {
    // Stub: thread/resume errors (unknown thread id); thread/start succeeds.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let requests_log = tmp.path().join("requests.log");
    let script = tmp.path().join("codex-stub.sh");
    std::fs::write(
        &script,
        r##"#!/bin/sh
LOG="$STUB_LOG"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"id":%s,"result":{"userAgent":"stub/0.139.0"}}\n' "$id"
      ;;
    *'"method":"thread/resume"'*)
      printf '{"id":%s,"error":{"code":-32600,"message":"thread not found: sid-dead"}}\n' "$id"
      ;;
    *'"method":"thread/start"'*)
      printf '{"id":%s,"result":{"thread":{"id":"t-fresh"},"model":"gpt-5.5"}}\n' "$id"
      ;;
    *'"method":"turn/start"'*)
      printf '{"id":%s,"result":{"turn":{"id":"turn-1","items":[],"status":"inProgress"}}}\n' "$id"
      printf '{"method":"item/completed","params":{"threadId":"t-fresh","turnId":"turn-1","completedAtMs":1,"item":{"id":"i1","type":"agentMessage","text":"recovered"}}}\n'
      printf '{"method":"turn/completed","params":{"threadId":"t-fresh","turn":{"id":"turn-1","items":[],"status":"completed"}}}\n'
      ;;
  esac
done
"##,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let config = CodexConfig {
        codex_bin: script.into_os_string(),
        worktree_path: tmp.path().to_path_buf(),
        system_prompt: Some("SYSPROMPT".into()),
        model: None,
        reasoning_effort: None,
        git_common_dir: None,
        env: vec![(
            std::ffi::OsString::from("STUB_LOG"),
            requests_log.clone().into_os_string(),
        )],
    };
    let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
    let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
    let (_control_tx, control_rx) = tokio::sync::mpsc::unbounded_channel();
    let (permission_tx, _permission_rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(app_server_driver_task(
        config,
        Some("sid-dead".to_string()),
        false,
        events_tx,
        input_rx,
        control_rx,
        permission_tx,
        cancel.clone(),
    ));
    input_tx
        .send(AgentInput {
            text: "continue please".into(),
            images: vec![],
        })
        .unwrap();

    // Init must carry the FRESH thread id — the engine persists it so the
    // next resume targets the live thread.
    assert!(matches!(
        next_event(&mut events_rx).await,
        AgentEvent::Init { session_id, .. } if session_id == "t-fresh"
    ));
    assert!(matches!(
        next_event(&mut events_rx).await,
        AgentEvent::Message { text, .. } if text == "recovered"
    ));
    assert!(matches!(
        next_event(&mut events_rx).await,
        AgentEvent::Result { error: None, .. }
    ));
    let requests = logged_requests(&requests_log);
    assert!(
        requests
            .iter()
            .any(|r| r.contains(r#""method":"thread/resume""#)),
        "resume must be attempted first"
    );
    assert!(
        requests
            .iter()
            .any(|r| r.contains(r#""method":"thread/start""#)),
        "fallback must start a fresh thread"
    );
    cancel.cancel();
}
