use super::*;
use crate::runtime::stub_executable::write_stub_executable;

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

fn stub_driver_running(script_tail: &str, resume: Option<&str>) -> StubSession {
    stub_driver_prompted("SYSPROMPT", script_tail, resume)
}

const STUB_SESSION_CONTEXT: &str = "SESSION CONTEXT: branch `stub-branch`";

fn stub_driver_prompted(
    system_prompt: &str,
    script_tail: &str,
    resume: Option<&str>,
) -> StubSession {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let args_log = tmp.path().join("args.log");
    let stdin_log = tmp.path().join("stdin.log");
    let script = tmp.path().join("codex-stub.sh");
    let body = format!(
        "#!/bin/sh\nprintf '%s' \"$*\" | tr '\\n' ' ' >> {log}\nprintf '\\n' >> {log}\n\
         tr '\\n' ' ' >> {stdin}\nprintf '\\n' >> {stdin}\n{script_tail}",
        log = args_log.display(),
        stdin = stdin_log.display(),
    );
    write_stub_executable(&script, &body);

    let config = CodexConfig {
        codex_bin: script.into_os_string(),
        worktree_path: tmp.path().to_path_buf(),
        system_prompt: Some(system_prompt.into()),
        session_context: Some(STUB_SESSION_CONTEXT.into()),
        model: None,
        reasoning_effort: None,
        sandbox_writable_roots: Vec::new(),
        sandbox: crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
        login_shell: true,
        env_removed: Vec::new(),
        env: Vec::new(),
    };
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let (withdraw_tx, withdraw_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        config,
        resume.map(str::to_string),
        events_tx,
        input_rx,
        control_rx,
        withdraw_rx,
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
            withdraw_tx,
        },
        cancel,
    }
}

const EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

async fn next_event(agent: &mut RunningAgent) -> AgentEvent {
    recv(&mut agent.events_rx).await
}

async fn recv(rx: &mut mpsc::UnboundedReceiver<AgentEvent>) -> AgentEvent {
    tokio::time::timeout(EVENT_TIMEOUT, async {
        loop {
            let event = rx.recv().await.expect("events channel open");
            if !matches!(event, AgentEvent::BillingObserved { .. }) {
                break event;
            }
        }
    })
    .await
    .expect("an event before the liveness ceiling")
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
            defer_on_steer_failure: false,
            text: "ping".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .expect("send input");

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead { .. }
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

    let invocations = logged_invocations(&s.args_log);
    assert_eq!(invocations.len(), 1);
    assert!(!invocations[0].contains("resume"));
    let prompts = logged_invocations(&s.stdin_log);
    assert!(prompts[0].contains("SYSPROMPT"));
    assert!(
        prompts[0].ends_with(&format!("{STUB_SESSION_CONTEXT}  ping")),
        "a fresh session's first turn carries the session context: {}",
        prompts[0]
    );
}

#[tokio::test]
async fn follow_up_turn_resumes_with_session_id_from_first_turn() {
    let mut s = stub_driver(HAPPY_TURN, None);
    s.agent
        .input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
            text: "first".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .unwrap();
    for _ in 0..5 {
        let _ = next_event(&mut s.agent).await;
    }
    s.agent
        .input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
            text: "second".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .unwrap();
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead { .. }
    ));
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::Message { .. }
    ));
    let _ = next_event(&mut s.agent).await;
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

#[tokio::test]
async fn a_resumed_session_runs_the_engine_continuation_as_an_input() {
    let continuation = "Continue from where you left off.";
    let mut s = stub_driver(HAPPY_TURN, Some("sid-9"));
    s.agent
        .input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
            text: continuation.into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .expect("send input");

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead { .. }
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

const LINUX_MAX_ARG_STRLEN: usize = 131_072;

#[tokio::test]
async fn a_huge_first_turn_prompt_travels_on_stdin_not_argv() {
    let marker = "THREAD-HISTORY-LINE ";
    let system_prompt = marker.repeat(1024 * 1024 / marker.len());
    let script_tail = format!("cat <<'JSONL_EOF'\n{HAPPY_TURN}\nJSONL_EOF\n");
    let mut s = stub_driver_prompted(&system_prompt, &script_tail, None);
    s.agent
        .input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
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
    let body = r#"{"type":"thread.started","thread_id":"t-1"}
{"type":"error","message":"401 Unauthorized"}"#;
    let mut s = stub_driver(body, None);
    s.agent
        .input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
            text: "ping".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .unwrap();

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead { .. }
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
    let body = r#"{"type":"thread.started","thread_id":"t-1"}
{"type":"item.started","item":{"id":"i0","type":"command_execution","command":"sleep 99","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
    let mut s = stub_driver(body, None);
    s.agent
        .input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
            text: "go".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .unwrap();

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead { .. }
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
    s.cancel.cancel();
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::Exited { .. }
    ));
}

#[tokio::test]
async fn interrupt_kills_in_flight_turn_and_synthesizes_canceled_result() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let script = tmp.path().join("codex-stub.sh");
    write_stub_executable(
        &script,
        "#!/bin/sh\n\
         printf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"t-1\"}'\n\
         printf '%s\\n' '{\"type\":\"item.started\",\"item\":{\"id\":\"i0\",\"type\":\"command_execution\",\"command\":\"sleep\",\"aggregated_output\":\"\",\"exit_code\":null,\"status\":\"in_progress\"}}'\n\
         sleep 60\n",
    );
    let config = CodexConfig {
        codex_bin: script.into_os_string(),
        worktree_path: tmp.path().to_path_buf(),
        system_prompt: None,
        session_context: None,
        model: None,
        reasoning_effort: None,
        sandbox_writable_roots: Vec::new(),
        sandbox: crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
        login_shell: true,
        env_removed: Vec::new(),
        env: Vec::new(),
    };
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let (_withdraw_tx, withdraw_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        config,
        None,
        events_tx,
        input_rx,
        control_rx,
        withdraw_rx,
        cancel.clone(),
    ));

    input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
            text: "go".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .unwrap();
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::InputRead { .. }
    ));
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::Init { .. }
    ));
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::ToolUse { .. }
    ));

    input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
            text: "queued".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .unwrap();
    control_tx.send(ControlRequest::Interrupt).unwrap();

    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::ToolResult { status, .. } if status == "error"
    ));
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::Result { error: None, .. }
    ));
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::InputRead { .. }
    ));
    assert!(matches!(
        recv(&mut events_rx).await,
        AgentEvent::ToolUse { .. }
    ));

    cancel.cancel();
    loop {
        match recv(&mut events_rx).await {
            AgentEvent::Exited { .. } => break,
            AgentEvent::ToolResult { .. } | AgentEvent::Result { .. } => continue,
            other => panic!("unexpected event during shutdown: {:?}", other),
        }
    }
}

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
            defer_on_steer_failure: false,
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
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    s.agent
        .input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
            text: "later".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .expect("send input");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    std::fs::write(&go_on, "").expect("release the stub");

    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead { .. }
    ));
    let init = next_event(&mut s.agent).await;
    assert!(
        matches!(&init, AgentEvent::Init { session_id, .. } if session_id == "t-1"),
        "the split line must arrive whole, got {init:?}"
    );

    s.cancel.cancel();
}

fn input(text: &str) -> AgentInput {
    AgentInput {
        defer_on_steer_failure: false,
        text: text.into(),
        images: vec![],
        uuid: uuid::Uuid::new_v4(),
    }
}

async fn withdraw(agent: &RunningAgent, input_uuid: uuid::Uuid) -> InputWithdrawal {
    let (reply, answer) = tokio::sync::oneshot::channel();
    agent
        .withdraw_tx
        .send(WithdrawRequest { input_uuid, reply })
        .expect("driver alive");
    tokio::time::timeout(EVENT_TIMEOUT, answer)
        .await
        .expect("withdraw answered")
        .expect("driver answered")
}

#[tokio::test]
async fn withdraw_drops_a_queued_input_and_refuses_the_one_in_flight() {
    let signals = tempfile::TempDir::new().expect("tempdir");
    let go_on = signals.path().join("go-on");
    let tail = format!(
        "printf '%s\\n' '{{\"type\":\"thread.started\",\"thread_id\":\"t-1\"}}'\n\
         while [ ! -e '{go}' ]; do sleep 0.05; done\n\
         cat <<'JSONL_EOF'\n{rest}\nJSONL_EOF\n",
        go = go_on.display(),
        rest = HAPPY_TURN.split_once('\n').expect("a multi-line turn").1,
    );
    let mut s = stub_driver_running(&tail, None);
    let running = input("running");
    s.agent.input_tx.send(running.clone()).unwrap();
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead { input } if input == running.uuid
    ));

    let queued = input("queued report");
    s.agent.input_tx.send(queued.clone()).unwrap();
    assert_eq!(
        withdraw(&s.agent, queued.uuid).await,
        InputWithdrawal::Withdrawn
    );
    assert_eq!(
        withdraw(&s.agent, running.uuid).await,
        InputWithdrawal::AlreadyRead
    );
    std::fs::write(&go_on, "").expect("release the stub");
    while !matches!(next_event(&mut s.agent).await, AgentEvent::Result { .. }) {}

    let next = input("next");
    s.agent.input_tx.send(next.clone()).unwrap();
    assert!(matches!(
        next_event(&mut s.agent).await,
        AgentEvent::InputRead { input } if input == next.uuid
    ));
    let prompts = std::fs::read_to_string(&s.stdin_log).unwrap_or_default();
    assert!(
        !prompts.contains("queued report"),
        "a withdrawn input must never start a turn: {prompts}"
    );
    s.cancel.cancel();
}

#[tokio::test]
async fn a_turn_that_never_starts_rejects_its_input() {
    let mut s = stub_driver_running("exit 1\n", None);
    let ping = uuid::Uuid::new_v4();
    s.agent
        .input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
            text: "ping".into(),
            images: vec![],
            uuid: ping,
        })
        .expect("send input");
    assert_eq!(rejected_before_the_result(&mut s.agent).await, ping);
    s.cancel.cancel();
}

#[tokio::test]
async fn a_turn_that_cannot_spawn_rejects_its_input() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config = CodexConfig {
        codex_bin: tmp.path().join("no-such-codex").into_os_string(),
        worktree_path: tmp.path().to_path_buf(),
        system_prompt: Some("SYSPROMPT".into()),
        session_context: None,
        model: None,
        reasoning_effort: None,
        sandbox_writable_roots: Vec::new(),
        sandbox: crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
        login_shell: true,
        env_removed: Vec::new(),
        env: Vec::new(),
    };
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let (withdraw_tx, withdraw_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        config,
        None,
        events_tx,
        input_rx,
        control_rx,
        withdraw_rx,
        cancel.clone(),
    ));
    let mut agent = RunningAgent {
        kind: CodingAgent::Codex,
        events_rx,
        input_tx,
        control_tx,
        permission_rx: None,
        withdraw_tx,
    };
    let ping = uuid::Uuid::new_v4();
    agent
        .input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
            text: "ping".into(),
            images: vec![],
            uuid: ping,
        })
        .expect("send input");
    assert_eq!(rejected_before_the_result(&mut agent).await, ping);
    cancel.cancel();
}

async fn rejected_before_the_result(agent: &mut RunningAgent) -> uuid::Uuid {
    let mut rejected = None;
    loop {
        match next_event(agent).await {
            AgentEvent::InputRead { .. } => panic!("no output, so no read"),
            AgentEvent::InputRejected { input } => rejected = Some(input),
            AgentEvent::Result { error, .. } => {
                assert!(error.is_some(), "the turn failed");
                break;
            }
            _ => {}
        }
    }
    rejected.expect("the rejected input is named before the Result")
}
