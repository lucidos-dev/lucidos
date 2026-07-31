use super::*;

/// Stand up a driver over a stub "codex" shell script. The stub appends its
/// argv to `args.log` in the temp dir (one line per invocation, `|`-joined)
/// and prints the given JSONL body — the same seam CC's driver tests get
/// from a pre-spawned `sh` child.
struct StubSession {
    _tmp: tempfile::TempDir,
    args_log: PathBuf,
    agent: RunningAgent,
    cancel: CancellationToken,
}

fn stub_driver(jsonl_body: &str, resume: Option<&str>, continuation: bool) -> StubSession {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let args_log = tmp.path().join("args.log");
    let script = tmp.path().join("codex-stub.sh");
    // One log line per invocation — the prompt argument is multi-line (the
    // system-prompt block), so flatten newlines before appending.
    let body = format!(
        "#!/bin/sh\nprintf '%s' \"$*\" | tr '\\n' ' ' >> {log}\nprintf '\\n' >> {log}\ncat <<'JSONL_EOF'\n{}\nJSONL_EOF\n",
        jsonl_body,
        log = args_log.display(),
    );
    std::fs::write(&script, body).expect("write stub");
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
        sandbox_writable_roots: Vec::new(),
        env: Vec::new(),
    };
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        config,
        resume.map(str::to_string),
        continuation,
        events_tx,
        input_rx,
        control_rx,
        cancel.clone(),
    ));
    StubSession {
        _tmp: tmp,
        args_log,
        agent: RunningAgent {
            kind: CodingAgent::Codex,
            events_rx,
            input_tx,
            control_tx,
            permission_rx: None,
        },
        cancel,
    }
}

async fn next_event(agent: &mut RunningAgent) -> AgentEvent {
    tokio::time::timeout(std::time::Duration::from_secs(10), agent.events_rx.recv())
        .await
        .expect("event within 10s")
        .expect("events channel open")
}

fn logged_invocations(args_log: &Path) -> Vec<String> {
    std::fs::read_to_string(args_log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

const HAPPY_TURN: &str = r#"{"type":"thread.started","thread_id":"t-1"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"i0","type":"agent_message","text":"pong"}}
{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":2}}"#;

#[tokio::test]
async fn one_turn_emits_init_message_usage_result_then_exited_on_close() {
    let mut s = stub_driver(HAPPY_TURN, None, false);
    s.agent
        .input_tx
        .send(AgentInput {
            text: "ping".into(),
            images: vec![],
        })
        .expect("send input");

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::Init { session_id, .. } if session_id == "t-1"
    ));
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::Message { text, .. } if text == "pong"
    ));
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::Usage {
            input_tokens: 6,
            cache_read_tokens: 4,
            output_tokens: 2,
            ..
        }
    ));
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::Result { text, error: None, .. } if text == "pong"
    ));

    // Engine ends the session by dropping the senders — driver must wind
    // down with exactly one Exited.
    let RunningAgent {
        input_tx,
        control_tx,
        mut events_rx,
        ..
    } = s.agent;
    drop(input_tx);
    drop(control_tx);
    let exited = tokio::time::timeout(std::time::Duration::from_secs(10), events_rx.recv())
        .await
        .expect("Exited within 10s")
        .expect("events channel open");
    assert!(matches!(exited, AgentEvent::Exited { .. }));
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), events_rx.recv())
            .await
            .expect("channel should close")
            .is_none(),
        "events channel must close after Exited"
    );

    // First fresh turn must carry the system prompt inline and no resume.
    let invocations = logged_invocations(&s.args_log);
    assert_eq!(invocations.len(), 1);
    assert!(invocations[0].contains("SYSPROMPT"));
    assert!(!invocations[0].contains("resume"));
}

#[tokio::test]
async fn follow_up_turn_resumes_with_session_id_from_first_turn() {
    let mut s = stub_driver(HAPPY_TURN, None, false);
    s.agent
        .input_tx
        .send(AgentInput {
            text: "first".into(),
            images: vec![],
        })
        .unwrap();
    // Drain turn 1: Init, Message, Usage, Result.
    for _ in 0..4 {
        let _ = next_event(&mut s.agent).await;
    }
    s.agent
        .input_tx
        .send(AgentInput {
            text: "second".into(),
            images: vec![],
        })
        .unwrap();
    // Turn 2: duplicate thread.started is suppressed → Message, Usage, Result.
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::Message { .. }
    ));
    let _ = next_event(&mut s.agent).await; // Usage
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::Result { .. }
    ));

    let invocations = logged_invocations(&s.args_log);
    assert_eq!(invocations.len(), 2);
    assert!(
        invocations[1].contains("resume t-1"),
        "turn 2 must resume the thread id announced in turn 1; got {:?}",
        invocations[1]
    );
    assert!(
        !invocations[1].contains("SYSPROMPT"),
        "resumed turns must not re-send the system prompt — it's already in the Codex-side history"
    );

    s.cancel.cancel();
}

#[tokio::test]
async fn continuation_spawns_turn_without_any_input() {
    // ContinuationRequested recovery: the engine sends NO input and expects
    // the agent to pick up on its own. The driver must start the turn with
    // the synthetic continuation prompt against the resumed session.
    let mut s = stub_driver(HAPPY_TURN, Some("sid-9"), true);
    // No input sent — events must still arrive.
    let mut saw_result = false;
    for _ in 0..4 {
        if matches!(next_event(&mut s.agent).await, AgentEvent::Result { .. }) {
            saw_result = true;
            break;
        }
    }
    assert!(saw_result, "continuation turn must complete without input");

    let invocations = logged_invocations(&s.args_log);
    assert_eq!(invocations.len(), 1);
    assert!(invocations[0].contains("resume sid-9"));
    assert!(invocations[0].contains(CONTINUATION_PROMPT));

    s.cancel.cancel();
}

#[tokio::test]
async fn child_death_without_terminal_synthesizes_failed_result() {
    // Auth failure / crash shape: stream announces the thread then dies with
    // no turn.completed. The engine waits on a Result — the driver must
    // synthesize a failed one instead of leaving the thread wedged.
    let body = r#"{"type":"thread.started","thread_id":"t-1"}
{"type":"error","message":"401 Unauthorized"}"#;
    let mut s = stub_driver(body, None, false);
    s.agent
        .input_tx
        .send(AgentInput {
            text: "ping".into(),
            images: vec![],
        })
        .unwrap();

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::Init { .. }
    ));
    match next_event(&mut s.agent).await {
        AgentEvent::Result {
            error: Some(err), ..
        } => assert!(
            err.contains("401 Unauthorized"),
            "synthesized Result must carry the last stream error; got {err}"
        ),
        other => panic!("expected synthesized failed Result, got {:?}", other),
    }

    s.cancel.cancel();
}

#[tokio::test]
async fn abandoned_tool_call_is_closed_at_synthesized_turn_end() {
    // A turn that dies mid-command must close the ToolUse it opened —
    // otherwise the engine's tools_in_flight counter never re-arms the
    // hang watchdog.
    let body = r#"{"type":"thread.started","thread_id":"t-1"}
{"type":"item.started","item":{"id":"i0","type":"command_execution","command":"sleep 99","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
    let mut s = stub_driver(body, None, false);
    s.agent
        .input_tx
        .send(AgentInput {
            text: "go".into(),
            images: vec![],
        })
        .unwrap();

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::Init { .. }
    ));
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::ToolUse { .. }
    ));
    match next_event(&mut s.agent).await {
        AgentEvent::ToolResult { status, id, .. } => {
            assert_eq!(status, "error");
            assert_eq!(id, "i0");
        }
        other => panic!("expected closing ToolResult, got {:?}", other),
    }
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::Result { error: Some(_), .. }
    ));

    s.cancel.cancel();
}

#[tokio::test]
async fn cancellation_kills_session_and_emits_exited() {
    let mut s = stub_driver(HAPPY_TURN, None, false);
    // Cancel while idle (no turn running).
    s.cancel.cancel();
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::Exited { .. }
    ));
}

#[tokio::test]
async fn interrupt_kills_in_flight_turn_and_synthesizes_canceled_result() {
    // Stub that opens a command then blocks: the driver must kill it on
    // Interrupt and synthesize an error-free Result (the engine's
    // user_hit_stop latch turns it into ResponseCanceled).
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let script = tmp.path().join("codex-stub.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\n\
         printf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"t-1\"}'\n\
         printf '%s\\n' '{\"type\":\"item.started\",\"item\":{\"id\":\"i0\",\"type\":\"command_execution\",\"command\":\"sleep\",\"aggregated_output\":\"\",\"exit_code\":null,\"status\":\"in_progress\"}}'\n\
         sleep 60\n",
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
        system_prompt: None,
        model: None,
        reasoning_effort: None,
        sandbox_writable_roots: Vec::new(),
        env: Vec::new(),
    };
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        config,
        None,
        false,
        events_tx,
        input_rx,
        control_rx,
        cancel.clone(),
    ));

    input_tx
        .send(AgentInput {
            text: "go".into(),
            images: vec![],
        })
        .unwrap();
    async fn recv(rx: &mut mpsc::UnboundedReceiver<AgentEvent>) -> AgentEvent {
        tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
            .await
            .expect("event within 10s")
            .expect("channel open")
    }
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::Init { .. }
    ));
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::ToolUse { .. }
    ));

    // Queue a follow-up BEFORE interrupting. The engine counted it in
    // pending_followups and expects a Result for it — the driver must run
    // it as a fresh turn after the interrupt (CC's stdin queue behaves the
    // same way after Esc).
    input_tx
        .send(AgentInput {
            text: "queued".into(),
            images: vec![],
        })
        .unwrap();
    control_tx.send(ControlRequest::Interrupt).unwrap();

    // Closing ToolResult for the abandoned command, then the synthesized
    // error-free Result (engine's user_hit_stop turns it into Canceled).
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::ToolResult { status, .. } if status == "error"
    ));
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::Result { error: None, .. }
    ));
    // The queued follow-up starts the next turn (the stub blocks again, so
    // its ToolUse is the signal the turn is running).
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::ToolUse { .. }
    ));

    cancel.cancel();
    // The cancelled second turn produces no further Result — the driver
    // winds down with Exited (possibly after the closing ToolResult).
    loop {
        match recv(&mut events_rx).await {
            AgentEvent::Exited { .. } => break,
            AgentEvent::ToolResult { .. } | AgentEvent::Result { .. } => continue,
            other => panic!("unexpected event during shutdown: {:?}", other),
        }
    }
}
