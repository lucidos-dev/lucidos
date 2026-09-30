use super::*;

/// Spawn an arbitrary subprocess and wire it to `driver_task` so we can
/// integration-test the channel plumbing without requiring the `claude`
/// CLI to be installed.
async fn spawn_driver_for_test(program: &str, args: &[&str]) -> (RunningAgent, CancellationToken) {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn test child");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let (withdraw_tx, withdraw_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        child,
        stdin,
        BufReader::new(stdout),
        BufReader::new(stderr),
        events_tx,
        input_rx,
        control_rx,
        withdraw_rx,
        cancel.clone(),
        None,
        CcStreamState::default(),
    ));
    (
        RunningAgent {
            kind: CodingAgent::ClaudeCode,
            events_rx,
            input_tx,
            control_tx,
            permission_rx: None,
            withdraw_tx: Some(withdraw_tx),
        },
        cancel,
    )
}

#[tokio::test]
async fn driver_task_parses_stdout_into_typed_events() {
    // Subprocess prints two CC-format lines then exits. The driver must
    // forward both as typed AgentEvents and finish with Exited.
    let cmd = format!(
        "printf '{}\\n{}\\n'",
        r#"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-1\"}"#,
        r#"{\"type\":\"result\",\"result\":\"done\",\"duration_ms\":42}"#,
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
        } => {
            assert_eq!(text, "done");
            assert_eq!(duration_ms, 42);
            assert!(error.is_none());
        }
        other => panic!("expected Result, got {:?}", other),
    }

    let exited = tokio::time::timeout(std::time::Duration::from_secs(5), agent.events_rx.recv())
        .await
        .expect("driver should emit Exited after EOF")
        .expect("events channel should be open");
    // Clean exit (printf then EOF) — not a signal kill.
    assert!(matches!(
        exited,
        AgentEvent::Exited {
            killed_by_signal: false
        }
    ));

    // Channel closes after Exited
    assert!(agent.events_rx.recv().await.is_none());
}

#[tokio::test]
async fn driver_task_cancellation_terminates_process() {
    // Spawn a long-running sleep — driver must kill it when cancel fires.
    let (mut agent, cancel) = spawn_driver_for_test("sh", &["-c", "sleep 30"]).await;

    cancel.cancel();

    // The cancel path is NOT instant: it runs `graceful_kill_child_process_group`,
    // whose `GROUP_TEARDOWN_GRACE` is a FIXED `tokio::time::sleep` (it can't
    // early-exit — the group leader is an unreaped zombie, so a liveness poll
    // never sees the group empty; see that function's doc comment). So `Exited`
    // cannot arrive before `GROUP_TEARDOWN_GRACE` elapses no matter how fast the
    // sleep dies. Budget the deadline as that fixed floor PLUS generous headroom
    // so the assertion tracks the real contract and stays robust under the timer
    // slippage of a fully-loaded test runner (a flat 5s left only ~2s over the
    // 3s floor and flaked under the full-suite concurrent load). A genuine
    // "never emits Exited" hang still fails — it just has an honest budget.
    let deadline = GROUP_TEARDOWN_GRACE + std::time::Duration::from_secs(10);
    let exited = tokio::time::timeout(deadline, agent.events_rx.recv())
        .await
        .expect("driver should emit Exited within the cancel grace + headroom")
        .expect("events channel should be open");
    // Engine-initiated cancel — the driver's own SIGKILL must NOT be reported
    // as a stray signal kill (that would wrongly trigger auto-resume).
    assert!(matches!(
        exited,
        AgentEvent::Exited {
            killed_by_signal: false
        }
    ));
}

/// A stray external signal that kills CC mid-run (the `exit=143` bug) must be
/// reported as `killed_by_signal: true` — distinct from the engine's own
/// cancel-path SIGKILL — so the safety net can auto-resume instead of
/// surfacing a red-dot abort. Sends SIGTERM directly to the child's pid, NOT
/// through the driver's `cancel` token.
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
    let stderr = child.stderr.take().expect("stderr");
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let (_input_tx, input_rx) = mpsc::unbounded_channel();
    let (_control_tx, control_rx) = mpsc::unbounded_channel();
    let (_withdraw_tx, withdraw_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        child,
        stdin,
        BufReader::new(stdout),
        BufReader::new(stderr),
        events_tx,
        input_rx,
        control_rx,
        withdraw_rx,
        cancel,
        None,
        CcStreamState::default(),
    ));

    // External kill — the engine did NOT cancel. SAFETY: kill with a positive
    // pid + integer signal number; no pointer args.
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }

    let exited = tokio::time::timeout(std::time::Duration::from_secs(5), events_rx.recv())
        .await
        .expect("driver should emit Exited within 5s of the kill")
        .expect("events channel should be open");
    assert!(
        matches!(
            exited,
            AgentEvent::Exited {
                killed_by_signal: true
            }
        ),
        "a stray signal kill must set killed_by_signal=true, got {:?}",
        exited,
    );
}

// ── format_exit_status ───────────────────────────────────────────────────
// Tests for the wait-status decoder. See the function's own doc comment
// in `runtime/claude_code.rs` for the case analysis.

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
    // Raw 256 = WIFEXITED with WEXITSTATUS=1. Below the 128+N range so no
    // probable-signal hint.
    let s = std::process::ExitStatus::from_raw(256);
    assert_eq!(format_exit_status(&Ok(s)), "exit=1");
}

#[cfg(unix)]
#[test]
fn format_exit_status_decodes_sigterm_via_exit_code_143() {
    use std::os::unix::process::ExitStatusExt;
    // Raw 36608 = 0x8F00 = WIFEXITED with WEXITSTATUS=143. This is the
    // exact value observed when Node.js (Claude Code's runtime) catches
    // SIGTERM and exits cleanly. The decoder MUST flag the probable
    // signal so debugging doesn't require manual 128+N arithmetic.
    let s = std::process::ExitStatus::from_raw(36608);
    assert_eq!(format_exit_status(&Ok(s)), "exit=143 (probable SIGTERM)",);
}

#[cfg(unix)]
#[test]
fn format_exit_status_decodes_sigkill_via_exit_code_137() {
    use std::os::unix::process::ExitStatusExt;
    // Raw 35072 = 0x8900 = exit 137 = 128 + 9 (SIGKILL) — macOS Jetsam /
    // OOM-killer convention.
    let s = std::process::ExitStatus::from_raw(35072);
    assert_eq!(format_exit_status(&Ok(s)), "exit=137 (probable SIGKILL)",);
}

#[cfg(unix)]
#[test]
fn format_exit_status_decodes_direct_signal() {
    use std::os::unix::process::ExitStatusExt;
    // Raw 15 = WIFSIGNALED with WTERMSIG=15 (SIGTERM). This is the path
    // taken when the child does NOT install a signal handler — kernel
    // delivers the signal as the cause of death directly. Distinct from
    // the "exit=143" case above where the child caught + re-raised.
    let s = std::process::ExitStatus::from_raw(15);
    assert_eq!(format_exit_status(&Ok(s)), "signal=SIGTERM (15)");
}

#[cfg(unix)]
#[test]
fn format_exit_status_decodes_unknown_signal_number() {
    use std::os::unix::process::ExitStatusExt;
    // Raw 63 = WIFSIGNALED with WTERMSIG=63 — outside the named set.
    // Decoder must still produce a useful string instead of silently
    // dropping the number.
    let s = std::process::ExitStatus::from_raw(63);
    assert_eq!(format_exit_status(&Ok(s)), "signal=63");
}

// ── exit_indicates_signal_kill ───────────────────────────────────────────
// The classifier the safety net keys off to auto-resume a stray-killed turn.

#[cfg(unix)]
#[test]
fn exit_indicates_signal_kill_true_for_exit_143_and_137() {
    use std::os::unix::process::ExitStatusExt;
    // exit=143 (Node caught SIGTERM, re-raised) and exit=137 (128+SIGKILL).
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
    // Raw 15 = WIFSIGNALED with WTERMSIG=15 (kernel-delivered SIGTERM).
    assert!(exit_indicates_signal_kill(
        &std::process::ExitStatus::from_raw(15)
    ));
    // Raw 9 = WTERMSIG=9 (SIGKILL).
    assert!(exit_indicates_signal_kill(
        &std::process::ExitStatus::from_raw(9)
    ));
}

#[cfg(unix)]
#[test]
fn exit_indicates_signal_kill_false_for_clean_and_plain_exit() {
    use std::os::unix::process::ExitStatusExt;
    // exit=0 (clean) and exit=1 (plain failure) are NOT signal kills.
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

/// Regression: when a Claude Code subprocess exits but a backgrounded grandchild
/// keeps stdout busy (e.g., `cargo` forks rustc, rustc inherits the pipe
/// and keeps streaming build progress while CC itself dies), the driver
/// must still detect parent exit and emit `AgentEvent::Exited` promptly.
///
/// Without an explicit `child.wait()` arm in the select! loop, the engine
/// relied on either stdout EOF (which the grandchild prevents by holding
/// the pipe open) or a 500ms `try_wait` poll. The poll arm never fires
/// when read_line stays continuously ready: tokio::select! re-creates
/// futures each iteration, so a noise line every ~100ms resets the 500ms
/// timer before it can resolve. The engine then sat at status='running'
/// forever — no `CodingAgentIdled`, no `ResponseAborted`, no terminal
/// event of any kind — until the grandchild eventually died on its own
/// (minutes, hours, or never).
///
/// The fix: poll `child.wait()` directly as a select! arm so the OS-level
/// exit signal triggers a break regardless of stdout state.
#[tokio::test]
async fn driver_task_detects_subprocess_exit_when_grandchild_holds_stdout_busy() {
    // sh script:
    //   1. echo the init JSON line
    //   2. background a grandchild that writes "noise\n" every ~100ms
    //      for ~5 seconds (longer than our 2-second test deadline)
    //   3. exit the parent shell immediately
    //
    // The grandchild inherits stdout, so the pipe stays open and busy
    // after the parent dies. The driver must catch the parent's exit
    // signal directly — relying on stdout EOF or the 500ms try_wait
    // poll alone leaves the test wedged for the grandchild's full
    // lifetime, exceeding the 2-second timeout.
    let cmd = "echo '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-1\"}'; \
               (i=0; while [ $i -lt 50 ]; do echo noise; sleep 0.1; i=$((i+1)); done) & \
               exit 0";
    let (mut agent, _cancel) = spawn_driver_for_test("sh", &["-c", cmd]).await;

    // First event: Init from the echo'd JSON.
    let first = tokio::time::timeout(std::time::Duration::from_secs(2), agent.events_rx.recv())
        .await
        .expect("Init event must arrive within 2s")
        .expect("events channel closed before Init");
    assert!(
        matches!(first, AgentEvent::Init { .. }),
        "first event must be Init, got {:?}",
        first,
    );

    // Second event: Exited.
    //
    // "noise" lines are not valid CC JSON, so parse_line returns no
    // events for them — the events channel sees Init then Exited
    // with no intermediate events. The parent shell exits within
    // milliseconds of the echo; the OS delivers SIGCHLD immediately,
    // and child.wait() in the select! loop resolves on the next
    // poll. We allow 2 seconds to tolerate CI scheduler jitter.
    //
    // Without the child.wait() arm, this assertion times out after
    // 2 seconds — the grandchild's 100ms noise cadence outpaces the
    // 500ms try_wait poll's re-arm cycle, and the driver never
    // notices the parent died.
    let second = tokio::time::timeout(std::time::Duration::from_secs(2), agent.events_rx.recv())
        .await
        .expect(
            "AgentEvent::Exited did not arrive within 2s of subprocess death. \
             The grandchild keeps stdout busy with a noise line every ~100ms, \
             starving the (removed) 500ms try_wait poll. driver_task needs \
             `child.wait()` as a direct select! arm to detect parent exit \
             regardless of stdout state — without it, the engine wedges at \
             status='running' forever.",
        )
        .expect("events channel closed without Exited");
    assert!(
        matches!(second, AgentEvent::Exited { .. }),
        "second event must be Exited (grandchild noise is unparseable and \
         produces no events), got {:?}",
        second,
    );
}

/// A line that arrives in two chunks survives an input sent between them.
///
/// The child writes half a `result` line and blocks on stdin. The input wins
/// the driver's select while the read holds that half, so the read is dropped.
/// The child then writes the rest. A read that loses the half leaves only an
/// unparseable tail, and no `Result` ever arrives.
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
    // Time for the driver to read the half into its line buffer.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    agent
        .input_tx
        .send(AgentInput {
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

/// The stdout-EOF twin of the test above. Two inputs each drop a read: the
/// first holds the start of the line, the second holds its newline-less end.
/// The child then closes stdout while alive, so the next read returns `Ok(0)`
/// with the whole line already in the buffer.
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
        // Time for the driver to read the bytes into its line buffer.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        agent
            .input_tx
            .send(AgentInput {
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

// ── side questions ───────────────────────────────────────────────────────

#[test]
fn side_question_result_reads_the_answer_and_every_failure() {
    let answer = |line: &str| side_question_result(line).map(|(answer, _)| answer);
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

/// The copy is a real model call, so its result's usage is kept for the cost
/// record. Claude Code reports the uncached input alone, and the recorded
/// input is the total, as the main session's capture records it.
#[test]
fn side_question_result_keeps_the_usage_as_a_total() {
    let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"a","usage":{"input_tokens":2,"cache_read_input_tokens":17925,"cache_creation_input_tokens":625,"output_tokens":40}}"#;
    let (_, usage) = side_question_result(line).unwrap();
    let usage = usage.expect("usage reported");
    assert_eq!(usage.input_tokens, 2 + 17925 + 625);
    assert_eq!(usage.cache_read_tokens, 17925);
    assert_eq!(usage.cache_creation_tokens, 625);
    assert_eq!(usage.output_tokens, 40);
}

#[test]
fn init_model_reads_only_the_init_line() {
    assert_eq!(
        init_model(r#"{"type":"system","subtype":"init","model":"claude-opus-5-5"}"#).as_deref(),
        Some("claude-opus-5-5")
    );
    assert_eq!(init_model(r#"{"type":"result","result":"init"}"#), None);
}

/// The one line a side question writes: the framing and the question in one
/// text block, then each image as a base64 block.
#[test]
fn side_question_message_carries_the_question_then_each_image() {
    let image = crate::api::ChatImage {
        base64: "iVBORw0KGgo=".to_string(),
        mime_type: "image/png".to_string(),
    };
    let line = side_question_message("what is this?", std::slice::from_ref(&image));
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

/// The replay Claude Code prints when it reads the side question.
const SIDE_QUESTION_REPLAY: &str =
    r#"{"type":"user","message":{"role":"user","content":"q"},"isReplay":true}"#;

/// A shell stand-in for Claude Code: it logs the line it reads to `log`,
/// replays it, then answers with a `result` whose text is `answer`.
fn answer_side_question_script(log: &Path, answer: &str) -> String {
    format!(
        r#"read l; printf '%s\n' "$l" >> {log}; printf '{SIDE_QUESTION_REPLAY}\n'; printf '{{"type":"result","subtype":"success","is_error":false,"result":"{answer}"}}\n'"#,
        log = log.display(),
    )
}

// ── withdraws ────────────────────────────────────────────────────────────

fn withdraw_reply(body: &str) -> InputWithdrawal {
    let (_, body) = control_reply(body).expect("a control_response line");
    withdrawal(body)
}

/// Only `cancelled: true` withdraws. Every other answer is a refusal to report.
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

/// The input sent right before its withdraw is written first, and the cancel
/// names that input's uuid. The reply reaches the caller, never the session.
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
            text: "never mind this".into(),
            images: vec![],
            uuid: input_uuid,
        })
        .unwrap();
    agent
        .withdraw_tx
        .as_ref()
        .expect("Claude Code takes withdraws")
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
            matches!(ev, AgentEvent::Exited { .. }),
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

/// Wait until the stand-in has written `path`, pid and all. A short deadline
/// that starts first can kill a shell still starting on a loaded host.
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

/// True once `pid` names no process. Polls briefly: an orphan zombie lingers
/// until init reaps it.
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
    let answer =
        ask_side_question_of(child, "q\n", deadline_in(std::time::Duration::from_secs(5))).await;
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
        1,
        "only the side question: {stdin_log}"
    );
}

/// A resumed session with a background task still running reports the task
/// stopped and closes that with an empty `result` of its own. That `result`
/// comes before Claude Code reads the question, so it is not the answer.
#[tokio::test]
async fn a_side_question_skips_a_result_from_before_it_was_read() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("stdin.log");
    let script = format!(
        r#"printf '{{"type":"system","subtype":"task_notification","status":"stopped"}}\n'; printf '{{"type":"result","subtype":"success","is_error":false,"num_turns":0,"result":""}}\n'; {}; sleep 30"#,
        answer_side_question_script(&log, "after the notice")
    );
    let child = spawn_cold_fake(&script);
    let reply = ask_side_question_of(child, "q\n", deadline_in(std::time::Duration::from_secs(5)))
        .await
        .unwrap();
    assert_eq!(reply.answer.unwrap(), "after the notice");
}

/// The side-question process never outlives the request: a timeout kills its whole
/// process group, grandchildren included.
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
    let outcome = ask_side_question_of(
        child,
        "q\n",
        deadline_in(std::time::Duration::from_millis(500)),
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
    let err = ask_side_question_of(child, "q\n", deadline_in(std::time::Duration::from_secs(5)))
        .await
        .expect_err("no answer")
        .to_string();
    assert!(err.contains("exited before it answered"), "got {err}");
}

/// An asker that goes away mid-question, such as an HTTP request the browser
/// abandoned, still takes the side-question process group down with it.
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
    let asked = ask_side_question_of(
        child,
        "q\n",
        deadline_in(std::time::Duration::from_secs(60)),
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

// ── model probe ──────────────────────────────────────────────────────────

/// A shell stand-in for Claude Code that answers the first line on stdin with
/// an `initialize` reply carrying an account and two models. Lines it reads go
/// to `log`.
fn answer_initialize_script(log: &Path) -> String {
    let reply = r#"{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"account":{"email":"someone@example.com"},"models":[{"value":"default","resolvedModel":"claude-opus-5-5[1m]","displayName":"Default","description":"d","supportsEffort":true,"supportedEffortLevels":["low","high"]},{"value":"haiku","displayName":"Haiku 4.5","description":"h"}]}}}"#;
    format!(
        r#"read l; printf '%s\n' "$l" >> {log}; id=$(printf '%s' "$l" | sed 's/.*"request_id":"\([^"]*\)".*/\1/'); printf '{reply}\n' "$id""#,
        log = log.display(),
    )
}

/// The probe writes exactly one line, an `initialize` request, and never a
/// prompt, so it costs no tokens. Hook frames before the reply are skipped.
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

/// A Claude Code that never answers is killed at the deadline, and the error
/// names what it was waiting for.
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
