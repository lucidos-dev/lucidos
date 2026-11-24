//! The OS sandbox around an agent subprocess (ADR 0645).
//!
//! A [`Policy`] names what the child may not touch. [`command`] turns a program
//! into a `std::process::Command` that runs it under the policy:
//!
//! - **macOS**: a seatbelt profile through `/usr/bin/sandbox-exec`
//!   ([`seatbelt::profile`]).
//! - **Linux**: a Landlock ruleset the child applies to itself between fork
//!   and exec ([`landlock`]). A write scope adds read-only mounts outside it
//!   where the kernel allows ([`mount_ns`]).
//!
//! [`probe`] says whether this machine can apply either one. The engine gates
//! every covered spawn on it.
//!
//! When a confined command fails, [`seatbelt_log`] finds the macOS denial and
//! [`write_refusal`] reads the refused path from its output.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

pub mod landlock;
pub mod mount_ns;
pub mod ps_shim;
pub mod seatbelt;
pub mod seatbelt_log;
pub mod temp_dirs;
pub mod write_refusal;

/// What a sandboxed child may not touch. Every path is absolute.
///
/// The rest of the filesystem stays open: reads, writes and execution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    /// No read, write or unix-socket connect anywhere beneath.
    pub hidden: Vec<PathBuf>,
    /// Private roots take precedence over filesystem exceptions.
    pub private: Vec<PathBuf>,
    /// Folders inside a hidden root that stay fully open, such as a gateway
    /// data dir's `workspaces/`.
    pub open_within_hidden: Vec<PathBuf>,
    /// Exact Unix sockets that remain connectable inside hidden roots.
    /// This grants no filesystem access to the socket or its folder.
    pub connectable_sockets: Vec<PathBuf>,
    /// Readable, never writable.
    pub read_only: Vec<PathBuf>,
    /// Folders inside a read-only root that stay writable, such as a
    /// workspace's `.lucidos/worktrees/`.
    pub writable_within_read_only: Vec<PathBuf>,
    /// Optional additional allowlist that narrows writes without reopening protected paths.
    pub write_scope: Option<Vec<PathBuf>>,
}

impl Policy {
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
            private: both(&self.private),
            connectable_sockets: both(&self.connectable_sockets),
            open_within_hidden: both(&self.open_within_hidden),
            read_only: both(&self.read_only),
            writable_within_read_only: both(&self.writable_within_read_only),
            write_scope: self.write_scope.clone(),
        }
    }

    /// Whether a child cannot read `path`: the file it names lies in a hidden
    /// root and in no folder reopened inside one. A symlink is judged by its
    /// target, which is what an open reads.
    pub fn hides(&self, path: &Path) -> bool {
        self.resolved().hides_resolved(path)
    }

    /// Check an already resolved policy against both spellings of a candidate.
    pub fn hides_resolved(&self, path: &Path) -> bool {
        let target = path
            .ancestors()
            .find_map(|ancestor| {
                let suffix = path.strip_prefix(ancestor).ok()?;
                Some(ancestor.canonicalize().ok()?.join(suffix))
            })
            .unwrap_or_else(|| path.to_path_buf());
        let within = |roots: &[PathBuf]| roots.iter().any(|root| target.starts_with(root));
        self.private
            .iter()
            .any(|root| target.starts_with(root) || path.starts_with(root))
            || (within(&self.hidden) && !within(&self.open_within_hidden))
    }

    /// The policy over the machine's gateway data dirs and workspaces:
    ///
    /// - **Hidden:** every gateway data dir.
    /// - **Open inside a data dir:** its `workspaces/`, where workspaces live.
    /// - **Read-only:** each workspace's `.lucidos/`, which holds its grant
    ///   files, except the `writable` subpaths.
    pub fn for_topology(
        data_dirs: &[PathBuf],
        workspaces: &[PathBuf],
        writable: &[&str],
    ) -> Policy {
        let mut policy = Policy::default();
        let mut workspaces = workspaces.to_vec();
        for data in data_dirs {
            push_unique(&mut policy.hidden, data.clone());
            let open = data.join("workspaces");
            if let Ok(entries) = std::fs::read_dir(&open) {
                workspaces.extend(entries.flatten().map(|entry| entry.path()));
            }
            push_unique(&mut policy.open_within_hidden, open);
        }
        for ws in workspaces {
            push_unique(&mut policy.read_only, ws.join(".lucidos"));
            for subpath in writable {
                push_unique(&mut policy.writable_within_read_only, ws.join(subpath));
            }
        }
        policy
    }

    /// This policy with every write refused outside `roots`, the OS temp dirs
    /// and `/dev`. Each open folder inside a hidden root turns read-only too,
    /// so the reopening grants reads only.
    pub fn confine_writes(&self, roots: &[PathBuf]) -> Policy {
        let mut confined = self.clone();
        push_unique(&mut confined.read_only, PathBuf::from("/"));
        for open in &self.open_within_hidden {
            push_unique(&mut confined.read_only, open.clone());
        }
        confined.writable_within_read_only.clear();
        for root in roots
            .iter()
            .cloned()
            .chain(temp_dirs::roots().iter().cloned())
            .chain([PathBuf::from("/dev")])
        {
            push_unique(&mut confined.writable_within_read_only, root);
        }
        confined
    }
}

pub fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
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

/// Whether a write scope also stops a child changing the metadata of files
/// outside it: permissions, owners, times and xattrs. Linux needs an
/// unprivileged user namespace for that ([`mount_ns`]). Probes on first call.
pub fn guards_metadata() -> bool {
    #[cfg(target_os = "linux")]
    {
        mount_ns::supported()
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
    let mut mounts = match &policy.write_scope {
        Some(roots) if mount_ns::supported() => mount_ns::ReadOnlyOutside::new(roots)?,
        _ => None,
    };
    let mut cmd = std::process::Command::new(program);
    // SAFETY: the hook makes syscalls on descriptors and buffers the parent
    // built. It allocates nothing and takes no lock, so it is safe between
    // fork and exec in a multithreaded parent.
    unsafe {
        cmd.pre_exec(move || {
            if let Some(mounts) = &mut mounts {
                mounts.enter()?;
            }
            ruleset.restrict_self()
        });
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

    #[cfg(unix)]
    #[test]
    fn resolved_keeps_both_the_given_and_the_real_spelling() {
        let scratch = tempfile::tempdir().unwrap();
        let top = std::fs::canonicalize(scratch.path()).unwrap();
        std::fs::create_dir(top.join("real")).unwrap();
        std::os::unix::fs::symlink(top.join("real"), top.join("link")).unwrap();
        let policy = Policy {
            hidden: vec![top.join("link")],
            read_only: vec![top.join("link")],
            ..Policy::default()
        };
        let resolved = policy.resolved();
        for paths in [&resolved.hidden, &resolved.read_only] {
            assert_eq!(paths, &vec![top.join("link"), top.join("real")]);
        }
    }

    #[test]
    fn confined_writes_reopen_only_the_given_roots() {
        let policy = Policy::for_topology(
            &[PathBuf::from("/h/.lucidos/gateway")],
            &[PathBuf::from("/w")],
            &[".lucidos/worktrees"],
        );
        let confined = policy.confine_writes(&[PathBuf::from("/w/.lucidos/worktrees/t1")]);
        assert!(confined.read_only.contains(&PathBuf::from("/")));
        assert!(confined
            .read_only
            .contains(&PathBuf::from("/h/.lucidos/gateway/workspaces")));
        assert!(confined
            .writable_within_read_only
            .contains(&PathBuf::from("/w/.lucidos/worktrees/t1")));
        assert!(
            !confined
                .writable_within_read_only
                .contains(&PathBuf::from("/w/.lucidos/worktrees")),
            "a sibling worktree stays read-only"
        );
        assert_eq!(confined.hidden, policy.hidden);
    }

    #[test]
    fn confinement_uses_the_shared_scratch_roots() {
        let policy = Policy::default().confine_writes(&[]);
        for root in temp_dirs::roots() {
            assert!(root.is_absolute());
            assert!(
                policy.writable_within_read_only.contains(root),
                "{}",
                root.display()
            );
        }
        assert!(temp_dirs::roots().contains(&PathBuf::from("/tmp")));
    }
}
