use super::*;

/// Stand up a driver over a stub "codex" shell script. The stub appends its
/// argv to `args.log` and its stdin to `stdin.log` in the temp dir (one line
/// per invocation each), then prints the given JSONL body. That is the same
/// seam CC's driver tests get from a pre-spawned `sh` child.
struct StubSession {
    _tmp: tempfile::TempDir,
    args_log: PathBuf,
    stdin_log: PathBuf,
    agent: RunningAgent,
    cancel: CancellationToken,
}

fn stub_driver(jsonl_body: &str, resume: Option<&str>) -> StubSession {
    stub_driver_running(
        &format!("cat <<'JSONL_EOF'\n{jsonl_body}\nJSONL_EOF\n"),
        resume,
    )
}

/// [`stub_driver`] with the stub's output left to `script_tail`, a shell
/// snippet that runs after the argv is logged.
fn stub_driver_running(script_tail: &str, resume: Option<&str>) -> StubSession {
    stub_driver_prompted("SYSPROMPT", script_tail, resume)
}

/// [`stub_driver_running`] under the given engine system prompt.
fn stub_driver_prompted(
    system_prompt: &str,
    script_tail: &str,
    resume: Option<&str>,
) -> StubSession {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let args_log = tmp.path().join("args.log");
    let stdin_log = tmp.path().join("stdin.log");
    let script = tmp.path().join("codex-stub.sh");
    // One log line per invocation in each log. The stdin prompt is
    // multi-line (the system-prompt block), so flatten newlines first.
    let body = format!(
        "#!/bin/sh\nprintf '%s' \"$*\" | tr '\\n' ' ' >> {log}\nprintf '\\n' >> {log}\n\
         tr '\\n' ' ' >> {stdin}\nprintf '\\n' >> {stdin}\n{script_tail}",
        log = args_log.display(),
        stdin = stdin_log.display(),
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
        system_prompt: Some(system_prompt.into()),
        model: None,
        reasoning_effort: None,
        sandbox_writable_roots: Vec::new(),
        env_removed: Vec::new(),
        env: Vec::new(),
    };
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        config,
        resume.map(str::to_string),
        events_tx,
        input_rx,
        control_rx,
        cancel.clone(),
    ));
    StubSession {
        _tmp: tmp,
        args_log,
        stdin_log,
        agent: RunningAgent {
            kind: CodingAgent::Codex,
            events_rx,
            input_tx,
            control_tx,
            permission_rx: None,
            withdraw_tx: None,
        },
        cancel,
    }
}

/// How long a driver test waits for the app-server to say anything.
///
/// A liveness ceiling, not a claim about speed. The child is a `/bin/sh` stub,
/// but the full suite runs thousands of tests at once and starves it.
/// Ten seconds passed in isolation and timed out three of these under load,
/// which reads as breakage in the diff and never is. A slow spawn is not the
/// bug these tests look for, so the ceiling only has to clear a real one.
const EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

async fn next_event(agent: &mut RunningAgent) -> AgentEvent {
    tokio::time::timeout(EVENT_TIMEOUT, agent.events_rx.recv())
        .await
        .expect("an event before the liveness ceiling")
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
    let mut s = stub_driver(HAPPY_TURN, None);
    s.agent
        .input_tx
        .send(AgentInput {
            text: "ping".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .expect("send input");

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead(None)
    ));
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
    let exited = tokio::time::timeout(EVENT_TIMEOUT, events_rx.recv())
        .await
        .expect("Exited before the liveness ceiling")
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
    assert!(!invocations[0].contains("resume"));
    let prompts = logged_invocations(&s.stdin_log);
    assert!(prompts[0].contains("SYSPROMPT"));
    assert!(prompts[0].ends_with("ping"));
}

#[tokio::test]
async fn follow_up_turn_resumes_with_session_id_from_first_turn() {
    let mut s = stub_driver(HAPPY_TURN, None);
    s.agent
        .input_tx
        .send(AgentInput {
            text: "first".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .unwrap();
    // Drain turn 1: InputRead, Init, Message, Usage, Result.
    for _ in 0..5 {
        let _ = next_event(&mut s.agent).await;
    }
    s.agent
        .input_tx
        .send(AgentInput {
            text: "second".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .unwrap();
    // Turn 2: duplicate thread.started is suppressed → Message, Usage, Result.
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead(None)
    ));
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
    let prompts = logged_invocations(&s.stdin_log);
    assert_eq!(prompts[1], "second");
    assert!(
        !prompts[1].contains("SYSPROMPT"),
        "resumed turns must not re-send the system prompt: it is already in the Codex-side history"
    );

    s.cancel.cancel();
}

/// Recovery resumes a session with the continuation as an ordinary input. The
/// turn runs against the resumed session and reports that input read, so the
/// engine's ledger settles it (ADR 0268).
#[tokio::test]
async fn a_resumed_session_runs_the_engine_continuation_as_an_input() {
    let continuation = "Continue from where you left off.";
    let mut s = stub_driver(HAPPY_TURN, Some("sid-9"));
    s.agent
        .input_tx
        .send(AgentInput {
            text: continuation.into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .expect("send input");

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead(None)
    ));
    let mut saw_result = false;
    for _ in 0..4 {
        if matches!(next_event(&mut s.agent).await, AgentEvent::Result { .. }) {
            saw_result = true;
            break;
        }
    }
    assert!(saw_result, "the continuation turn must complete");

    let invocations = logged_invocations(&s.args_log);
    assert_eq!(invocations.len(), 1);
    assert!(invocations[0].contains("resume sid-9"));
    assert_eq!(logged_invocations(&s.stdin_log), [continuation]);

    s.cancel.cancel();
}

/// Linux's `MAX_ARG_STRLEN`: the most bytes one argv string may hold.
const LINUX_MAX_ARG_STRLEN: usize = 131_072;

/// A first turn carrying a long thread's history reaches codex on stdin. On
/// argv it would fail the spawn with `E2BIG` once it passed the Linux cap, and
/// anyone on the host could read it in `/proc/<pid>/cmdline`.
#[tokio::test]
async fn a_huge_first_turn_prompt_travels_on_stdin_not_argv() {
    let marker = "THREAD-HISTORY-LINE ";
    let system_prompt = marker.repeat(1024 * 1024 / marker.len());
    let script_tail = format!("cat <<'JSONL_EOF'\n{HAPPY_TURN}\nJSONL_EOF\n");
    let mut s = stub_driver_prompted(&system_prompt, &script_tail, None);
    s.agent
        .input_tx
        .send(AgentInput {
            text: "ping".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .expect("send input");
    loop {
        if let AgentEvent::Result { error, .. } = next_event(&mut s.agent).await {
            assert_eq!(error, None, "the turn must run");
            break;
        }
    }

    let invocations = logged_invocations(&s.args_log);
    assert_eq!(invocations.len(), 1);
    assert!(invocations[0].len() <= LINUX_MAX_ARG_STRLEN);
    assert!(
        !invocations[0].contains(marker),
        "the prompt leaked into argv"
    );
    let prompts = logged_invocations(&s.stdin_log);
    assert!(prompts[0].contains(&system_prompt));
    assert!(prompts[0].ends_with("ping"));

    s.cancel.cancel();
}

#[tokio::test]
async fn child_death_without_terminal_synthesizes_failed_result() {
    // Auth failure / crash shape: stream announces the thread then dies with
    // no turn.completed. The engine waits on a Result — the driver must
    // synthesize a failed one instead of leaving the thread wedged.
    let body = r#"{"type":"thread.started","thread_id":"t-1"}
{"type":"error","message":"401 Unauthorized"}"#;
    let mut s = stub_driver(body, None);
    s.agent
        .input_tx
        .send(AgentInput {
            text: "ping".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .unwrap();

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead(None)
    ));
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
    let mut s = stub_driver(body, None);
    s.agent
        .input_tx
        .send(AgentInput {
            text: "go".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .unwrap();

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead(None)
    ));
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
    let mut s = stub_driver(HAPPY_TURN, None);
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
        env_removed: Vec::new(),
        env: Vec::new(),
    };
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        config,
        None,
        events_tx,
        input_rx,
        control_rx,
        cancel.clone(),
    ));

    input_tx
        .send(AgentInput {
            text: "go".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .unwrap();
    async fn recv(rx: &mut mpsc::UnboundedReceiver<AgentEvent>) -> AgentEvent {
        tokio::time::timeout(EVENT_TIMEOUT, rx.recv())
            .await
            .expect("an event before the liveness ceiling")
            .expect("channel open")
    }
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::InputRead(None)
    ));
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::Init { .. }
    ));
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::ToolUse { .. }
    ));

    // Queue a follow-up BEFORE interrupting. The engine owes it until its turn
    // starts (ADR 0268), so the driver must run it as a fresh turn after the
    // interrupt. CC's stdin queue behaves the same way after Esc.
    input_tx
        .send(AgentInput {
            text: "queued".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
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
    // The queued follow-up starts the next turn, which reports it read. The
    // stub blocks again, so its ToolUse is the signal the turn is running.
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::InputRead(None)
    ));
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

/// A line that arrives in two chunks survives an input queued between them.
///
/// The stub writes half its `thread.started` line, then waits. An input sent
/// meanwhile wins the driver's select and drops the read. A read that loses
/// the half leaves an unparseable tail, and the turn never reports `Init`.
#[tokio::test]
async fn a_line_split_around_a_queued_input_still_arrives_whole() {
    let signals = tempfile::TempDir::new().expect("tempdir");
    let half_written = signals.path().join("half-written");
    let go_on = signals.path().join("go-on");
    let tail = format!(
        "printf '%s' '{{\"type\":\"thread.started\",'\n\
         touch '{half}'\n\
         while [ ! -e '{go}' ]; do sleep 0.05; done\n\
         printf '%s\\n' '\"thread_id\":\"t-1\"}}'\n\
         cat <<'JSONL_EOF'\n{rest}\nJSONL_EOF\n",
        half = half_written.display(),
        go = go_on.display(),
        rest = HAPPY_TURN.split_once('\n').expect("a multi-line turn").1,
    );
    let mut s = stub_driver_running(&tail, None);
    s.agent
        .input_tx
        .send(AgentInput {
            text: "ping".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .expect("send input");

    let started = std::time::Instant::now();
    while !half_written.exists() {
        assert!(
            started.elapsed() < EVENT_TIMEOUT,
            "the stub never wrote its first half"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    // Time for the driver to read the half, then to take the queued input.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    s.agent
        .input_tx
        .send(AgentInput {
            text: "later".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .expect("send input");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    std::fs::write(&go_on, "").expect("release the stub");

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead(None)
    ));
    let init = next_event(&mut s.agent).await;
    assert!(
        matches!(&init, AgentEvent::Init { session_id, .. } if session_id == "t-1"),
        "the split line must arrive whole, got {init:?}"
    );

    s.cancel.cancel();
}
