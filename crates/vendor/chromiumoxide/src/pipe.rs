//! CDP over a pipe: Chrome's `--remote-debugging-pipe`.
//!
//! Chrome reads commands from fd 3 and writes responses and events to fd 4.
//! Each message is JSON ended by a NUL byte. Unlike `--remote-debugging-port`,
//! no socket exists for another process to reach. Only the launcher holds the
//! other ends of the two pipes.
//!
//! Each direction runs on its own thread with blocking I/O, so the transport
//! needs neither async runtime's reactor. Dropping the [`PipeTransport`] ends
//! the writer thread, which closes Chrome's fd 3, and Chrome then exits.

use std::io::{self, BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::process::Command;

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::task::{Context, Poll};
use futures::StreamExt;

/// The fd Chrome reads commands from.
const CHROME_READ_FD: libc::c_int = 3;
/// The fd Chrome writes responses and events to.
const CHROME_WRITE_FD: libc::c_int = 4;
/// Above every fd a fresh child could already hold at 3 or 4.
const STAGING_FD_FLOOR: libc::c_int = 10;

/// The launcher's side of the two pipes.
#[derive(Debug)]
pub struct PipeTransport {
    outgoing: UnboundedSender<Vec<u8>>,
    incoming: UnboundedReceiver<io::Result<String>>,
}

impl PipeTransport {
    /// Queue one message for Chrome. Fails once the writer thread is gone.
    pub(crate) fn send(&self, message: String) -> io::Result<()> {
        self.outgoing
            .unbounded_send(message.into_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the CDP pipe to Chrome is closed"))
    }

    /// The next message from Chrome, or `None` once Chrome closed its end.
    pub(crate) fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<io::Result<String>>> {
        self.incoming.poll_next_unpin(cx)
    }
}

/// Arrange for `command` to speak CDP over a pipe, and return the launcher's
/// side.
///
/// Adds `--remote-debugging-pipe` and maps the child's ends onto fd 3 and fd 4
/// between `fork` and `exec`. The launcher's ends are close-on-exec, so no
/// other child inherits them.
pub(crate) fn attach(command: &mut Command) -> io::Result<PipeTransport> {
    let (chrome_reads, launcher_writes) = io::pipe()?;
    let (launcher_reads, chrome_writes) = io::pipe()?;
    let chrome_reads = OwnedFd::from(chrome_reads);
    let chrome_writes = OwnedFd::from(chrome_writes);

    command.arg("--remote-debugging-pipe");
    // SAFETY: the closure runs in the forked child before `exec`, and calls
    // only `fcntl` and `dup2`, which are async-signal-safe. It owns both fds,
    // so they stay open until the `Command` is dropped after spawning.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(command, move || {
            map_child_fds(&chrome_reads, &chrome_writes)
        });
    }

    let (outgoing, to_chrome) = unbounded();
    let (from_chrome, incoming) = unbounded();
    spawn_writer(launcher_writes, to_chrome)?;
    spawn_reader(launcher_reads, from_chrome)?;
    Ok(PipeTransport { outgoing, incoming })
}

/// Put the child's pipe ends on fd 3 and fd 4, without close-on-exec.
///
/// Both ends go to close-on-exec fds of at least 10 first. Otherwise an end
/// that already sits on 3 or 4 is either clobbered by the first `dup2`, or kept
/// with its close-on-exec flag, since `dup2` onto itself changes nothing. The
/// staged copies close at `exec`, so Chrome holds fd 3 and fd 4 alone.
fn map_child_fds(chrome_reads: &OwnedFd, chrome_writes: &OwnedFd) -> io::Result<()> {
    let staged_read = stage(chrome_reads)?;
    let staged_write = stage(chrome_writes)?;
    for (from, to) in [(staged_read, CHROME_READ_FD), (staged_write, CHROME_WRITE_FD)] {
        // SAFETY: `from` is an fd this child owns, and `dup2` clears
        // close-on-exec on `to`.
        if unsafe { libc::dup2(from, to) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn stage(fd: &OwnedFd) -> io::Result<libc::c_int> {
    // SAFETY: `F_DUPFD_CLOEXEC` on an open fd returns a new fd or -1.
    match unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, STAGING_FD_FLOOR) } {
        -1 => Err(io::Error::last_os_error()),
        staged => Ok(staged),
    }
}

/// Write each queued message and its NUL. Ends when the sender is dropped, and
/// closing the pipe then tells Chrome to exit.
fn spawn_writer(mut pipe: PipeWriter, mut messages: UnboundedReceiver<Vec<u8>>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("cdp-pipe-writer".into())
        .spawn(move || {
            while let Some(message) = futures::executor::block_on(messages.next()) {
                if let Err(err) = pipe.write_all(&message).and_then(|()| pipe.write_all(b"\0")) {
                    tracing::debug!("CDP pipe write failed: {err}");
                    return;
                }
            }
        })?;
    Ok(())
}

/// Read NUL-ended messages until Chrome closes its end.
fn spawn_reader(pipe: PipeReader, messages: UnboundedSender<io::Result<String>>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("cdp-pipe-reader".into())
        .spawn(move || {
            let mut reader = BufReader::new(pipe);
            loop {
                let mut frame = Vec::new();
                let message = match reader.read_until(b'\0', &mut frame) {
                    Ok(0) => return,
                    Ok(_) => {
                        if frame.last() == Some(&b'\0') {
                            frame.pop();
                        }
                        String::from_utf8(frame).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
                    }
                    Err(err) => Err(err),
                };
                let failed = message.is_err();
                if messages.unbounded_send(message).is_err() || failed {
                    return;
                }
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Stands in for Chrome: echoes each NUL-ended message from fd 3 to fd 4.
    /// Proves the fd mapping and the framing together.
    #[test]
    fn a_child_reads_fd_3_and_writes_fd_4() {
        let mut command = Command::new("sh");
        command.args(["-c", "cat <&3 >&4", "--"]);
        let mut transport = attach(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        drop(command);

        transport.send(r#"{"id":1}"#.into()).unwrap();
        transport.send(r#"{"id":2}"#.into()).unwrap();
        let next = |transport: &mut PipeTransport| {
            futures::executor::block_on(futures::future::poll_fn(|cx| transport.poll_next(cx)))
        };
        assert_eq!(next(&mut transport).unwrap().unwrap(), r#"{"id":1}"#);
        assert_eq!(next(&mut transport).unwrap().unwrap(), r#"{"id":2}"#);

        // Dropping the transport closes the child's fd 3, so `cat` ends and
        // the child closes fd 4.
        drop(transport);
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn the_flag_is_the_pipe_and_never_a_port() {
        let mut command = Command::new("true");
        let _transport = attach(&mut command).unwrap();
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["--remote-debugging-pipe"]);
    }

    /// The launcher's ends must not leak into a child: one that inherited the
    /// write end would hold Chrome's fd 3 open after the launcher let go.
    #[test]
    fn the_launchers_ends_stay_out_of_other_children() {
        fn fds_a_child_sees() -> Vec<String> {
            let mut probe = Command::new("sh");
            probe.args(["-c", "ls /dev/fd"]).stdout(std::process::Stdio::piped());
            let mut out = String::new();
            let mut child = probe.spawn().unwrap();
            child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
            child.wait().unwrap();
            out.split_whitespace().map(str::to_string).collect()
        }
        let before = fds_a_child_sees();
        let mut command = Command::new("true");
        let _transport = attach(&mut command).unwrap();
        assert_eq!(fds_a_child_sees(), before);
    }
}
