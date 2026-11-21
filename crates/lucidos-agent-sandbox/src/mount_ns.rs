//! Linux read-only mounts around a trigger's write scope.
//!
//! Landlock does not gate metadata: chmod, chown, utimes and xattrs. So a
//! child with a write scope also enters its own user and mount namespace.
//! There every mount turns read-only, and writable clones of the write roots
//! go back over their paths. Claude Code's sandbox does the same with
//! bubblewrap.
//!
//! Where the kernel refuses an unprivileged user namespace, [`supported`] says
//! so once per process, and the child runs under Landlock alone.

use std::path::{Path, PathBuf};

/// The roots that need a writable clone: canonical, existing, and not inside
/// another root, since a recursive clone of the outer one carries it.
pub fn outermost_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut real: Vec<PathBuf> = roots
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .collect();
    real.sort_by_key(|root| root.components().count());
    let mut out: Vec<PathBuf> = Vec::new();
    for root in real {
        if !out.iter().any(|kept| root.starts_with(kept)) {
            out.push(root);
        }
    }
    out
}

/// Whether a scope leaves anything to mount read-only. A scope holding `/`
/// already lets the child write everywhere Landlock allows.
pub fn confines(roots: &[PathBuf]) -> bool {
    !roots.iter().any(|root| root == Path::new("/"))
}

#[cfg(target_os = "linux")]
pub use sys::{supported, ReadOnlyOutside};

#[cfg(target_os = "linux")]
mod sys {
    use std::ffi::{CStr, CString};
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    const SYS_OPEN_TREE: libc::c_long = 428;
    const SYS_MOVE_MOUNT: libc::c_long = 429;
    const SYS_MOUNT_SETATTR: libc::c_long = 442;
    const OPEN_TREE_CLONE: libc::c_uint = 1;
    const AT_RECURSIVE: libc::c_uint = 0x8000;
    const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 0x4;
    const MOUNT_ATTR_RDONLY: u64 = 0x1;

    #[repr(C)]
    struct MountAttr {
        attr_set: u64,
        attr_clr: u64,
        propagation: u64,
        userns_fd: u64,
    }

    /// Everything the child needs, built in the parent so that
    /// [`ReadOnlyOutside::enter`] allocates nothing.
    pub struct ReadOnlyOutside {
        roots: Vec<CString>,
        clones: Vec<libc::c_int>,
        uid_map: Vec<u8>,
        gid_map: Vec<u8>,
    }

    impl ReadOnlyOutside {
        /// `None` when the scope leaves nothing to mount read-only.
        pub fn new(write_roots: &[PathBuf]) -> io::Result<Option<Self>> {
            let roots = super::outermost_roots(write_roots);
            if !super::confines(&roots) {
                return Ok(None);
            }
            let roots = roots
                .iter()
                .map(|root| CString::new(root.as_os_str().as_bytes()))
                .collect::<Result<Vec<_>, _>>()?;
            // SAFETY: neither call can fail.
            let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
            Ok(Some(ReadOnlyOutside {
                clones: vec![-1; roots.len()],
                roots,
                uid_map: format!("{uid} {uid} 1\n").into_bytes(),
                gid_map: format!("{gid} {gid} 1\n").into_bytes(),
            }))
        }

        /// Enter the namespace. Safe between fork and exec: syscalls on
        /// buffers built in the parent, no allocation, no lock. Once the
        /// unshare succeeds, any later failure fails the spawn. A child left
        /// without its id map could no longer write its own files.
        pub fn enter(&mut self) -> io::Result<()> {
            // SAFETY: integer flags only.
            check(unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS) })?;
            write_proc(c"/proc/self/setgroups", b"deny")?;
            write_proc(c"/proc/self/uid_map", &self.uid_map)?;
            write_proc(c"/proc/self/gid_map", &self.gid_map)?;
            // SAFETY: null source, type and data are valid for a propagation
            // change.
            check(unsafe {
                libc::mount(
                    std::ptr::null(),
                    c"/".as_ptr(),
                    std::ptr::null(),
                    libc::MS_REC | libc::MS_PRIVATE,
                    std::ptr::null(),
                )
            })?;
            // Inherited mounts are locked to their children, so only a
            // recursive clone may copy one.
            for (root, clone) in self.roots.iter().zip(self.clones.iter_mut()) {
                // SAFETY: a NUL-terminated path; the result is checked.
                let fd = unsafe {
                    libc::syscall(
                        SYS_OPEN_TREE,
                        libc::AT_FDCWD,
                        root.as_ptr(),
                        OPEN_TREE_CLONE | AT_RECURSIVE | libc::O_CLOEXEC as libc::c_uint,
                    )
                };
                check_long(fd)?;
                *clone = fd as libc::c_int;
            }
            // The clones are detached, outside `/`, so they stay writable.
            let attr = MountAttr {
                attr_set: MOUNT_ATTR_RDONLY,
                attr_clr: 0,
                propagation: 0,
                userns_fd: 0,
            };
            // SAFETY: `attr` outlives the call and its size is passed.
            check_long(unsafe {
                libc::syscall(
                    SYS_MOUNT_SETATTR,
                    libc::AT_FDCWD,
                    c"/".as_ptr(),
                    AT_RECURSIVE,
                    &attr as *const MountAttr,
                    std::mem::size_of::<MountAttr>(),
                )
            })?;
            for (root, clone) in self.roots.iter().zip(&self.clones) {
                // SAFETY: an open clone descriptor and NUL-terminated paths.
                check_long(unsafe {
                    libc::syscall(
                        SYS_MOVE_MOUNT,
                        *clone,
                        c"".as_ptr(),
                        libc::AT_FDCWD,
                        root.as_ptr(),
                        MOVE_MOUNT_F_EMPTY_PATH,
                    )
                })?;
                // SAFETY: the descriptor is ours and no longer needed.
                unsafe { libc::close(*clone) };
            }
            rejoin_cwd()
        }
    }

    /// std changes directory before it runs `pre_exec`, so the working
    /// directory still names the read-only mount a clone now covers. Walking
    /// back to the same path lands on the top mount. A removed directory has
    /// no path left, so it stays as it is: read-only.
    fn rejoin_cwd() -> io::Result<()> {
        let mut cwd = [0 as libc::c_char; libc::PATH_MAX as usize];
        // SAFETY: the buffer and its length match.
        if unsafe { libc::getcwd(cwd.as_mut_ptr(), cwd.len()) }.is_null() {
            return Ok(());
        }
        // SAFETY: getcwd wrote a NUL-terminated path.
        check(unsafe { libc::chdir(cwd.as_ptr()) })
    }

    fn write_proc(path: &CStr, contents: &[u8]) -> io::Result<()> {
        // SAFETY: a NUL-terminated path; the result is checked.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) };
        check(fd)?;
        // SAFETY: an open descriptor and a buffer of the given length.
        let written = unsafe { libc::write(fd, contents.as_ptr().cast(), contents.len()) };
        let result = if written == contents.len() as isize {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        };
        // SAFETY: the descriptor is ours.
        unsafe { libc::close(fd) };
        result
    }

    fn check(rc: libc::c_int) -> io::Result<()> {
        check_long(rc.into())
    }

    fn check_long(rc: libc::c_long) -> io::Result<()> {
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Whether a child here can enter the namespace. A real trial decides,
    /// once per process. The kernel version cannot tell, since a sysctl, an
    /// AppArmor policy or a container's seccomp profile may refuse it.
    pub fn supported() -> bool {
        static SUPPORTED: OnceLock<bool> = OnceLock::new();
        if let Some(known) = SUPPORTED.get() {
            return *known;
        }
        // A trial that could not run decides nothing. This spawn runs under
        // Landlock alone, and the next spawn tries again.
        trial().is_some_and(|answer| *SUPPORTED.get_or_init(|| answer))
    }

    /// `None` when the trial could not run, such as a fork refused under load.
    fn trial() -> Option<bool> {
        // `/dev` is in every trigger scope, and its inherited submounts test
        // the recursive clone. The trial writes nothing.
        let mut trial = match ReadOnlyOutside::new(&[PathBuf::from("/dev")]) {
            Ok(Some(trial)) => trial,
            Ok(None) => return Some(false),
            Err(_) => return None,
        };
        // SAFETY: the child makes syscalls on prebuilt buffers only and
        // leaves through `_exit`, so a multithreaded parent is fine.
        match unsafe { libc::fork() } {
            -1 => None,
            0 => {
                let code = if trial.enter().is_ok() { 0 } else { 1 };
                // SAFETY: ends the forked child without running destructors.
                unsafe { libc::_exit(code) }
            }
            pid => {
                let mut status = 0;
                loop {
                    // SAFETY: waits for the child forked above.
                    let rc = unsafe { libc::waitpid(pid, &mut status, 0) };
                    if rc == pid {
                        break;
                    }
                    if io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                        return None;
                    }
                }
                Some(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_outermost_existing_roots_get_a_clone() {
        let scratch = tempfile::tempdir().unwrap();
        let top = std::fs::canonicalize(scratch.path()).unwrap();
        std::fs::create_dir_all(top.join("ws/inner")).unwrap();
        std::fs::create_dir(top.join("grant")).unwrap();
        let roots = outermost_roots(&[
            top.join("ws/inner"),
            top.join("missing"),
            top.join("ws"),
            top.join("grant"),
            top.join("ws"),
        ]);
        assert_eq!(roots, vec![top.join("ws"), top.join("grant")]);
    }

    #[test]
    fn a_scope_holding_the_root_confines_nothing() {
        assert!(!confines(&outermost_roots(&["/".into(), "/tmp".into()])));
        assert!(confines(&outermost_roots(&["/tmp".into()])));
    }
}
