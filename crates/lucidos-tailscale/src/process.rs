//! Running a subprocess with a hard deadline.

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Stdout beyond this is dropped. Status and `lsof` output fit with room to spare.
const MAX_STDOUT: u64 = 1 << 20;

#[derive(Debug)]
pub(crate) enum RunError {
    /// The program could not start: not installed, or not on `PATH`.
    Spawn,
    /// It ran past its deadline and was killed.
    TimedOut,
}

pub(crate) struct Ran {
    pub status: ExitStatus,
    pub stdout: String,
}

/// Run `cmd`, killing it at `timeout`. Stdout is drained on a thread while the
/// child runs, so a full pipe can never stall it; stderr is discarded. The
/// deadline also covers stdout's EOF, which a lingering grandchild can hold.
pub(crate) fn run_bounded(mut cmd: Command, timeout: Duration) -> Result<Ran, RunError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|_| RunError::Spawn)?;
    let mut stdout = child.stdout.take().ok_or(RunError::Spawn)?;
    let (sender, read) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = (&mut stdout).take(MAX_STDOUT).read_to_end(&mut out);
        // Keep draining, or a child writing past the cap dies on a broken pipe.
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        let _ = sender.send(String::from_utf8_lossy(&out).into_owned());
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            // Overran, or cannot be waited on: either way it is stopped here.
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::TimedOut);
            }
        }
    };
    let stdout = read
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| RunError::TimedOut)?;
    Ok(Ran { status, stdout })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_hung_child_is_killed_at_its_bound() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let started = Instant::now();
        let result = run_bounded(cmd, Duration::from_millis(200));
        assert!(matches!(result, Err(RunError::TimedOut)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_grandchild_holding_stdout_cannot_outlast_the_bound() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30 & echo hello"]);
        let started = Instant::now();
        let result = run_bounded(cmd, Duration::from_millis(300));
        assert!(matches!(result, Err(RunError::TimedOut)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn output_past_the_cap_is_drained_not_cut_off() {
        let mut cmd = Command::new("head");
        cmd.args(["-c", "3000000", "/dev/zero"]);
        let ran = run_bounded(cmd, Duration::from_secs(10)).expect("sh runs");
        assert_eq!(ran.status.code(), Some(0));
        assert_eq!(ran.stdout.len() as u64, MAX_STDOUT);
    }

    #[test]
    fn output_and_exit_status_come_back() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo hello; exit 3"]);
        let ran = run_bounded(cmd, Duration::from_secs(5)).expect("sh runs");
        assert_eq!(ran.stdout.trim(), "hello");
        assert_eq!(ran.status.code(), Some(3));
    }

    #[test]
    fn a_missing_program_is_a_spawn_error() {
        let cmd = Command::new("/nonexistent/lucidos-no-such-binary");
        assert!(matches!(
            run_bounded(cmd, Duration::from_secs(1)),
            Err(RunError::Spawn)
        ));
    }
}
