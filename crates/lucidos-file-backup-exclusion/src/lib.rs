//! Opt a directory out of file-level backup. Decision: ADR 0153.
//!
//! A file-level backup is one the OS or a third-party product takes by copying
//! files off the machine. Time Machine is the shape that prompted this. It is
//! not a Lucidos backup, which is the workspace's own encrypted archive.
//!
//! # Directories, never files
//!
//! macOS records the exclusion in an extended attribute, which belongs to an
//! inode. An atomic write is write-tmp then rename. That swaps the inode and
//! drops the attribute, so a file-level exclusion evaporates on the next save.
//! A directory's attribute survives, and covers what lands inside it later.
//!
//! # Off macOS
//!
//! Nothing happens, and the call reports [`Exclusion::Unsupported`]. Linux has
//! no equivalent marker, and ADR 0153 says why encryption is its answer.

use std::io;
use std::path::Path;

/// What [`ensure_excluded`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exclusion {
    /// The call set the exclusion. The directory did not carry it before.
    Set,
    /// The directory already carried it. Nothing was written.
    AlreadySet,
    /// The platform has no file-level backup exclusion. Nothing was written.
    Unsupported,
}

impl Exclusion {
    /// Whether this call changed anything. Callers log on `true` alone, which
    /// is what keeps the every-start re-check silent.
    pub fn changed(self) -> bool {
        matches!(self, Exclusion::Set)
    }
}

/// Mark `dir` as excluded from file-level backup, and report what changed.
///
/// Idempotent: a directory that already carries the exclusion is left
/// untouched. An error means the exclusion is NOT in place, most often because
/// `dir` does not exist or the filesystem holds no extended attributes.
/// Callers treat that as best-effort and carry on, the same way
/// `ensure_workspace_gitignore_entries` does.
pub fn ensure_excluded(dir: &Path) -> io::Result<Exclusion> {
    platform::ensure_excluded(dir)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::Exclusion;
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    /// The attribute Time Machine reads. `CSBackupSetItemExcluded` writes this
    /// one, and `tmutil isexcluded` reports on it.
    const ATTRIBUTE: &[u8] = b"com.apple.metadata:com_apple_backup_excludeItem\0";

    /// The one string the value has to carry. Recognizing it by substring
    /// accepts an XML plist another tool may have written, as well as the
    /// binary one below.
    const BACKUPD: &[u8] = b"com.apple.backupd";

    /// The value `CSBackupSetItemExcluded` writes: a binary plist holding the
    /// single string `com.apple.backupd`. Written out byte for byte so the
    /// crate needs no plist encoder.
    #[rustfmt::skip]
    const EXCLUDE_VALUE: [u8; 61] = [
        // "bplist00"
        0x62, 0x70, 0x6c, 0x69, 0x73, 0x74, 0x30, 0x30,
        // one ASCII string, 17 bytes of it
        0x5f, 0x10, 0x11,
        // "com.apple.backupd"
        0x63, 0x6f, 0x6d, 0x2e, 0x61, 0x70, 0x70, 0x6c, 0x65, 0x2e,
        0x62, 0x61, 0x63, 0x6b, 0x75, 0x70, 0x64,
        // offset table: the one object starts at byte 8
        0x08,
        // trailer: 6 unused bytes, then 1-byte offsets and 1-byte refs
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01,
        // one object in the file
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        // the root is object 0
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        // the offset table starts at byte 28
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1c,
    ];

    /// Big enough for the 61-byte value plus any XML plist a tool might have
    /// left. A value that overflows it reads as "not ours" and gets replaced.
    const READ_BUFFER: usize = 512;

    pub fn ensure_excluded(dir: &Path) -> io::Result<Exclusion> {
        let path = CString::new(dir.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path holds a NUL byte"))?;

        if already_excluded(&path) {
            return Ok(Exclusion::AlreadySet);
        }

        // SAFETY: both pointers are valid for the lengths passed. `path` and
        // `ATTRIBUTE` are NUL-terminated, and the value is a borrowed const.
        let rc = unsafe {
            libc::setxattr(
                path.as_ptr(),
                ATTRIBUTE.as_ptr() as *const libc::c_char,
                EXCLUDE_VALUE.as_ptr() as *const libc::c_void,
                EXCLUDE_VALUE.len(),
                0,
                0,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Exclusion::Set)
    }

    /// Whether the attribute is already there and already names `backupd`.
    ///
    /// Any read failure answers `false`, which routes to a write. That is the
    /// safe direction: a needless `setxattr` costs one syscall, while reading
    /// a failure as "already excluded" would leave the secret in the backup.
    fn already_excluded(path: &std::ffi::CStr) -> bool {
        let mut buf = [0u8; READ_BUFFER];
        // SAFETY: `buf` is valid for `READ_BUFFER` bytes, and both C strings
        // are NUL-terminated.
        let len = unsafe {
            libc::getxattr(
                path.as_ptr(),
                ATTRIBUTE.as_ptr() as *const libc::c_char,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                0,
                0,
            )
        };
        if len <= 0 {
            return false;
        }
        buf[..len as usize]
            .windows(BACKUPD.len())
            .any(|w| w == BACKUPD)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The value has to be exactly what `CSBackupSetItemExcluded` writes,
        /// because `tmutil isexcluded` is what confirms the fix. These are the
        /// bytes read back off a directory `tmutil addexclusion` had marked.
        #[test]
        fn the_value_is_the_binary_plist_time_machine_writes() {
            assert_eq!(&EXCLUDE_VALUE[..8], b"bplist00");
            assert_eq!(&EXCLUDE_VALUE[8..11], &[0x5f, 0x10, 0x11]);
            assert_eq!(&EXCLUDE_VALUE[11..28], BACKUPD);
            // The trailer's last eight bytes point at the offset table, which
            // sits right after the object.
            assert_eq!(EXCLUDE_VALUE[60], 28);
            assert_eq!(EXCLUDE_VALUE[28], 8);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::Exclusion;
    use std::io;
    use std::path::Path;

    pub fn ensure_excluded(_dir: &Path) -> io::Result<Exclusion> {
        Ok(Exclusion::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The re-check on every start must be silent when nothing needs doing.
    /// The call sites gate their log on `changed()`. A second call reporting
    /// `Set` would therefore print a line on every boot, for ever.
    #[test]
    #[cfg(target_os = "macos")]
    fn setting_twice_reports_the_second_call_as_a_no_op() {
        let dir = tempfile::tempdir().unwrap();

        let first = ensure_excluded(dir.path()).expect("first call");
        assert_eq!(first, Exclusion::Set);
        assert!(first.changed());

        let second = ensure_excluded(dir.path()).expect("second call");
        assert_eq!(second, Exclusion::AlreadySet);
        assert!(!second.changed());
    }

    /// A directory's exclusion covers everything written inside it later,
    /// which is why the crate marks directories rather than files. Assert the
    /// attribute lands on the directory and survives a child appearing.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_attribute_stays_on_the_directory_as_it_fills_up() {
        let dir = tempfile::tempdir().unwrap();
        ensure_excluded(dir.path()).expect("mark the directory");

        std::fs::write(dir.path().join("backup.key"), "not a real key").unwrap();
        std::fs::create_dir(dir.path().join("worktrees")).unwrap();

        assert_eq!(
            ensure_excluded(dir.path()).expect("re-check"),
            Exclusion::AlreadySet
        );
    }

    /// An error must be an error. Reporting success for a path that does not
    /// exist would let a caller believe a secret is protected when it is not.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_missing_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("was-never-created");
        assert!(ensure_excluded(&missing).is_err());
    }

    /// Off macOS the call is a no-op that still succeeds, so no call site
    /// needs a `cfg` around it.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn off_macos_it_does_nothing_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = ensure_excluded(dir.path()).expect("never fails");
        assert_eq!(outcome, Exclusion::Unsupported);
        assert!(!outcome.changed());
    }
}
