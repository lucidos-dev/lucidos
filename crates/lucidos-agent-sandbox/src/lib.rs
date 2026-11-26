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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    pub hidden: Vec<PathBuf>,
    pub private: Vec<PathBuf>,
    pub open_within_hidden: Vec<PathBuf>,
    pub connectable_sockets: Vec<PathBuf>,
    pub read_only: Vec<PathBuf>,
    pub writable_within_read_only: Vec<PathBuf>,
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

    pub fn hides(&self, path: &Path) -> bool {
        self.resolved().hides_resolved(path)
    }

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    Unavailable(String),
}

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
