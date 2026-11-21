//! Bounded cleanup subprocesses. File-backed I/O avoids pipe deadlocks, including
//! inherited pipes held open by descendants after the immediate child exits.
use std::fs::File;
use std::io::{Read, Seek};
use std::os::unix::process::CommandExt;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

// Bulk transfers can spend hours in COPY on large workspaces. Keep their
// safety bound separate from short catalog and filesystem operations.
const BULK_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

type BoxError = Box<dyn std::error::Error + Send + Sync>;

fn run(
    command: &mut Command,
    destination: Option<File>,
    timeout: Duration,
) -> Result<Output, BoxError> {
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let capture = destination.is_none();
    command
        .stdout(destination.unwrap_or(stdout.try_clone()?))
        .stderr(stderr.try_clone()?)
        .process_group(0);
    let mut child = command.spawn()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => {
                // Only this newly spawned process group is signalled. Reap the
                // direct child even if group signalling failed.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(error) => error.into(),
                    _ => "cleanup subprocess exceeded its deadline".into(),
                });
            }
        }
    };
    // Cleanup tools must not leave writers behind that could alter an archive
    // after it has been fsynced and accepted as a recovery copy.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let mut output = Output {
        status,
        stdout: vec![],
        stderr: vec![],
    };
    if capture {
        stdout.rewind()?;
        stdout.read_to_end(&mut output.stdout)?;
    }
    stderr.rewind()?;
    stderr.read_to_end(&mut output.stderr)?;
    Ok(output)
}

pub(crate) trait CleanupCommand {
    fn cleanup_output(&mut self) -> Result<Output, BoxError>;
    fn cleanup_output_to(&mut self, file: File) -> Result<Output, BoxError>;
    fn cleanup_bulk_output(&mut self) -> Result<Output, BoxError>;
}

impl CleanupCommand for Command {
    fn cleanup_bulk_output(&mut self) -> Result<Output, BoxError> {
        run(self, None, BULK_TIMEOUT)
    }

    fn cleanup_output(&mut self) -> Result<Output, BoxError> {
        run(self, None, Duration::from_secs(120))
    }

    fn cleanup_output_to(&mut self, file: File) -> Result<Output, BoxError> {
        run(self, Some(file), BULK_TIMEOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_parent_cannot_leave_an_archive_writer_running() {
        let root = tempfile::tempdir().unwrap();
        let late = root.path().join("late");
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "(sleep 0.2; printf corrupt > \"$1\") &",
            "cleanup-test",
        ]);
        command.arg(&late);
        let output = run(&mut command, None, Duration::from_secs(5)).unwrap();
        assert!(output.status.success());
        std::thread::sleep(Duration::from_millis(400));
        assert!(!late.exists());
    }

    #[test]
    fn captures_large_output_and_failure_without_pipe_deadlock() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "dd if=/dev/zero bs=1024 count=256 2>/dev/null; printf failure >&2; false",
        ]);
        let output = run(&mut command, None, Duration::from_secs(5)).unwrap();
        assert!(!output.status.success());
        assert_eq!(output.stdout.len(), 256 * 1024);
        assert_eq!(output.stderr, b"failure");
    }

    #[test]
    fn timeout_returns_error_and_reaps_its_child() {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 60"]);
        let start = Instant::now();
        let error = run(&mut command, None, Duration::from_millis(50)).unwrap_err();
        assert!(error.to_string().contains("deadline"));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn dump_output_stays_in_file_and_stdin_can_be_a_file() {
        use std::io::Write;
        let mut input = tempfile::tempfile().unwrap();
        input.write_all(b"recovery data").unwrap();
        input.rewind().unwrap();
        let mut destination = tempfile::tempfile().unwrap();
        let mut command = Command::new("cat");
        command.stdin(input);
        let output = run(
            &mut command,
            Some(destination.try_clone().unwrap()),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        destination.rewind().unwrap();
        let mut bytes = Vec::new();
        destination.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"recovery data");
    }
}
