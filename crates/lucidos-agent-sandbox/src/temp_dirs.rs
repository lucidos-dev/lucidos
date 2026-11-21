//! Scratch roots shared by command scope and process write confinement.
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// True when an inherited temp setting may serve as a scratch root. The home
/// directory, and any folder that holds it, never may: a misset `TMPDIR`
/// would turn the user's own files into scratch.
fn inherited_is_usable(path: &Path, home: Option<&Path>) -> bool {
    let holds_home = |dir: &Path| home.is_some_and(|home| home.starts_with(dir));
    path.is_absolute()
        && !holds_home(path)
        && !path.canonicalize().is_ok_and(|real| holds_home(&real))
}

fn inherited_tmpdir() -> Option<PathBuf> {
    std::env::var_os("TMPDIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// True when the inherited `TMPDIR` is unset or is one of [`roots`].
pub fn inherited_tmpdir_is_scratch() -> bool {
    inherited_tmpdir().is_none_or(|dir| roots().contains(&dir))
}

pub fn roots() -> &'static [PathBuf] {
    static ROOTS: LazyLock<Vec<PathBuf>> = LazyLock::new(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut paths: Vec<PathBuf> = inherited_tmpdir()
            .into_iter()
            .filter(|path| inherited_is_usable(path, home.as_deref()))
            .collect();
        paths.extend(["/tmp", "/private/tmp", "/var/tmp"].map(PathBuf::from));
        #[cfg(target_os = "macos")]
        if let Some(path) = user_temp_dir() {
            paths.push(path);
        }
        paths.retain(|path| path.is_absolute() && path.parent().is_some());
        // Include canonical aliases, e.g. macOS /private/var/tmp.
        let aliases: Vec<_> = paths.iter().filter_map(|p| p.canonicalize().ok()).collect();
        paths.extend(aliases);
        paths.sort();
        paths.dedup();
        paths
    });
    &ROOTS
}

#[cfg(target_os = "macos")]
fn user_temp_dir() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    // SAFETY: a null buffer with length zero queries the required size.
    let size = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0) };
    if size == 0 {
        return None;
    }
    let mut bytes = vec![0u8; size];
    // SAFETY: the buffer has the queried capacity. A changed size is rejected.
    let written = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            bytes.as_mut_ptr().cast(),
            size,
        )
    };
    if written == 0 || written > size {
        return None;
    }
    bytes.truncate(written - 1);
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_temp_setting_that_holds_the_home_directory_is_no_scratch_root() {
        let home = Path::new("/home/u");
        for bad in ["/home/u", "/home", "/", "relative/tmp"] {
            assert!(!inherited_is_usable(Path::new(bad), Some(home)), "{bad}");
        }
        for good in ["/home/u/.cache/tmp", "/tmp", "/var/folders/ab/T"] {
            assert!(inherited_is_usable(Path::new(good), Some(home)), "{good}");
        }
    }
}
