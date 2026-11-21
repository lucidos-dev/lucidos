//! Capture this process's fd 2, preserving the original host stream. The
//! writer's own errors use the saved descriptor to avoid feeding back.
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

static ORIGINAL: OnceLock<std::fs::File> = OnceLock::new();
static INSTALLED: Mutex<bool> = Mutex::new(false);
static STDOUT_INSTALLED: Mutex<bool> = Mutex::new(false);

pub fn host_error(message: &str) {
    if let Some(mut file) = ORIGINAL.get() {
        let _ = writeln!(file, "{message}");
    } else {
        let _ = writeln!(io::stderr(), "{message}");
    }
}

/// Install once, after starting the diagnostic writer. Uses POSIX descriptors
/// on both shipped platforms, macOS and Linux. Children inherit fd 2 normally.
#[cfg(unix)]
pub fn install() -> io::Result<()> {
    install_stream(2, "Stderr", &INSTALLED, Some(&ORIGINAL))
}

/// The desktop shell also captures stdout, preserving its host stream. Other
/// processes leave machine-readable stdout alone.
#[cfg(unix)]
pub fn install_stdout() -> io::Result<()> {
    install_stream(1, "Stdout", &STDOUT_INSTALLED, None)
}

#[cfg(unix)]
fn install_stream(
    target: i32,
    label: &'static str,
    state: &Mutex<bool>,
    saved: Option<&OnceLock<std::fs::File>>,
) -> io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let mut installed = state.lock().unwrap_or_else(|e| e.into_inner());
    if *installed {
        return Ok(());
    }
    // SAFETY: dup returns a new owned descriptor, checked before construction.
    let original = unsafe { libc::fcntl(target, libc::F_DUPFD_CLOEXEC, 3) };
    if original < 0 {
        return Err(io::Error::last_os_error());
    }
    let original = unsafe { std::fs::File::from_raw_fd(original) };
    let mut fds = [-1; 2];
    // SAFETY: pipe initializes exactly two descriptors on success.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let reader = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let writer = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    for fd in [&reader, &writer] {
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let host = original.try_clone()?;
    std::thread::Builder::new()
        .name(format!("diagnostic-{label}"))
        .spawn(move || {
            capture(std::fs::File::from(reader), host, |bytes, overflow| {
                submit(label, bytes, overflow);
            });
        })?;
    // SAFETY: both descriptors are open; dup2 atomically replaces the target.
    if unsafe { libc::dup2(writer.as_raw_fd(), target) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if let Some(saved) = saved {
        let _ = saved.set(original);
    }
    *installed = true;
    Ok(())
}

#[cfg(not(unix))]
pub fn install() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "stderr capture requires POSIX",
    ))
}

#[cfg(not(unix))]
pub fn install_stdout() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "stdout capture requires POSIX",
    ))
}

/// Drain a detached child's stream with the same bounded, sanitized framing.
/// The caller owns its diagnostic sink; no raw host-log file is created.
pub fn capture_reader(label: &'static str, reader: impl io::Read) {
    capture(reader, io::sink(), |bytes, overflow| {
        submit(label, bytes, overflow)
    });
}

fn capture(mut reader: impl io::Read, mut host: impl Write, mut submit: impl FnMut(&[u8], bool)) {
    const CAP: usize = 32 * 1024;
    let mut chunk = [0; 4096];
    let mut pending = Vec::new();
    let mut overflow = false;
    loop {
        let n = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        // Always forward the bytes, including oversized and non-UTF8 output.
        let _ = host.write_all(&chunk[..n]);
        for &byte in &chunk[..n] {
            if byte == b'\n' {
                submit(&pending, overflow);
                pending.clear();
                overflow = false;
            } else if pending.len() < CAP {
                pending.push(byte);
            } else {
                overflow = true;
            }
        }
    }
    if !pending.is_empty() || overflow {
        submit(&pending, overflow);
    }
}

fn submit(label: &'static str, bytes: &[u8], overflow: bool) {
    let message = if overflow {
        // Do not retain a partial token from an overlong line.
        "<oversized stream line omitted>".into()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    crate::record_legacy(label, message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_gets_exact_bytes_and_capture_keeps_bounded_complete_lines() {
        let mut input = b"normal\nnon-utf8 \xff\n".to_vec();
        input.extend(vec![b'x'; 100_000]);
        input.extend_from_slice(b"\ntail");
        let mut host = Vec::new();
        let mut lines = Vec::new();
        capture(input.as_slice(), &mut host, |bytes, overflow| {
            assert!(bytes.len() <= 32 * 1024);
            lines.push((bytes.to_vec(), overflow));
        });
        assert_eq!(host, input);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], (b"normal".to_vec(), false));
        assert!(lines[2].1);
        assert_eq!(lines[3], (b"tail".to_vec(), false));
    }
}
