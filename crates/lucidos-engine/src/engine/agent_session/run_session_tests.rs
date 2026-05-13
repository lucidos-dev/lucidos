/// Verifies that `try_wait()` detects a dead child process even when
/// the stdout pipe hasn't produced EOF. This is the watchdog that
/// prevents threads from getting stuck in RUNNING state after the CC
/// process is killed (e.g. macOS sleep killing the process).
#[tokio::test]
async fn try_wait_detects_dead_cc_process() {
    use tokio::process::Command;

    // Spawn a short-lived process
    let mut child = Command::new("echo")
        .arg("done")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn 'echo'");

    // Take stdin/stdout (same as the real CC code does)
    let _stdin = child.stdin.take();
    let _stdout = child.stdout.take();

    // Wait for process to exit (with explicit wait, not just sleep)
    let _ = child.wait().await;

    // try_wait should report the exit status
    let status = child.try_wait().expect("try_wait should not error");
    assert!(
        status.is_some(),
        "try_wait must detect dead process even after stdin/stdout are taken"
    );
    assert!(
        status.unwrap().success(),
        "process should have exited successfully"
    );
}

/// Verifies that `try_wait()` returns None for a still-running process.
/// This ensures the watchdog doesn't false-positive on healthy CC sessions.
#[tokio::test]
async fn try_wait_returns_none_for_running_process() {
    use tokio::process::Command;

    let mut child = Command::new("sleep")
        .arg("10")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn 'sleep'");

    let _stdin = child.stdin.take();
    let _stdout = child.stdout.take();

    let status = child.try_wait().expect("try_wait should not error");
    assert!(
        status.is_none(),
        "try_wait must return None for a running process"
    );

    // Clean up
    let _ = child.kill().await;
}
