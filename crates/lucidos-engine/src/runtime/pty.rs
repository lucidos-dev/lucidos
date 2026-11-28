//! The engine's one pseudo-terminal module (ADRs 0356 and 0820).
//!
//! INVARIANT: Every PTY the engine opens comes from `open`. A thread terminal
//! spawns through `spawn_shell`, the only `pre_exec` spawn in this crate,
//! because a controlling terminal needs `setsid` and `TIOCSCTTY` in the child.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

impl Size {
    pub const DEFAULT: Size = Size { cols: 80, rows: 24 };

    pub fn clamped(cols: u16, rows: u16) -> Size {
        Size {
            cols: cols.clamp(2, 1000),
            rows: rows.clamp(1, 500),
        }
    }
}

pub struct ShellSpec {
    pub program: PathBuf,
    pub cwd: PathBuf,
    pub env: Vec<(OsString, OsString)>,
    pub size: Size,
}

pub struct Shell {
    pub child: std::process::Child,
    pub master: OwnedFd,
}

/// Open a PTY pair sized `size`. Both ends are close-on-exec from the open
/// call, so a concurrent spawn never inherits either end.
pub fn open(size: Size) -> io::Result<(OwnedFd, OwnedFd)> {
    use rustix::fs::{open, Mode, OFlags};
    use rustix::pty::{grantpt, ptsname, unlockpt};

    let flags = OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC;
    let master = open(c"/dev/ptmx", flags, Mode::empty())?;
    grantpt(&master)?;
    unlockpt(&master)?;
    let name = ptsname(&master, Vec::new())?;
    let slave = open(name.as_c_str(), flags, Mode::empty())?;
    resize(&slave, size)?;
    Ok((master, slave))
}

pub fn resize(fd: &impl AsFd, size: Size) -> io::Result<()> {
    rustix::termios::tcsetwinsize(
        fd,
        rustix::termios::Winsize {
            ws_col: size.cols,
            ws_row: size.rows,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )?;
    Ok(())
}

/// The user's login shell from the account database, never a hardcoded one.
/// An unusable entry falls back as ADR 0326 does.
pub fn login_shell() -> PathBuf {
    account_shell()
        .filter(|path| is_executable(path))
        .unwrap_or_else(|| {
            PathBuf::from(if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/sh"
            })
        })
}

fn account_shell() -> Option<PathBuf> {
    // SAFETY: getpwuid returns a pointer into static storage or null. The
    // string is copied before any other passwd call can overwrite it.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() || (*entry).pw_shell.is_null() {
            return None;
        }
        let shell = std::ffi::CStr::from_ptr((*entry).pw_shell);
        use std::os::unix::ffi::OsStrExt;
        let path = PathBuf::from(OsStr::from_bytes(shell.to_bytes()));
        (!path.as_os_str().is_empty()).then_some(path)
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_absolute()
        && std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Spawn `spec.program` as a login shell on a new PTY.
///
/// The child becomes a session leader with the PTY as its controlling
/// terminal. It starts with default signal dispositions and an empty mask,
/// since an ignored SIGHUP would survive exec and defeat the hang-up. It runs
/// at the engine's nice + 5 (ADR 0341).
pub fn spawn_shell(spec: ShellSpec) -> io::Result<Shell> {
    let (master, slave) = open(spec.size)?;
    let name = spec
        .program
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sh".to_owned());
    let nice = below_engine_nice();
    let mut cmd = std::process::Command::new(&spec.program);
    cmd.arg0(format!("-{name}"))
        .env_clear()
        .envs(spec.env)
        .current_dir(&spec.cwd)
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // SAFETY: the closure runs between fork and exec and calls only
    // async-signal-safe functions on plain integers.
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut empty);
            libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
            for signal in 1..65 {
                if signal != libc::SIGKILL && signal != libc::SIGSTOP {
                    libc::signal(signal, libc::SIG_DFL);
                }
            }
            libc::setpriority(libc::PRIO_PROCESS, 0, nice);
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    Ok(Shell { child, master })
}

fn below_engine_nice() -> libc::c_int {
    const BELOW_ENGINE_NICE: libc::c_int = 5;
    const MAX_NICE: libc::c_int = 19;
    // SAFETY: getpriority reads this process's own priority.
    let engine = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
    (engine + BELOW_ENGINE_NICE).min(MAX_NICE)
}

/// Whether the shell itself holds the terminal's foreground. An unknown
/// answer counts as busy, so a suggestion is never typed into a program.
pub fn shell_in_foreground(master: &impl AsFd, shell_pid: u32) -> bool {
    rustix::termios::tcgetpgrp(master)
        .is_ok_and(|group| group.as_raw_nonzero().get() as u32 == shell_pid)
}

/// The process group in the terminal's foreground, when the kernel reports one.
pub fn foreground_group(master: &impl AsFd) -> Option<u32> {
    rustix::termios::tcgetpgrp(master)
        .ok()
        .map(|group| group.as_raw_nonzero().get() as u32)
}

/// Every live process whose session id is `session`, the engine excluded.
pub fn session_members(session: u32) -> Vec<u32> {
    let own = std::process::id();
    all_pids()
        .into_iter()
        .filter(|&pid| pid != own && pid != 0)
        // SAFETY: getsid only reads the kernel's record of `pid`.
        .filter(|&pid| unsafe { libc::getsid(pid as libc::pid_t) } == session as libc::pid_t)
        .collect()
}

/// SIGKILL every process in `session`, including ones forked while it dies.
pub fn kill_session(session: u32) {
    freeze_then_kill(
        || session_members(session),
        |pid, signal| {
            // SAFETY: the pid still belongs to the shell's own session, which
            // `setsid` made apart from the engine's. The check right before
            // the signal leaves alone a pid reused since the listing.
            unsafe {
                if libc::getsid(pid as libc::pid_t) == session as libc::pid_t {
                    libc::kill(pid as libc::pid_t, signal);
                }
            }
        },
    );
}

const MAX_FREEZE_SWEEPS: usize = 32;

/// A member that forks between one listing and its own death leaves a child
/// the listing never saw. A stopped process cannot fork, so freeze every
/// member until a sweep finds nobody new, then kill the frozen set. The kernel
/// resumes a stopped group that the shell's exit orphans, so list again after
/// each kill until a sweep finds nobody at all.
fn freeze_then_kill(
    mut members: impl FnMut() -> Vec<u32>,
    mut signal: impl FnMut(u32, libc::c_int),
) {
    let mut seen: Vec<u32> = Vec::new();
    let mut frozen: Vec<u32> = Vec::new();
    for _ in 0..MAX_FREEZE_SWEEPS {
        let fresh: Vec<u32> = members()
            .into_iter()
            .filter(|pid| !seen.contains(pid))
            .collect();
        if fresh.is_empty() {
            if frozen.is_empty() {
                return;
            }
            for pid in frozen.drain(..) {
                signal(pid, libc::SIGKILL);
            }
            continue;
        }
        for &pid in &fresh {
            signal(pid, libc::SIGSTOP);
        }
        seen.extend(&fresh);
        frozen.extend(fresh);
    }
    for pid in frozen {
        signal(pid, libc::SIGKILL);
    }
}

/// Send `signal` to a process group.
pub fn signal_group(group: u32, signal: libc::c_int) {
    if group > 1 && group != std::process::id() {
        // SAFETY: a negative pid addresses one process group, which belongs
        // to the shell's session and never contains the engine.
        unsafe { libc::kill(-(group as libc::pid_t), signal) };
    }
}

pub fn is_alive(pid: u32) -> bool {
    // SAFETY: signal 0 probes for existence and delivers nothing.
    pid > 0 && unsafe { libc::kill(pid as libc::pid_t, 0) } == 0
}

#[cfg(target_os = "linux")]
fn all_pids() -> Vec<u32> {
    std::fs::read_dir("/proc")
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn all_pids() -> Vec<u32> {
    // SAFETY: the first call sizes the buffer; the second fills it and
    // returns how many pids it wrote.
    unsafe {
        let count = libc::proc_listallpids(std::ptr::null_mut(), 0);
        if count <= 0 {
            return Vec::new();
        }
        let mut pids = vec![0 as libc::c_int; count as usize + 64];
        let bytes = (pids.len() * std::mem::size_of::<libc::c_int>()) as libc::c_int;
        let written = libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes);
        if written <= 0 {
            return Vec::new();
        }
        pids.truncate(written as usize);
        pids.into_iter()
            .filter(|&pid| pid > 0)
            .map(|pid| pid as u32)
            .collect()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn all_pids() -> Vec<u32> {
    Vec::new()
}

#[cfg(test)]
#[path = "pty_tests.rs"]
mod tests;
