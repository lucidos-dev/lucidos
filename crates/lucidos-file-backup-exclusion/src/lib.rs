use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exclusion {
    Set,
    AlreadySet,
    Unsupported,
}

impl Exclusion {
    pub fn changed(self) -> bool {
        matches!(self, Exclusion::Set)
    }
}

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

    const ATTRIBUTE: &[u8] = b"com.apple.metadata:com_apple_backup_excludeItem\0";

    const BACKUPD: &[u8] = b"com.apple.backupd";

    #[rustfmt::skip]
    const EXCLUDE_VALUE: [u8; 61] = [
        0x62, 0x70, 0x6c, 0x69, 0x73, 0x74, 0x30, 0x30,
        0x5f, 0x10, 0x11,
        0x63, 0x6f, 0x6d, 0x2e, 0x61, 0x70, 0x70, 0x6c, 0x65, 0x2e,
        0x62, 0x61, 0x63, 0x6b, 0x75, 0x70, 0x64,
        0x08,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1c,
    ];

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

        #[test]
        fn the_value_is_the_binary_plist_time_machine_writes() {
            assert_eq!(&EXCLUDE_VALUE[..8], b"bplist00");
            assert_eq!(&EXCLUDE_VALUE[8..11], &[0x5f, 0x10, 0x11]);
            assert_eq!(&EXCLUDE_VALUE[11..28], BACKUPD);
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

    #[test]
    #[cfg(target_os = "macos")]
    fn a_missing_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("was-never-created");
        assert!(ensure_excluded(&missing).is_err());
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn off_macos_it_does_nothing_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = ensure_excluded(dir.path()).expect("never fails");
        assert_eq!(outcome, Exclusion::Unsupported);
        assert!(!outcome.changed());
    }
}
