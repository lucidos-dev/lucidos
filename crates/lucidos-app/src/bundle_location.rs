//! Is the running `.app` somewhere it can keep?
//!
//! Two places look like an install and are not. A mounted `.dmg` is read-only
//! and disappears on eject. A Gatekeeper App Translocation copy is read-only
//! and gets a fresh random path on every launch. From either one, the launchd
//! service plist pins a path that vanishes, and the updater's first rename
//! fails with EXDEV. ADR 0271 records why the client refuses to run there.

use std::path::{Path, PathBuf};

/// Where the running bundle lives, as far as keeping it goes.
#[derive(Debug, PartialEq, Eq)]
pub enum BundleLocation {
    /// A normal install: `/Applications`, `~/Applications`, a writable disk.
    Stable,
    /// A Gatekeeper App Translocation copy of a quarantined app.
    Translocated,
    /// A read-only volume, which in practice is the mounted `.dmg`.
    ReadOnlyVolume,
}

impl BundleLocation {
    /// Why this place is no good, as the opening of a sentence. `None` when
    /// the bundle can stay where it is.
    fn problem(&self) -> Option<&'static str> {
        match self {
            Self::Stable => None,
            Self::Translocated => Some(
                "Lucidos is running from a temporary copy that macOS made, because it was \
                 opened from where it was downloaded.",
            ),
            Self::ReadOnlyVolume => {
                Some("Lucidos is running from the disk image it was opened from.")
            }
        }
    }
}

/// The one instruction every unstable-location message ends on.
const MOVE_TO_APPLICATIONS: &str =
    "Quit Lucidos, drag it into your Applications folder, and open it from there.";

/// Classify a bundle path. Pure: the caller reads the mount flags, so every
/// case is testable without a real mount.
///
/// Translocation wins over read-only, since a translocation mount is both and
/// its message is the more specific.
pub fn classify(bundle: &Path, read_only_volume: bool) -> BundleLocation {
    if bundle
        .components()
        .any(|c| c.as_os_str() == "AppTranslocation")
    {
        BundleLocation::Translocated
    } else if read_only_volume {
        BundleLocation::ReadOnlyVolume
    } else {
        BundleLocation::Stable
    }
}

/// The launch dialog's body, or `None` for a stable location.
pub fn launch_refusal(location: &BundleLocation) -> Option<String> {
    location.problem().map(|problem| {
        format!(
            "{problem} From there it cannot keep its background service running or update \
             itself.\n\n{MOVE_TO_APPLICATIONS}"
        )
    })
}

/// Why an in-place update cannot work from here, or `None` when it can.
///
/// `same_device_as_temp` is the plugin's real precondition: it renames the
/// bundle into a directory under `$TMPDIR`, and a rename cannot cross devices.
/// That also catches a writable external disk, which is stable for the
/// service but not updatable.
pub fn update_blocker(location: &BundleLocation, same_device_as_temp: bool) -> Option<String> {
    if let Some(problem) = location.problem() {
        return Some(format!(
            "{problem} It cannot replace itself there. {MOVE_TO_APPLICATIONS}"
        ));
    }
    (!same_device_as_temp).then(|| {
        format!(
            "Lucidos can only update itself on your startup disk, and it is on another disk. \
             {MOVE_TO_APPLICATIONS}"
        )
    })
}

/// The bundle this process runs from, derived the way `tauri-plugin-updater`
/// derives the path it swaps: its own `extract_path_from_executable` over
/// `current_exe()`. `lib.rs` registers the plugin with no override, so the two
/// cannot disagree. ADR 0073 records why a hardcoded path is wrong.
pub fn bundle_path() -> Result<PathBuf, String> {
    let exe = tauri::utils::platform::current_exe()
        .map_err(|e| format!("cannot resolve this app's own executable: {e}"))?;
    tauri_plugin_updater::extract_path_from_executable(&exe).map_err(|e| {
        format!(
            "cannot resolve this app's bundle from {}: {e}",
            exe.display()
        )
    })
}

/// Classify the running bundle. Fails open: unreadable mount flags leave the
/// path check alone to decide. A failed `statfs` must not lock anybody out of
/// an app that works.
pub fn current(bundle: &Path) -> BundleLocation {
    classify(bundle, is_read_only_volume(bundle).unwrap_or(false))
}

/// Is `path` on a volume mounted read-only?
fn is_read_only_volume(path: &Path) -> Option<bool> {
    use std::os::unix::ffi::OsStrExt;

    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `c_path` is a valid NUL-terminated string and `stat` points at
    // writable storage of the right type. The struct is read only on success.
    let rc = unsafe { libc::statfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: `statfs` returned 0, so it filled the struct.
    let flags = unsafe { stat.assume_init() }.f_flags;
    Some(flags & libc::MNT_RDONLY as u32 != 0)
}

/// Are `a` and `b` on the same device? `None` when either cannot be read,
/// which the caller treats as "don't know" and lets the plugin report.
pub fn same_device(a: &Path, b: &Path) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;

    Some(std::fs::metadata(a).ok()?.dev() == std::fs::metadata(b).ok()?.dev())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    const TRANSLOCATED: &str =
        "/private/var/folders/ab/xyz/T/AppTranslocation/1F2E3D4C-0000-4000-8000-000000000000/d/Lucidos.app";

    #[test]
    fn a_translocated_bundle_is_translocated_whatever_the_mount_says() {
        for read_only in [true, false] {
            assert_eq!(
                classify(Path::new(TRANSLOCATED), read_only),
                BundleLocation::Translocated
            );
        }
    }

    #[test]
    fn a_bundle_on_a_read_only_volume_is_unstable() {
        assert_eq!(
            classify(Path::new("/Volumes/Lucidos/Lucidos.app"), true),
            BundleLocation::ReadOnlyVolume
        );
    }

    // The regression guard for every normal install. A false positive here
    // refuses to launch an app that works.
    #[test]
    fn ordinary_installs_are_stable() {
        for path in [
            "/Applications/Lucidos.app",
            "/Users/me/Applications/Lucidos.app",
            "/Volumes/Ext/Applications/Lucidos.app",
        ] {
            assert_eq!(
                classify(Path::new(path), false),
                BundleLocation::Stable,
                "{path}"
            );
        }
    }

    // A folder merely NAMED like the translocation root is not one. Only a
    // whole path component counts.
    #[test]
    fn a_lookalike_folder_name_is_not_translocation() {
        assert_eq!(
            classify(
                Path::new("/Users/me/AppTranslocationNotes/Lucidos.app"),
                false
            ),
            BundleLocation::Stable
        );
    }

    #[test]
    fn a_stable_location_says_nothing_at_launch() {
        assert_eq!(launch_refusal(&BundleLocation::Stable), None);
    }

    #[test]
    fn every_unstable_message_ends_on_the_same_instruction() {
        for location in [BundleLocation::Translocated, BundleLocation::ReadOnlyVolume] {
            for message in [
                launch_refusal(&location),
                update_blocker(&location, true),
                update_blocker(&location, false),
            ] {
                let message = message.unwrap_or_default();
                assert!(
                    message.ends_with(MOVE_TO_APPLICATIONS),
                    "{location:?}: {message}"
                );
            }
        }
    }

    #[test]
    fn a_stable_bundle_on_the_startup_disk_can_update() {
        assert_eq!(update_blocker(&BundleLocation::Stable, true), None);
    }

    // The writable external disk: fine for the service, but the plugin's rename
    // into $TMPDIR would fail with EXDEV, so the update must stop up front.
    #[test]
    fn a_stable_bundle_on_another_disk_cannot_update() {
        let message = update_blocker(&BundleLocation::Stable, false).unwrap_or_default();
        assert!(message.contains("another disk"), "{message}");
        assert!(message.ends_with(MOVE_TO_APPLICATIONS), "{message}");
    }

    #[test]
    fn two_paths_in_one_directory_share_a_device() {
        let tmp = TempDir::new("same-device");
        let a = tmp.path().join("a");
        std::fs::create_dir_all(&a).expect("create a");
        assert_eq!(same_device(&a, tmp.path()), Some(true));
    }

    #[test]
    fn a_missing_path_has_no_device_answer() {
        let tmp = TempDir::new("no-device");
        assert_eq!(same_device(&tmp.path().join("missing"), tmp.path()), None);
    }

    #[test]
    fn the_temp_dir_is_on_a_writable_volume() {
        assert_eq!(is_read_only_volume(&std::env::temp_dir()), Some(false));
    }
}
