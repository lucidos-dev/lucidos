use super::*;

async fn spawn_driver_for_test(program: &str, args: &[&str]) -> (RunningAgent, CancellationToken) {
    spawn_driver_owing(program, args, SessionContextDue::default()).await
}

async fn spawn_driver_owing(
    program: &str,
    args: &[&str],
    session_context: SessionContextDue,
) -> (RunningAgent, CancellationToken) {
    spawn_driver_checking(program, args, session_context, None).await
}

async fn spawn_driver_checking(
    program: &str,
    args: &[&str],
    session_context: SessionContextDue,
    startup_follow_up: Option<StartupFollowUp>,
) -> (RunningAgent, CancellationToken) {
    spawn_driver_full(
        program,
        args,
        session_context,
        NotesReminderDue::default(),
        startup_follow_up,
    )
    .await
}

async fn spawn_driver_owing_notes(
    program: &str,
    args: &[&str],
    notes_reminder: NotesReminderDue,
) -> (RunningAgent, CancellationToken) {
    spawn_driver_full(
        program,
        args,
        SessionContextDue::default(),
        notes_reminder,
        None,
    )
    .await
}

async fn spawn_driver_full(
    program: &str,
    args: &[&str],
    session_context: SessionContextDue,
    notes_reminder: NotesReminderDue,
    startup_follow_up: Option<StartupFollowUp>,
) -> (RunningAgent, CancellationToken) {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn test child");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = StderrTail::spawn(child.stderr.take().expect("stderr"), Vec::new());
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let (withdraw_tx, withdraw_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        child,
        stdin,
        BufReader::new(stdout),
        stderr,
        events_tx,
        input_rx,
        control_rx,
        withdraw_rx,
        cancel.clone(),
        None,
        session_context,
        notes_reminder,
        CcStreamState::default(),
        startup_follow_up,
    ));
    (
        RunningAgent {
            kind: CodingAgent::ClaudeCode,
            events_rx,
            input_tx,
            control_tx,
            permission_rx: None,
            withdraw_tx,
        },
        cancel,
    )
}

async fn output_ended_then_exited(
    events_rx: &mut mpsc::UnboundedReceiver<AgentEvent>,
    within: std::time::Duration,
) -> AgentEvent {
    let deadline = tokio::time::Instant::now() + within;
    let mut last_two = Vec::new();
    for _ in 0..2 {
        let ev = tokio::time::timeout_at(deadline, events_rx.recv())
            .await
            .expect("the driver ends its stream in time")
            .expect("events channel should be open");
        last_two.push(ev);
    }
    assert!(
        matches!(
            last_two.as_slice(),
            [AgentEvent::OutputEnded, AgentEvent::Exited { .. }]
        ),
        "got {last_two:?}"
    );
    last_two.pop().expect("two events read")
}

#[tokio::test]
async fn driver_task_parses_stdout_into_typed_events() {
    let cmd = format!(
        "printf '{}\\n{}\\n'",
        r#"{"type":"system","subtype":"init","session_id":"sess-1"}"#,
        r#"{"type":"result","result":"done","duration_ms":42}"#,
    );
    let (mut agent, _cancel) = spawn_driver_for_test("sh", &["-c", &cmd]).await;

    let init = tokio::time::timeout(std::time::Duration::from_secs(5), agent.events_rx.recv())
        .await
        .expect("driver should emit Init within 5s")
        .expect("events channel should be open");
    match init {
        AgentEvent::Init { session_id, .. } => assert_eq!(session_id, "sess-1"),
        other => panic!("expected Init, got {:?}", other),
    }

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), agent.events_rx.recv())
        .await
        .expect("driver should emit Result")
        .expect("events channel should be open");
    match result {
        AgentEvent::Result {
            text,
            duration_ms,
            error,
            ..
        } => {
            assert_eq!(text, "done");
            assert_eq!(duration_ms, 42);
            assert!(error.is_none());
        }
        other => panic!("expected Result, got {:?}", other),
    }

    let exited =
        output_ended_then_exited(&mut agent.events_rx, std::time::Duration::from_secs(5)).await;
    assert!(matches!(
        exited,
        AgentEvent::Exited {
            exit: AgentExitFacts {
                killed_by_signal: false,
                ..
            }
        }
    ));

    assert!(agent.events_rx.recv().await.is_none());
}

#[tokio::test]
async fn only_the_first_input_carries_the_owed_session_context() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("stdin.jsonl");
    let script = format!("head -n 3 > '{}'", out.display());
    let (agent, _cancel) = spawn_driver_owing(
        "sh",
        &["-c", &script],
        SessionContextDue::for_spawn(None, Some(&"SESSION CONTEXT: branch `b`".into())),
    )
    .await;
    for text in ["/cost", "first", "second"] {
        agent
            .input_tx
            .send(AgentInput {
                defer_on_steer_failure: false,
                text: text.to_string(),
                images: Vec::new(),
                uuid: uuid::Uuid::new_v4(),
            })
            .unwrap();
    }
    let mut events_rx = agent.events_rx;
    output_ended_then_exited(&mut events_rx, std::time::Duration::from_secs(5)).await;

    let written = std::fs::read_to_string(&out).unwrap();
    let texts: Vec<String> = written
        .lines()
        .map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            v["message"]["content"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(
        texts,
        ["/cost", "SESSION CONTEXT: branch `b`\n\nfirst", "second"],
        "{written}"
    );
}

fn input_text(text: &str) -> AgentInput {
    AgentInput {
        defer_on_steer_failure: false,
        text: text.to_string(),
        images: Vec::new(),
        uuid: uuid::Uuid::new_v4(),
    }
}

fn written_texts(out: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(out)
        .unwrap()
        .lines()
        .map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            v["message"]["content"].as_str().unwrap().to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_note_that_arrives_is_announced_to_the_agent_once() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("stdin.jsonl");
    let note = serde_json::json!({
        "type": "assistant",
        "message": {
            "model": "claude-opus-5-5",
            "content": [{"type": "thinking", "thinking": "Found it.", "signature": "s"}],
        },
    });
    let ping = r#"{"type":"stream_event","event":{"type":"content_block_stop"}}"#;
    let script = format!(
        "printf '%s\\n%s\\n' '{note}' '{ping}'; head -n 3 > '{}'",
        out.display()
    );
    let (mut agent, _cancel) =
        spawn_driver_owing_notes("sh", &["-c", &script], NotesReminderDue::default()).await;
    while !matches!(
        agent.events_rx.recv().await,
        Some(AgentEvent::StreamActivity)
    ) {}
    for text in ["/cost", "first", "second"] {
        agent.input_tx.send(input_text(text)).unwrap();
    }
    output_ended_then_exited(&mut agent.events_rx, std::time::Duration::from_secs(5)).await;

    assert_eq!(
        written_texts(&out),
        [
            "/cost".to_string(),
            format!(
                "{}\n\nfirst",
                crate::runtime::agent_notes::NOTES_NOW_SHOWN_REMINDER
            ),
            "second".to_string(),
        ]
    );
}

#[tokio::test]
async fn driver_task_cancellation_terminates_process() {
    let (mut agent, cancel) = spawn_driver_for_test("sh", &["-c", "sleep 30"]).await;

    cancel.cancel();

    let ended = tokio::time::timeout(std::time::Duration::from_secs(1), agent.events_rx.recv())
        .await
        .expect("OutputEnded well inside the teardown grace")
        .expect("events channel should be open");
    assert!(matches!(ended, AgentEvent::OutputEnded), "got {ended:?}");

    let deadline = GROUP_TEARDOWN_GRACE + std::time::Duration::from_secs(10);
    let exited = tokio::time::timeout(deadline, agent.events_rx.recv())
        .await
        .expect("driver should emit Exited within the cancel grace + headroom")
        .expect("events channel should be open");
    assert!(matches!(
        exited,
        AgentEvent::Exited {
            exit: AgentExitFacts {
                killed_by_signal: false,
                ..
            }
        }
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn driver_task_flags_stray_signal_kill() {
    let mut child = tokio::process::Command::new("sleep")
        .arg("30")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn test child");
    let pid = child.id().expect("child pid");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = StderrTail::spawn(child.stderr.take().expect("stderr"), Vec::new());
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let (_input_tx, input_rx) = mpsc::unbounded_channel();
    let (_control_tx, control_rx) = mpsc::unbounded_channel();
    let (_withdraw_tx, withdraw_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        child,
        stdin,
        BufReader::new(stdout),
        stderr,
        events_tx,
        input_rx,
        control_rx,
        withdraw_rx,
        cancel,
        None,
        SessionContextDue::default(),
        NotesReminderDue::default(),
        CcStreamState::default(),
        None,
    ));

    // External kill — the engine did NOT cancel. SAFETY: kill with a positive
    // pid + integer signal number; no pointer args.
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }

    let exited = output_ended_then_exited(&mut events_rx, std::time::Duration::from_secs(5)).await;
    assert!(
        matches!(
            exited,
            AgentEvent::Exited {
                exit: AgentExitFacts {
                    killed_by_signal: true,
                    ..
                }
            }
        ),
        "a stray signal kill must set killed_by_signal=true, got {:?}",
        exited,
    );
}

#[tokio::test]
async fn a_stray_kill_that_lands_after_stdout_closes_still_counts() {
    let (mut agent, _cancel) =
        spawn_driver_for_test("sh", &["-c", "exec 1>&-; sleep 0.3; kill -TERM $$"]).await;

    let exited =
        output_ended_then_exited(&mut agent.events_rx, std::time::Duration::from_secs(10)).await;
    assert!(
        matches!(
            exited,
            AgentEvent::Exited {
            exit: AgentExitFacts {
                killed_by_signal: true,
                ..
            }
        }
        ),
        "a process that kills itself after closing stdout must set killed_by_signal=true, got {exited:?}",
    );
}

#[cfg(unix)]
#[test]
fn format_exit_status_decodes_clean_exit() {
    use std::os::unix::process::ExitStatusExt;
    let s = std::process::ExitStatus::from_raw(0);
    assert_eq!(format_exit_status(&Ok(s)), "exit=0");
}

#[cfg(unix)]
#[test]
fn format_exit_status_decodes_plain_nonzero_exit() {
    use std::os::unix::process::ExitStatusExt;
    let s = std::process::ExitStatus::from_raw(256);
    assert_eq!(format_exit_status(&Ok(s)), "exit=1");
}

#[cfg(unix)]
#[test]
fn format_exit_status_decodes_sigterm_via_exit_code_143() {
    use std::os::unix::process::ExitStatusExt;
    let s = std::process::ExitStatus::from_raw(36608);
    assert_eq!(format_exit_status(&Ok(s)), "exit=143 (probable SIGTERM)",);
}

#[cfg(unix)]
#[test]
fn format_exit_status_decodes_sigkill_via_exit_code_137() {
    use std::os::unix::process::ExitStatusExt;
    let s = std::process::ExitStatus::from_raw(35072);
    assert_eq!(format_exit_status(&Ok(s)), "exit=137 (probable SIGKILL)",);
}

#[cfg(unix)]
#[test]
fn format_exit_status_decodes_direct_signal() {
    use std::os::unix::process::ExitStatusExt;
    let s = std::process::ExitStatus::from_raw(15);
    assert_eq!(format_exit_status(&Ok(s)), "signal=SIGTERM (15)");
}

#[cfg(unix)]
#[test]
fn format_exit_status_decodes_unknown_signal_number() {
    use std::os::unix::process::ExitStatusExt;
    let s = std::process::ExitStatus::from_raw(63);
    assert_eq!(format_exit_status(&Ok(s)), "signal=63");
}

#[cfg(unix)]
#[test]
fn exit_indicates_signal_kill_true_for_exit_143_and_137() {
    use std::os::unix::process::ExitStatusExt;
    assert!(exit_indicates_signal_kill(
        &std::process::ExitStatus::from_raw(143 << 8)
    ));
    assert!(exit_indicates_signal_kill(
        &std::process::ExitStatus::from_raw(137 << 8)
    ));
}

#[cfg(unix)]
#[test]
fn exit_indicates_signal_kill_true_for_direct_signal() {
    use std::os::unix::process::ExitStatusExt;
    assert!(exit_indicates_signal_kill(
        &std::process::ExitStatus::from_raw(15)
    ));
    assert!(exit_indicates_signal_kill(
        &std::process::ExitStatus::from_raw(9)
    ));
}

#[cfg(unix)]
#[test]
fn exit_indicates_signal_kill_false_for_clean_and_plain_exit() {
    use std::os::unix::process::ExitStatusExt;
    assert!(!exit_indicates_signal_kill(
        &std::process::ExitStatus::from_raw(0)
    ));
    assert!(!exit_indicates_signal_kill(
        &std::process::ExitStatus::from_raw(1 << 8)
    ));
}

#[test]
fn format_exit_status_decodes_wait_error() {
    let err = std::io::Error::other("permission denied");
    let result: std::io::Result<std::process::ExitStatus> = Err(err);
    let formatted = format_exit_status(&result);
    assert!(
        formatted.starts_with("wait_err: "),
        "wait error must be surfaced verbatim, got {formatted:?}",
    );
    assert!(formatted.contains("permission denied"));
}

#[tokio::test]
async fn driver_task_detects_subprocess_exit_when_grandchild_holds_stdout_busy() {
    let cmd = "echo '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-1\"}'; \
               (i=0; while [ $i -lt 50 ]; do echo noise; sleep 0.1; i=$((i+1)); done) & \
               exit 0";
    let (mut agent, _cancel) = spawn_driver_for_test("sh", &["-c", cmd]).await;

    let first = tokio::time::timeout(std::time::Duration::from_secs(2), agent.events_rx.recv())
        .await
        .expect("Init event must arrive within 2s")
        .expect("events channel closed before Init");
    assert!(
        matches!(first, AgentEvent::Init { .. }),
        "first event must be Init, got {:?}",
        first,
    );

    let last =
        output_ended_then_exited(&mut agent.events_rx, std::time::Duration::from_secs(2)).await;
    assert!(matches!(last, AgentEvent::Exited { .. }), "got {last:?}");
}

#[tokio::test]
async fn a_line_split_around_an_input_still_arrives_whole() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let half_written = tmp.path().join("half-written");
    let script = format!(
        r#"printf '%s' '{{"type":"result",'; touch '{marker}'; read reply; printf '%s\n' '"result":"done","duration_ms":7}}'"#,
        marker = half_written.display(),
    );
    let (mut agent, _cancel) = spawn_driver_for_test("sh", &["-c", &script]).await;

    let ceiling = std::time::Duration::from_secs(30);
    let started = std::time::Instant::now();
    while !half_written.exists() {
        assert!(
            started.elapsed() < ceiling,
            "the child never wrote its first half"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    agent
        .input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
            text: "go on".into(),
            images: vec![],
            uuid: uuid::Uuid::new_v4(),
        })
        .expect("send input");

    let mut events = Vec::new();
    loop {
        let ev = tokio::time::timeout(ceiling, agent.events_rx.recv())
            .await
            .expect("an event before the liveness ceiling")
            .expect("events channel open");
        let exited = matches!(ev, AgentEvent::Exited { .. });
        events.push(ev);
        if exited {
            break;
        }
    }
    assert!(
        events
            .iter()
            .any(|ev| matches!(ev, AgentEvent::Result { text, .. } if text == "done")),
        "the split line must arrive whole, got {events:?}"
    );
}

#[tokio::test]
async fn a_split_last_line_ended_by_stdout_eof_still_arrives() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let first = tmp.path().join("first-half");
    let second = tmp.path().join("second-half");
    let script = format!(
        r#"printf '%s' '{{"type":"result",'; touch '{first}'; read a; printf '%s' '"result":"done","duration_ms":7}}'; touch '{second}'; read b; exec 1>&-; sleep 5"#,
        first = first.display(),
        second = second.display(),
    );
    let (mut agent, _cancel) = spawn_driver_for_test("sh", &["-c", &script]).await;

    let ceiling = std::time::Duration::from_secs(30);
    for marker in [&first, &second] {
        let started = std::time::Instant::now();
        while !marker.exists() {
            assert!(
                started.elapsed() < ceiling,
                "the child never wrote {marker:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        agent
            .input_tx
            .send(AgentInput {
                defer_on_steer_failure: false,
                text: "go on".into(),
                images: vec![],
                uuid: uuid::Uuid::new_v4(),
            })
            .expect("send input");
    }

    let mut events = Vec::new();
    loop {
        let ev = tokio::time::timeout(ceiling, agent.events_rx.recv())
            .await
            .expect("an event before the liveness ceiling")
            .expect("events channel open");
        let exited = matches!(ev, AgentEvent::Exited { .. });
        events.push(ev);
        if exited {
            break;
        }
    }
    assert!(
        events
            .iter()
            .any(|ev| matches!(ev, AgentEvent::Result { text, .. } if text == "done")),
        "the last line must arrive, got {events:?}"
    );
}

#[tokio::test]
async fn a_call_open_when_the_process_dies_still_reports_its_usage() {
    let start = r#"{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"message_start","message":{"id":"msg_cut","model":"claude-sonnet-5","usage":{"input_tokens":2,"cache_read_input_tokens":40000,"cache_creation_input_tokens":300,"output_tokens":4}}}}"#;
    let script = format!("printf '%s\\n' '{start}'; exit 1");
    let (mut agent, _cancel) = spawn_driver_for_test("sh", &["-c", &script]).await;

    let mut events = Vec::new();
    loop {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(10), agent.events_rx.recv())
            .await
            .expect("an event before the timeout")
            .expect("events channel open");
        let exited = matches!(ev, AgentEvent::Exited { .. });
        events.push(ev);
        if exited {
            break;
        }
    }
    let usage = events.iter().position(|ev| {
        matches!(
            ev,
            AgentEvent::Usage {
                input_tokens: 2,
                cache_read_tokens: 40_000,
                cache_creation_tokens: 300,
                ..
            }
        )
    });
    assert!(
        usage.is_some_and(|i| i + 3 == events.len()),
        "the open call reports just ahead of OutputEnded and Exited, got {events:?}"
    );
}

#[test]
fn side_question_result_reads_the_answer_and_every_failure() {
    let answer = |line: &str| session_copy_result(line).map(|(answer, _)| answer);
    let ok = r#"{"type":"result","subtype":"success","is_error":false,"result":" forty-two \n"}"#;
    assert_eq!(answer(ok), Some(Ok("forty-two".to_string())));
    let turns = r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#;
    let Some(Err(message)) = answer(turns) else {
        panic!("a spent turn budget is a failure");
    };
    assert!(message.contains("reaching for tools"), "got {message}");
    let empty = r#"{"type":"result","subtype":"success","is_error":false,"result":""}"#;
    assert!(matches!(answer(empty), Some(Err(_))));
    assert_eq!(
        answer(r#"{"type":"assistant","message":{"content":"result"}}"#),
        None
    );
}

#[test]
fn side_question_result_keeps_the_usage_as_a_total() {
    let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"a","usage":{"input_tokens":2,"cache_read_input_tokens":17925,"cache_creation_input_tokens":625,"output_tokens":40}}"#;
    let (_, usage) = session_copy_result(line).unwrap();
    let usage = usage.expect("usage reported");
    assert_eq!(usage.input_tokens, 2 + 17925 + 625);
    assert_eq!(usage.cache_read_tokens, 17925);
    assert_eq!(usage.cache_creation_tokens, 625);
    assert_eq!(usage.output_tokens, 40);
}

#[test]
fn side_question_result_keeps_the_write_split() {
    use crate::engine::cache_ttl::{CacheLifetime, CacheTtl};
    let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"a","usage":{"input_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":625,"cache_creation":{"ephemeral_1h_input_tokens":625,"ephemeral_5m_input_tokens":0},"output_tokens":40}}"#;
    let (_, usage) = session_copy_result(line).unwrap();
    let usage = usage.expect("usage reported");
    assert_eq!(usage.cache_creation_1h_tokens, Some(625));
    assert_eq!(
        CacheTtl::of_write(&usage),
        CacheTtl::Reported(CacheLifetime::OneHour)
    );
}

#[test]
fn init_model_reads_only_the_init_line() {
    assert_eq!(
        init_model(r#"{"type":"system","subtype":"init","model":"claude-opus-5-5"}"#).as_deref(),
        Some("claude-opus-5-5")
    );
    assert_eq!(init_model(r#"{"type":"result","result":"init"}"#), None);
}

#[test]
fn a_side_question_message_carries_the_question_then_each_image() {
    let image = crate::engine::types::ChatImage {
        base64: "iVBORw0KGgo=".to_string(),
        mime_type: "image/png".to_string(),
    };
    let line = session_copy_message(
        &side_question_text("what is this?"),
        std::slice::from_ref(&image),
    );
    assert!(line.ends_with('\n'));
    let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(value["type"], "user");
    let content = value["message"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    let text = content[0]["text"].as_str().unwrap();
    assert!(text.starts_with("<system-reminder>This is a side question"));
    assert!(text.ends_with("what is this?"));
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["source"]["type"], "base64");
}

const SIDE_QUESTION_REPLAY: &str =
    r#"{"type":"user","message":{"role":"user","content":"q"},"isReplay":true}"#;

fn answer_side_question_script(log: &Path, answer: &str) -> String {
    format!(
        r#"while read -r l; do printf '%s\n' "$l" >> {log}; case "$l" in *control_request*) continue;; esac; break; done; printf '{SIDE_QUESTION_REPLAY}\n'; printf '{{"type":"result","subtype":"success","is_error":false,"result":"{answer}"}}\n'"#,
        log = log.display(),
    )
}

fn withdraw_reply(body: &str) -> InputWithdrawal {
    let (_, body) = control_reply(body).expect("a control_response line");
    withdrawal(body)
}

#[test]
fn a_withdraw_reply_reads_each_answer() {
    assert_eq!(
        withdraw_reply(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"r","response":{"cancelled":true}}}"#
        ),
        InputWithdrawal::Withdrawn
    );
    assert_eq!(
        withdraw_reply(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"r","response":{"cancelled":false}}}"#
        ),
        InputWithdrawal::AlreadyRead
    );
    assert!(matches!(
        withdraw_reply(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"r","response":{}}}"#
        ),
        InputWithdrawal::Refused(_)
    ));
    let InputWithdrawal::Refused(why) = withdraw_reply(
        r#"{"type":"control_response","response":{"subtype":"error","request_id":"r","error":"Unsupported control request subtype"}}"#,
    ) else {
        panic!("an error reply is a refusal");
    };
    assert!(why.contains("Unsupported control request subtype"), "{why}");
}

#[tokio::test]
async fn a_withdraw_never_overtakes_its_input() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("stdin.log");
    let script = format!(
        r#"read a; printf '%s\n' "$a" >> {log}; read b; printf '%s\n' "$b" >> {log}; id=$(printf '%s' "$b" | sed 's/.*"request_id":"\([^"]*\)".*/\1/'); printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{"cancelled":true}}}}}}\n' "$id"; sleep 5"#,
        log = log.display(),
    );
    let (mut agent, cancel) = spawn_driver_for_test("sh", &["-c", &script]).await;
    let input_uuid = uuid::Uuid::new_v4();
    let (reply, answer) = tokio::sync::oneshot::channel();
    agent
        .input_tx
        .send(AgentInput {
            defer_on_steer_failure: false,
            text: "never mind this".into(),
            images: vec![],
            uuid: input_uuid,
        })
        .unwrap();
    agent
        .withdraw_tx
        .send(WithdrawRequest { input_uuid, reply })
        .unwrap();
    let answer = tokio::time::timeout(std::time::Duration::from_secs(10), answer)
        .await
        .expect("answered within 10s")
        .expect("reply not dropped");
    assert_eq!(answer, InputWithdrawal::Withdrawn);
    cancel.cancel();

    let stdin_log = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = stdin_log.lines().collect();
    assert_eq!(lines.len(), 2, "the input, then the cancel: {stdin_log}");
    assert!(lines[0].contains(r#""type":"user""#), "{stdin_log}");
    assert!(lines[0].contains(&input_uuid.to_string()), "{stdin_log}");
    assert!(
        lines[1].contains(r#""subtype":"cancel_async_message""#),
        "{stdin_log}"
    );
    assert!(lines[1].contains(&input_uuid.to_string()), "{stdin_log}");
    while let Some(ev) =
        tokio::time::timeout(std::time::Duration::from_secs(15), agent.events_rx.recv())
            .await
            .expect("driver finishes")
    {
        assert!(
            matches!(ev, AgentEvent::OutputEnded | AgentEvent::Exited { .. }),
            "the withdraw reply must never reach the session: {ev:?}"
        );
    }
}

fn spawn_cold_fake(script: &str) -> Child {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.args(["-c", script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::runtime::spawn_env::isolate_in_process_group(&mut cmd);
    cmd.spawn().expect("spawn fake")
}

fn deadline_in(wait: std::time::Duration) -> tokio::time::Instant {
    tokio::time::Instant::now() + wait
}

async fn wait_for_file(path: &Path) {
    let started = std::time::Instant::now();
    while !std::fs::read_to_string(path).is_ok_and(|s| !s.trim().is_empty()) {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "the stand-in never wrote {path:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[cfg(unix)]
async fn process_gone(pid: i32) -> bool {
    for _ in 0..40 {
        // SAFETY: signal 0 only probes; no pointer arguments.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

const TIMED_OUT: &str = "the copy did not answer in time";

#[tokio::test]
async fn a_side_question_is_answered_past_unrelated_lines() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("stdin.log");
    let script = format!(
        r#"printf '{{"type":"system","subtype":"init","session_id":"s","model":"m"}}\n'; {}; sleep 30"#,
        answer_side_question_script(&log, "from the transcript")
    );
    let child = spawn_cold_fake(&script);
    let pid = child.id().unwrap() as i32;
    let answer = ask_session_copy_of(
        child,
        "q\n",
        deadline_in(std::time::Duration::from_secs(5)),
        TIMED_OUT,
    )
    .await;
    let reply = answer.unwrap();
    assert_eq!(reply.answer.unwrap(), "from the transcript");
    assert_eq!(reply.model.as_deref(), Some("m"));
    #[cfg(unix)]
    assert!(
        process_gone(pid).await,
        "the side-question process must not outlive the answer"
    );
    let stdin_log = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        stdin_log.lines().count(),
        3,
        "billing requests and the side question: {stdin_log}"
    );
}

#[tokio::test]
async fn session_copy_billing_comes_from_its_own_control_replies() {
    use crate::runtime::billing_provider::BillingAttribution;
    let initialize = |provider: &str| {
        serde_json::json!({
        "type":"control_response", "response":{
            "request_id":BILLING_INITIALIZE_REQUEST_ID, "subtype":"success",
            "response":{"account":{"apiProvider":provider,"apiKeySource":"none","tokenSource":"claude-login"}}
        }
    }).to_string()
    };
    let status = |custom: bool| {
        let mut rows = vec![
            serde_json::json!({"label":"Version","value":"fixture"}),
            serde_json::json!({"label":"Session ID","value":"fixture"}),
            serde_json::json!({"label":"cwd","value":"/tmp/fixture"}),
        ];
        if custom {
            rows.push(
                serde_json::json!({"label":"Anthropic base URL","value":"https://example.com"}),
            );
        }
        serde_json::json!({"type":"control_response","response":{
            "request_id":BILLING_STATUS_REQUEST_ID,"subtype":"success",
            "response":{"sections":[{"title":"Session","rows":rows}]}
        }})
        .to_string()
    };
    for (frames, expected) in [
        (vec![initialize("gateway")], "claude-gateway"),
        (
            vec![initialize("firstParty"), status(false)],
            "anthropic-subscription",
        ),
        (vec![status(false), initialize("vertex")], "vertex"),
        (vec![initialize("firstParty")], "unknown"),
        (
            vec![initialize("firstParty"), status(false), status(true)],
            "unknown",
        ),
        (vec![], "unknown"),
    ] {
        let mut script = String::from("read -r init; read -r status; read -r question; ");
        for frame in frames {
            script.push_str(&format!("printf '%s\\n' '{frame}'; "));
        }
        script.push_str(&format!("printf '%s\\n' '{SIDE_QUESTION_REPLAY}'; "));
        script.push_str(r#"printf '%s\n' '{"type":"result","subtype":"error_max_turns","usage":{"input_tokens":10,"output_tokens":2}}'; sleep 30"#);
        let reply = ask_session_copy_of(
            spawn_cold_fake(&script),
            "q\n",
            deadline_in(std::time::Duration::from_secs(5)),
            TIMED_OUT,
        )
        .await
        .unwrap();
        assert!(reply.answer.is_err());
        assert_eq!(reply.usage.unwrap().input_tokens, 10);
        assert_eq!(reply.billing.provider_id(), expected);
        if expected == "unknown" {
            assert!(matches!(
                reply.billing,
                BillingAttribution::Unresolved { .. }
            ));
        }
    }
}

#[tokio::test]
async fn a_side_question_skips_a_result_from_before_it_was_read() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("stdin.log");
    let script = format!(
        r#"printf '{{"type":"system","subtype":"task_notification","status":"stopped"}}\n'; printf '{{"type":"result","subtype":"success","is_error":false,"num_turns":0,"result":""}}\n'; {}; sleep 30"#,
        answer_side_question_script(&log, "after the notice")
    );
    let child = spawn_cold_fake(&script);
    let reply = ask_session_copy_of(
        child,
        "q\n",
        deadline_in(std::time::Duration::from_secs(5)),
        TIMED_OUT,
    )
    .await
    .unwrap();
    assert_eq!(reply.answer.unwrap(), "after the notice");
}

#[cfg(unix)]
#[tokio::test]
async fn a_side_question_that_never_answers_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let grandchild = dir.path().join("grandchild.pid");
    let script = format!(
        "sleep 30 & echo $! > {}; cat > /dev/null",
        grandchild.display()
    );
    let child = spawn_cold_fake(&script);
    let pid = child.id().unwrap() as i32;
    wait_for_file(&grandchild).await;
    let outcome = ask_session_copy_of(
        child,
        "q\n",
        deadline_in(std::time::Duration::from_millis(500)),
        TIMED_OUT,
    )
    .await;
    let err = outcome.expect_err("times out").to_string();
    assert!(err.contains("did not answer"), "got {err}");
    assert!(
        process_gone(pid).await,
        "the side-question process is reaped"
    );
    let grandchild_pid: i32 = std::fs::read_to_string(&grandchild)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        process_gone(grandchild_pid).await,
        "the group kill reaches the grandchild"
    );
}

#[tokio::test]
async fn a_side_question_process_that_exits_first_reports_it() {
    let child = spawn_cold_fake("read l; exit 3");
    let err = ask_session_copy_of(
        child,
        "q\n",
        deadline_in(std::time::Duration::from_secs(5)),
        TIMED_OUT,
    )
    .await
    .expect_err("no answer")
    .to_string();
    assert!(err.contains("exited before it answered"), "got {err}");
}

#[cfg(unix)]
#[tokio::test]
async fn an_abandoned_side_question_kills_its_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let grandchild = dir.path().join("grandchild.pid");
    let script = format!(
        "sleep 30 & echo $! > {}; cat > /dev/null",
        grandchild.display()
    );
    let child = spawn_cold_fake(&script);
    let pid = child.id().unwrap() as i32;
    wait_for_file(&grandchild).await;
    let asked = ask_session_copy_of(
        child,
        "q\n",
        deadline_in(std::time::Duration::from_secs(60)),
        TIMED_OUT,
    );
    let abandoned = tokio::time::timeout(std::time::Duration::from_millis(500), asked).await;
    assert!(abandoned.is_err(), "the asker gave up before any answer");
    assert!(
        process_gone(pid).await,
        "the side-question process dies with its asker"
    );
    let grandchild_pid: i32 = std::fs::read_to_string(&grandchild)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        process_gone(grandchild_pid).await,
        "the group kill reaches the grandchild"
    );
}

fn answer_initialize_script(log: &Path) -> String {
    let reply = r#"{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"account":{"email":"someone@example.com"},"models":[{"value":"default","resolvedModel":"claude-opus-5-5[1m]","displayName":"Default","description":"d","supportsEffort":true,"supportedEffortLevels":["low","high"]},{"value":"haiku","displayName":"Haiku 4.5","description":"h"}]}}}"#;
    format!(
        r#"read l; printf '%s\n' "$l" >> {log}; id=$(printf '%s' "$l" | sed 's/.*"request_id":"\([^"]*\)".*/\1/'); printf '{reply}\n' "$id""#,
        log = log.display(),
    )
}

#[tokio::test]
async fn the_model_probe_sends_only_initialize_and_reads_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("stdin.log");
    let script = format!(
        r#"printf '{{"type":"system","subtype":"hook_started"}}\n'; {}; sleep 30"#,
        answer_initialize_script(&log)
    );
    let child = spawn_cold_fake(&script);
    let pid = child.id().unwrap() as i32;
    let models = probe_models_of(child, deadline_in(std::time::Duration::from_secs(5)))
        .await
        .unwrap();
    let values: Vec<&str> = models.iter().map(|m| m.value.as_str()).collect();
    assert_eq!(values, ["default", "haiku"]);
    #[cfg(unix)]
    assert!(
        process_gone(pid).await,
        "the probe must not outlive the reply"
    );
    let stdin_log = std::fs::read_to_string(&log).unwrap();
    assert_eq!(stdin_log.lines().count(), 1, "one line only: {stdin_log}");
    let sent: serde_json::Value = serde_json::from_str(stdin_log.trim()).unwrap();
    assert_eq!(sent["type"], "control_request");
    assert_eq!(
        sent["request"],
        serde_json::json!({ "subtype": "initialize" })
    );
}

#[tokio::test]
async fn a_model_probe_that_never_answers_times_out() {
    let child = spawn_cold_fake("cat > /dev/null");
    let err = probe_models_of(child, deadline_in(std::time::Duration::from_millis(300)))
        .await
        .expect_err("times out")
        .to_string();
    assert!(err.contains("did not answer initialize"), "got {err}");
}

#[tokio::test]
async fn a_refused_initialize_reports_claude_codes_error() {
    let script = r#"read l; id=$(printf '%s' "$l" | sed 's/.*"request_id":"\([^"]*\)".*/\1/'); printf '{"type":"control_response","response":{"subtype":"error","request_id":"%s","error":"not now"}}\n' "$id"; sleep 30"#;
    let child = spawn_cold_fake(script);
    let err = probe_models_of(child, deadline_in(std::time::Duration::from_secs(5)))
        .await
        .expect_err("refused")
        .to_string();
    assert_eq!(err, "not now");
}

const GATEWAY_MESSAGE: &str = "Couldn't load settings from Cloud gateway \
https://claude-gateway.example.com. Check your network connection, or sign in again with /login.";

#[tokio::test]
async fn a_start_failure_reports_its_exit_code_and_words_with_no_init() {
    let script = format!("echo \"{GATEWAY_MESSAGE}\" >&2; exit 1");
    let (mut agent, _cancel) = spawn_driver_for_test("sh", &["-c", &script]).await;

    let exited =
        output_ended_then_exited(&mut agent.events_rx, std::time::Duration::from_secs(10)).await;
    let AgentEvent::Exited { exit } = exited else {
        panic!("Exited is last");
    };
    assert_eq!(exit.code, Some(1));
    assert_eq!(exit.stderr_tail, GATEWAY_MESSAGE);
    assert!(!exit.killed_by_signal);
    assert!(agent.events_rx.recv().await.is_none());
}

#[tokio::test]
async fn a_noisy_stderr_never_blocks_the_child() {
    let script = format!(
        "yes x | head -c 200000 >&2; printf '{}\\n{}\\n'; echo last-words >&2",
        r#"{"type":"system","subtype":"init","session_id":"sess-1"}"#,
        r#"{"type":"result","result":"done","duration_ms":42}"#,
    );
    let (mut agent, _cancel) = spawn_driver_for_test("sh", &["-c", &script]).await;

    let init = tokio::time::timeout(std::time::Duration::from_secs(10), agent.events_rx.recv())
        .await
        .expect("a full stderr pipe must not block the child's stdout")
        .expect("events channel should be open");
    assert!(matches!(init, AgentEvent::Init { .. }), "got {init:?}");
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), agent.events_rx.recv())
        .await
        .expect("result arrives")
        .expect("events channel should be open");
    assert!(
        matches!(result, AgentEvent::Result { .. }),
        "got {result:?}"
    );
    let exited =
        output_ended_then_exited(&mut agent.events_rx, std::time::Duration::from_secs(10)).await;
    let AgentEvent::Exited { exit } = exited else {
        panic!("Exited is last");
    };
    assert!(
        exit.stderr_tail.ends_with("last-words"),
        "got {:?}",
        exit.stderr_tail
    );
    assert!(exit.stderr_tail.len() <= crate::runtime::spawn_env::STDERR_TAIL_BYTES);
    assert_eq!(exit.code, Some(0));
}

#[tokio::test]
async fn an_engine_cancel_reports_no_exit_code() {
    let (mut agent, cancel) = spawn_driver_for_test("sh", &["-c", "sleep 30"]).await;
    cancel.cancel();
    let exited = output_ended_then_exited(
        &mut agent.events_rx,
        GROUP_TEARDOWN_GRACE + std::time::Duration::from_secs(10),
    )
    .await;
    let AgentEvent::Exited { exit } = exited else {
        panic!("Exited is last");
    };
    assert_eq!(exit.code, None);
}

#[tokio::test]
async fn the_first_result_checks_the_startup_files_against_the_transcript() {
    let workspace = tempfile::tempdir().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    std::fs::write(worktree.path().join("CLAUDE.md"), "# Rules\n").unwrap();
    let decide = || {
        startup_instructions::decide(
            workspace.path(),
            worktree.path(),
            config.path(),
            None,
            Some("2.1.300"),
            None,
        )
    };
    let StartupDecision::Keep(Some(check)) = decide() else {
        panic!("a new key asks for a check");
    };
    let cwd = std::fs::canonicalize(worktree.path()).unwrap();
    let transcripts = config.path().join("projects").join(
        cwd.to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>(),
    );
    std::fs::create_dir_all(&transcripts).unwrap();
    let recorded = serde_json::json!({
        "type": "attachment",
        "attachment": {"type": "instructions", "files": [
            {"type": "Project", "path": cwd.join("CLAUDE.md"), "content": "# Rules"},
        ]},
    });
    std::fs::write(transcripts.join("sess-1.jsonl"), recorded.to_string()).unwrap();
    let script = format!(
        "printf '{}\\n{}\\n'",
        r#"{"type":"system","subtype":"init","session_id":"sess-1"}"#,
        r#"{"type":"result","result":"done","duration_ms":1}"#,
    );
    let (mut agent, _cancel) = spawn_driver_checking(
        "sh",
        &["-c", &script],
        SessionContextDue::default(),
        Some(StartupFollowUp::Check(check)),
    )
    .await;
    output_ended_then_exited_after_results(&mut agent.events_rx).await;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if matches!(decide(), StartupDecision::Move(_)) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the verified key never reached the ledger"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_first_result_records_a_moved_session() {
    let workspace = tempfile::tempdir().unwrap();
    let script = format!(
        "printf '{}\\n{}\\n'",
        r#"{"type":"system","subtype":"init","session_id":"sess-moved"}"#,
        r#"{"type":"result","result":"done","duration_ms":1}"#,
    );
    let (mut agent, _cancel) = spawn_driver_checking(
        "sh",
        &["-c", &script],
        SessionContextDue::default(),
        Some(StartupFollowUp::moved(workspace.path())),
    )
    .await;
    output_ended_then_exited_after_results(&mut agent.events_rx).await;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !startup_instructions::StartupLedger::session_moved(workspace.path(), "sess-moved") {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the moved session never reached the ledger"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn lines_forwarded_after_the_child_exits_still_settle_the_startup_follow_up() {
    let workspace = tempfile::tempdir().unwrap();
    let mut state = CcStreamState::default();
    let mut pending = PendingReplies::new();
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let mut session_id = None;
    let mut follow_up = Some(StartupFollowUp::moved(workspace.path()));
    for line in [
        r#"{"type":"system","subtype":"init","session_id":"sess-drained"}"#,
        r#"{"type":"result","result":"done","duration_ms":1}"#,
    ] {
        let events = route_stdout_line(
            &mut state,
            &mut pending,
            &mut NotesReminderDue::default(),
            line,
        );
        assert!(forward_stdout_events(
            events,
            &mut session_id,
            &mut follow_up,
            &events_tx
        ));
    }
    assert_eq!(session_id.as_deref(), Some("sess-drained"));
    assert!(follow_up.is_none(), "the result settles the follow-up once");
    assert!(events_rx.recv().await.is_some());

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while !startup_instructions::StartupLedger::session_moved(workspace.path(), "sess-drained") {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the drained session never reached the ledger"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

async fn output_ended_then_exited_after_results(
    events_rx: &mut mpsc::UnboundedReceiver<AgentEvent>,
) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while let Some(event) = tokio::time::timeout_at(deadline, events_rx.recv())
        .await
        .expect("the driver ends its stream in time")
    {
        if matches!(event, AgentEvent::Exited { .. }) {
            return;
        }
    }
}
