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
    let cancel = CancellationToken::new();
    tokio::spawn(driver_task(
        child,
        stdin,
        BufReader::new(stdout),
        BufReader::new(stderr),
        events_tx,
        input_rx,
        control_rx,
        cancel.clone(),
        None,
    ));
    (
        RunningAgent {
            kind: CodingAgent::ClaudeCode,
            events_rx,
            input_tx,
            control_tx,
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
    assert!(matches!(exited, AgentEvent::Exited));

    // Channel closes after Exited
    assert!(agent.events_rx.recv().await.is_none());
}

#[tokio::test]
async fn driver_task_cancellation_terminates_process() {
    // Spawn a long-running sleep — driver must kill it when cancel fires.
    let (mut agent, cancel) = spawn_driver_for_test("sh", &["-c", "sleep 30"]).await;

    cancel.cancel();

    let exited = tokio::time::timeout(std::time::Duration::from_secs(5), agent.events_rx.recv())
        .await
        .expect("driver should emit Exited within 5s of cancellation")
        .expect("events channel should be open");
    assert!(matches!(exited, AgentEvent::Exited));
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
    assert_eq!(
        format_exit_status(&Ok(s)),
        "exit=143 (probable SIGTERM)",
    );
}

#[cfg(unix)]
#[test]
fn format_exit_status_decodes_sigkill_via_exit_code_137() {
    use std::os::unix::process::ExitStatusExt;
    // Raw 35072 = 0x8900 = exit 137 = 128 + 9 (SIGKILL) — macOS Jetsam /
    // OOM-killer convention.
    let s = std::process::ExitStatus::from_raw(35072);
    assert_eq!(
        format_exit_status(&Ok(s)),
        "exit=137 (probable SIGKILL)",
    );
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
        matches!(second, AgentEvent::Exited),
        "second event must be Exited (grandchild noise is unparseable and \
         produces no events), got {:?}",
        second,
    );
}
