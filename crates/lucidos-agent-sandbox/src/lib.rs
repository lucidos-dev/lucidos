//! The OS sandbox around an agent subprocess (ADR 0645).
//!
//! A [`Policy`] names what the child may not touch. [`command`] turns a program
//! into a `std::process::Command` that runs it under the policy:
//!
//! - **macOS**: a seatbelt profile through `/usr/bin/sandbox-exec`
//!   ([`seatbelt::profile`]).
//! - **Linux**: a Landlock ruleset the child applies to itself between fork
//!   and exec ([`landlock`]).
//!
//! [`probe`] says whether this machine can apply either one. The engine gates
//! every covered spawn on it.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

pub mod landlock;
pub mod seatbelt;

/// What a sandboxed child may not touch. Every path is absolute.
///
/// The rest of the filesystem stays open: reads, writes and execution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    /// No read, write or unix-socket connect anywhere beneath.
    pub hidden: Vec<PathBuf>,
    /// Folders inside a hidden root that stay fully open, such as a gateway
    /// data dir's `workspaces/`.
    pub open_within_hidden: Vec<PathBuf>,
    /// Readable, never writable.
    pub read_only: Vec<PathBuf>,
    /// Folders inside a read-only root that stay writable, such as a
    /// workspace's `.lucidos/worktrees/`.
    pub writable_within_read_only: Vec<PathBuf>,
}

impl Policy {
    /// The policy with each path also spelled through its resolved form, when
    /// that differs. A seatbelt matches resolved paths, and Landlock rules
    /// bind to the inode the resolved path names.
    pub fn resolved(&self) -> Policy {
        let both = |paths: &[PathBuf]| -> Vec<PathBuf> {
            let mut out = Vec::new();
            for path in paths {
                push_unique(&mut out, path.clone());
                if let Ok(real) = std::fs::canonicalize(path) {
                    push_unique(&mut out, real);
                }
            }
            out
        };
        Policy {
            hidden: both(&self.hidden),
            open_within_hidden: both(&self.open_within_hidden),
            read_only: both(&self.read_only),
            writable_within_read_only: both(&self.writable_within_read_only),
        }
    }

    /// Whether a child cannot read `path`: the file it names lies in a hidden
    /// root and in no folder reopened inside one. A symlink is judged by its
    /// target, which is what an open reads.
    pub fn hides(&self, path: &Path) -> bool {
        let policy = self.resolved();
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let within = |roots: &[PathBuf]| roots.iter().any(|root| target.starts_with(root));
        within(&policy.hidden) && !within(&policy.open_within_hidden)
    }
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

/// Whether this machine can sandbox a child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    /// Why not, in words the owner can act on.
    Unavailable(String),
}

/// Probe this machine. Runs a real trial, so it costs a spawn on macOS: cache
/// the answer.
pub fn probe() -> Availability {
    probe_platform()
}

#[cfg(target_os = "macos")]
fn probe_platform() -> Availability {
    seatbelt::probe()
}

#[cfg(target_os = "linux")]
fn probe_platform() -> Availability {
    landlock::probe()
}

/// Whether the sandbox stops a child from emptying a file it may not write.
/// Landlock gained that right only at ABI 3 (Linux 6.2).
pub fn guards_truncation() -> bool {
    #[cfg(target_os = "linux")]
    {
        landlock::guards_truncation()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn probe_platform() -> Availability {
    Availability::Unavailable("this operating system has no supported sandbox".to_string())
}

/// A command that runs `program` under `policy`. Add arguments, environment
/// and stdio to it as to any other command.
pub fn command(program: impl AsRef<OsStr>, policy: &Policy) -> io::Result<std::process::Command> {
    command_platform(program.as_ref(), &policy.resolved())
}

#[cfg(target_os = "macos")]
fn command_platform(program: &OsStr, policy: &Policy) -> io::Result<std::process::Command> {
    let mut cmd = std::process::Command::new(seatbelt::SANDBOX_EXEC);
    cmd.arg("-p").arg(seatbelt::profile(policy)).arg(program);
    Ok(cmd)
}

#[cfg(target_os = "linux")]
fn command_platform(program: &OsStr, policy: &Policy) -> io::Result<std::process::Command> {
    use std::os::unix::process::CommandExt as _;
    let ruleset = landlock::Ruleset::for_policy(policy)?;
    let mut cmd = std::process::Command::new(program);
    // SAFETY: the hook makes two syscalls on a descriptor the parent opened.
    // It allocates nothing and takes no lock, so it is safe between fork and
    // exec in a multithreaded parent.
    unsafe {
        cmd.pre_exec(move || ruleset.restrict_self());
    }
    Ok(cmd)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn command_platform(_program: &OsStr, _policy: &Policy) -> io::Result<std::process::Command> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "this operating system has no supported sandbox",
    ))
}

/// Whether `path` is `root` or lies beneath it, compared by components.
fn beneath(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_hidden_only_outside_every_reopened_folder() {
        let policy = Policy {
            hidden: vec!["/h/gw".into()],
            open_within_hidden: vec!["/h/gw/workspaces".into()],
            ..Policy::default()
        };
        assert!(policy.hides(Path::new("/h/gw/tls/key.pem")));
        assert!(!policy.hides(Path::new("/h/gw/workspaces/ws/cert.pem")));
        assert!(!policy.hides(Path::new("/h/certs/key.pem")));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_a_hidden_folder_is_judged_by_its_target() {
        let scratch = tempfile::tempdir().unwrap();
        let top = std::fs::canonicalize(scratch.path()).unwrap();
        for dir in ["hidden", "open"] {
            std::fs::create_dir(top.join(dir)).unwrap();
        }
        std::fs::write(top.join("open/key.pem"), "k").unwrap();
        std::fs::write(top.join("hidden/own.pem"), "k").unwrap();
        std::os::unix::fs::symlink(top.join("open/key.pem"), top.join("hidden/key.pem")).unwrap();
        std::os::unix::fs::symlink(top.join("hidden/own.pem"), top.join("open/own.pem")).unwrap();
        let policy = Policy {
            hidden: vec![top.join("hidden")],
            ..Policy::default()
        };
        assert!(!policy.hides(&top.join("hidden/key.pem")));
        assert!(policy.hides(&top.join("open/own.pem")));
    }
}
