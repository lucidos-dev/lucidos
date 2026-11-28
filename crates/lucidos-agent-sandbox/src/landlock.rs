use std::io;
use std::path::{Path, PathBuf};

use crate::{beneath, Policy};

pub const EXECUTE: u64 = 1 << 0;
pub const WRITE_FILE: u64 = 1 << 1;
pub const READ_FILE: u64 = 1 << 2;
pub const READ_DIR: u64 = 1 << 3;
pub const ABI1_RIGHTS: u64 = (1 << 13) - 1;
pub const REFER: u64 = 1 << 13;
pub const TRUNCATE: u64 = 1 << 14;
pub const IOCTL_DEV: u64 = 1 << 15;
pub const FILE_RIGHTS: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV;

pub fn handled_rights(abi: u32) -> u64 {
    let mut rights = ABI1_RIGHTS;
    if abi >= 2 {
        rights |= REFER;
    }
    if abi >= 3 {
        rights |= TRUNCATE;
    }
    if abi >= 5 {
        rights |= IOCTL_DEV;
    }
    rights
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    Full,
    Read,
    ListOnly,
}

impl Grant {
    pub fn rights(self, handled: u64) -> u64 {
        match self {
            Grant::Full => handled,
            Grant::Read => (EXECUTE | READ_FILE | READ_DIR) & handled,
            Grant::ListOnly => READ_DIR & handled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Open,
    ReadOnly,
    Hidden,
}

pub fn grants(policy: &Policy) -> io::Result<Vec<(PathBuf, Grant)>> {
    grants_from(Path::new("/"), policy)
}

pub fn grants_from(top: &Path, policy: &Policy) -> io::Result<Vec<(PathBuf, Grant)>> {
    let walk = Walk::new(policy);
    let mut out = Vec::new();
    if let Some(grant) = walk.ancestor_grant(top) {
        out.push((top.to_path_buf(), grant));
    }
    walk.descend(top, &mut out)?;
    Ok(out)
}

struct Walk<'a> {
    private: &'a [PathBuf],
    write_scope: Option<&'a [PathBuf]>,
    rooted: Vec<(&'a Path, Class)>,
}

impl<'a> Walk<'a> {
    fn new(policy: &'a Policy) -> Self {
        let mut rooted = Vec::new();
        for (paths, class) in [
            (&policy.hidden, Class::Hidden),
            (&policy.open_within_hidden, Class::Open),
            (&policy.read_only, Class::ReadOnly),
            (&policy.writable_within_read_only, Class::Open),
        ] {
            rooted.extend(paths.iter().map(|p| (p.as_path(), class)));
        }
        rooted.extend(
            policy
                .private
                .iter()
                .map(|path| (path.as_path(), Class::Hidden)),
        );
        Walk {
            rooted,
            private: &policy.private,
            write_scope: policy.write_scope.as_deref(),
        }
    }

    fn class(&self, path: &Path) -> Class {
        if self.private.iter().any(|root| beneath(path, root)) {
            return Class::Hidden;
        }
        let base = self
            .rooted
            .iter()
            .filter(|(root, _)| beneath(path, root))
            .max_by_key(|(root, _)| root.components().count())
            .map_or(Class::Open, |(_, class)| *class);
        if base == Class::Open
            && self
                .write_scope
                .is_some_and(|roots| !roots.iter().any(|root| beneath(path, root)))
        {
            Class::ReadOnly
        } else {
            base
        }
    }

    fn has_root_beneath(&self, path: &Path) -> bool {
        self.rooted
            .iter()
            .any(|(root, _)| *root != path && beneath(root, path))
            || self
                .write_scope
                .is_some_and(|roots| roots.iter().any(|root| root != path && beneath(root, path)))
    }

    fn ancestor_grant(&self, dir: &Path) -> Option<Grant> {
        let below = |class: Class| {
            self.rooted
                .iter()
                .any(|(root, _)| *root != dir && beneath(root, dir) && self.class(root) == class)
        };
        match self.class(dir) {
            Class::Hidden => None,
            _ if below(Class::Hidden) => Some(Grant::ListOnly),
            Class::ReadOnly => Some(Grant::Read),
            _ if below(Class::ReadOnly) => Some(Grant::Read),
            Class::Open => Some(Grant::Full),
        }
    }

    fn descend(&self, dir: &Path, out: &mut Vec<(PathBuf, Grant)>) -> io::Result<()> {
        let entries = std::fs::read_dir(dir).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("cannot list {} for the sandbox: {e}", dir.display()),
            )
        })?;
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_symlink() {
                continue;
            }
            let child = entry.path();
            if self.has_root_beneath(&child) {
                if let Some(grant) = self.ancestor_grant(&child) {
                    out.push((child.clone(), grant));
                }
                self.descend(&child, out)?;
                continue;
            }
            let grant = match self.class(&child) {
                Class::Open => Grant::Full,
                Class::ReadOnly => Grant::Read,
                Class::Hidden => continue,
            };
            out.push((child, grant));
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub use sys::{guards_truncation, probe, Ruleset};

#[cfg(target_os = "linux")]
mod sys {
    use std::ffi::CString;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    use super::{grants, handled_rights, FILE_RIGHTS};
    use crate::{Availability, Policy};

    const CREATE_RULESET_VERSION: u32 = 1 << 0;
    const RULE_PATH_BENEATH: libc::c_int = 1;
    const SYS_CREATE_RULESET: libc::c_long = 444;
    const SYS_ADD_RULE: libc::c_long = 445;
    const SYS_RESTRICT_SELF: libc::c_long = 446;

    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
    }

    #[repr(C, packed)]
    struct PathBeneathAttr {
        allowed_access: u64,
        parent_fd: i32,
    }

    fn abi() -> io::Result<u32> {
        // SAFETY: the version query takes no attribute pointer.
        let v = unsafe {
            libc::syscall(
                SYS_CREATE_RULESET,
                std::ptr::null::<RulesetAttr>(),
                0usize,
                CREATE_RULESET_VERSION,
            )
        };
        if v < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(v as u32)
    }

    pub fn probe() -> Availability {
        match abi() {
            Ok(v) if v >= 1 => Availability::Available,
            Ok(v) => Availability::Unavailable(format!("Landlock reports ABI {v}")),
            Err(e) => Availability::Unavailable(match e.raw_os_error() {
                Some(libc::ENOSYS) => "this kernel has no Landlock (Linux 5.13 or later)".into(),
                Some(libc::EOPNOTSUPP) => {
                    "Landlock is built but disabled: it is missing from the lsm= boot list".into()
                }
                _ => format!("the Landlock probe failed: {e}"),
            }),
        }
    }

    pub fn guards_truncation() -> bool {
        abi().is_ok_and(|v| handled_rights(v) & super::TRUNCATE != 0)
    }

    pub struct Ruleset {
        fd: OwnedFd,
    }

    impl Ruleset {
        pub fn for_policy(policy: &Policy) -> io::Result<Self> {
            let handled = handled_rights(abi()?);
            let attr = RulesetAttr {
                handled_access_fs: handled,
            };
            // SAFETY: `attr` outlives the call and its size is passed.
            let raw = unsafe {
                libc::syscall(
                    SYS_CREATE_RULESET,
                    &attr as *const RulesetAttr,
                    std::mem::size_of::<RulesetAttr>(),
                    0u32,
                )
            };
            if raw < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: the kernel just returned this descriptor to us.
            let ruleset = Ruleset {
                fd: unsafe { OwnedFd::from_raw_fd(raw as i32) },
            };
            for (path, grant) in grants(policy)? {
                ruleset.add(&path, grant.rights(handled))?;
            }
            Ok(ruleset)
        }

        fn add(&self, path: &std::path::Path, rights: u64) -> io::Result<()> {
            let c_path = CString::new(path.as_os_str().as_bytes())?;
            // SAFETY: a NUL-terminated path; the result is checked.
            let raw = unsafe {
                libc::open(
                    c_path.as_ptr(),
                    libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if raw < 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::ENOENT) {
                    return Ok(());
                }
                return Err(err);
            }
            // SAFETY: `open` just returned this descriptor to us.
            let target = unsafe { OwnedFd::from_raw_fd(raw) };
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: `stat` is written by fstat before it is read.
            if unsafe { libc::fstat(target.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: fstat succeeded, so `stat` is initialized.
            let is_dir = unsafe { stat.assume_init() }.st_mode & libc::S_IFMT == libc::S_IFDIR;
            let allowed = if is_dir { rights } else { rights & FILE_RIGHTS };
            if allowed == 0 {
                return Ok(());
            }
            let attr = PathBeneathAttr {
                allowed_access: allowed,
                parent_fd: target.as_raw_fd(),
            };
            // SAFETY: `attr` outlives the call; both descriptors are open.
            let rc = unsafe {
                libc::syscall(
                    SYS_ADD_RULE,
                    self.fd.as_raw_fd(),
                    RULE_PATH_BENEATH,
                    &attr as *const PathBeneathAttr,
                    0u32,
                )
            };
            if rc < 0 {
                return Err(io::Error::other(format!(
                    "Landlock refused a rule for {}: {}",
                    path.display(),
                    io::Error::last_os_error()
                )));
            }
            Ok(())
        }

        pub fn restrict_self(&self) -> io::Result<()> {
            // SAFETY: prctl with integer arguments only.
            if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: an open ruleset descriptor and no flags.
            if unsafe { libc::syscall(SYS_RESTRICT_SELF, self.fd.as_raw_fd(), 0u32) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "landlock_tests.rs"]
mod tests;
