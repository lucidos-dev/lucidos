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
    let (side_question_tx, side_question_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        child,
        stdin,
        BufReader::new(stdout),
        BufReader::new(stderr),
        events_tx,
        input_rx,
        control_rx,
        side_question_rx,
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
            side_question_tx: Some(side_question_tx),
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
    let (_side_question_tx, side_question_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        child,
        stdin,
        BufReader::new(stdout),
        BufReader::new(stderr),
        events_tx,
        input_rx,
        control_rx,
        side_question_rx,
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
fn side_question_reply_reads_success_and_error() {
    let ok = r#"{"type":"control_response","response":{"subtype":"success","request_id":"r1","response":{"response":"forty-two","synthetic":false}}}"#;
    assert_eq!(
        side_question_reply(ok),
        Some(("r1".to_string(), Ok("forty-two".to_string())))
    );
    let err = r#"{"type":"control_response","response":{"subtype":"error","request_id":"r2","error":"no context"}}"#;
    assert_eq!(
        side_question_reply(err),
        Some(("r2".to_string(), Err("no context".to_string())))
    );
    assert_eq!(
        side_question_reply(r#"{"type":"result","result":"x"}"#),
        None
    );
}

/// A shell stand-in for Claude Code that answers the first `side_question`
/// on stdin with `answer`, echoing its request id. Lines it reads go to `log`.
fn answer_side_question_script(log: &Path, answer: &str) -> String {
    format!(
        r#"read l; printf '%s\n' "$l" >> {log}; id=$(printf '%s' "$l" | sed 's/.*"request_id":"\([^"]*\)".*/\1/'); printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{"response":"{answer}","synthetic":false}}}}}}\n' "$id""#,
        log = log.display(),
    )
}

/// The running turn is never touched: a side question asked mid-turn writes
/// exactly one `side_question` line, is answered before the turn's `Result`,
/// and its answer reaches the asker only, never `events_rx`.
#[tokio::test]
async fn a_side_question_mid_turn_leaves_the_turn_alone() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("stdin.log");
    let script = format!(
        r#"read l; printf '%s\n' "$l" >> {log}; printf '{init}\n'; {answer}; printf '{result}\n'"#,
        log = log.display(),
        init = r#"{"type":"system","subtype":"init","session_id":"sess-1"}"#,
        answer = answer_side_question_script(&log, "forty-two"),
        result = r#"{"type":"result","result":"main done","duration_ms":5}"#,
    );
    let (mut agent, _cancel) = spawn_driver_for_test("sh", &["-c", &script]).await;
    agent
        .input_tx
        .send(AgentInput {
            text: "do the main work".into(),
            images: vec![],
        })
        .unwrap();
    let init = tokio::time::timeout(std::time::Duration::from_secs(5), agent.events_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(init, AgentEvent::Init { .. }), "got {init:?}");

    let (reply, answer) = tokio::sync::oneshot::channel();
    agent
        .side_question_tx
        .as_ref()
        .expect("Claude Code takes side questions")
        .send(SideQuestionRequest {
            question: "what is the codeword?".into(),
            reply,
        })
        .unwrap();
    let answer = tokio::time::timeout(std::time::Duration::from_secs(5), answer)
        .await
        .expect("answered within 5s")
        .expect("reply not dropped");
    assert_eq!(answer, Ok("forty-two".to_string()));

    let mut rest = Vec::new();
    while let Some(ev) =
        tokio::time::timeout(std::time::Duration::from_secs(5), agent.events_rx.recv())
            .await
            .expect("driver finishes")
    {
        rest.push(ev);
    }
    assert!(
        matches!(&rest[0], AgentEvent::Result { text, error: None, .. } if text == "main done"),
        "the turn's own Result follows unchanged, got {rest:?}"
    );
    assert!(
        !format!("{rest:?}").contains("forty-two"),
        "the side answer must never reach the session's events"
    );

    let stdin_log = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = stdin_log.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "one user line, one side question: {stdin_log}"
    );
    assert!(lines[0].contains(r#""type":"user""#));
    assert!(lines[1].contains(r#""subtype":"side_question""#));
    assert!(lines[1].contains("what is the codeword?"));
    assert!(!stdin_log.contains("interrupt"));
}

/// A process that exits with a side question pending drops its reply, which
/// tells the engine to fall back to a cold process.
#[tokio::test]
async fn a_pending_side_question_is_dropped_when_the_process_exits() {
    let (agent, _cancel) = spawn_driver_for_test("sh", &["-c", "read l; exit 0"]).await;
    let (reply, answer) = tokio::sync::oneshot::channel();
    agent
        .side_question_tx
        .as_ref()
        .unwrap()
        .send(SideQuestionRequest {
            question: "q".into(),
            reply,
        })
        .unwrap();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), answer)
        .await
        .expect("resolves once the driver ends");
    assert!(
        outcome.is_err(),
        "reply dropped unanswered, got {outcome:?}"
    );
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
async fn a_cold_side_question_is_answered_past_unrelated_lines() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("stdin.log");
    let script = format!(
        r#"printf '{{"type":"system","subtype":"init","session_id":"s"}}\n'; {}; sleep 30"#,
        answer_side_question_script(&log, "from the transcript")
    );
    let child = spawn_cold_fake(&script);
    let pid = child.id().unwrap() as i32;
    let answer =
        ask_side_question_of(child, "q", deadline_in(std::time::Duration::from_secs(5))).await;
    assert_eq!(answer.unwrap(), "from the transcript");
    #[cfg(unix)]
    assert!(
        process_gone(pid).await,
        "the cold process must not outlive the answer"
    );
    let stdin_log = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        stdin_log.lines().count(),
        1,
        "only the control request: {stdin_log}"
    );
    assert!(!stdin_log.contains(r#""type":"user""#));
}

/// The cold process never outlives the request: a timeout kills its whole
/// process group, grandchildren included.
#[cfg(unix)]
#[tokio::test]
async fn a_cold_side_question_that_never_answers_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let grandchild = dir.path().join("grandchild.pid");
    let script = format!(
        "sleep 30 & echo $! > {}; cat > /dev/null",
        grandchild.display()
    );
    let child = spawn_cold_fake(&script);
    let pid = child.id().unwrap() as i32;
    let outcome = ask_side_question_of(
        child,
        "q",
        deadline_in(std::time::Duration::from_millis(500)),
    )
    .await;
    let err = outcome.expect_err("times out").to_string();
    assert!(err.contains("did not answer"), "got {err}");
    assert!(process_gone(pid).await, "the cold process is reaped");
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
async fn a_cold_process_that_exits_first_reports_it() {
    let child = spawn_cold_fake("read l; exit 3");
    let err = ask_side_question_of(child, "q", deadline_in(std::time::Duration::from_secs(5)))
        .await
        .expect_err("no answer")
        .to_string();
    assert!(err.contains("exited before it answered"), "got {err}");
}

/// An asker that goes away mid-question, such as an HTTP request the browser
/// abandoned, still takes the cold process group down with it.
#[cfg(unix)]
#[tokio::test]
async fn an_abandoned_cold_side_question_kills_its_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let grandchild = dir.path().join("grandchild.pid");
    let script = format!(
        "sleep 30 & echo $! > {}; cat > /dev/null",
        grandchild.display()
    );
    let child = spawn_cold_fake(&script);
    let pid = child.id().unwrap() as i32;
    let asked = ask_side_question_of(child, "q", deadline_in(std::time::Duration::from_secs(60)));
    let abandoned = tokio::time::timeout(std::time::Duration::from_millis(500), asked).await;
    assert!(abandoned.is_err(), "the asker gave up before any answer");
    assert!(
        process_gone(pid).await,
        "the cold process dies with its asker"
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
