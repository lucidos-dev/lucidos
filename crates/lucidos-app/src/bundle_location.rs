use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum BundleLocation {
    Stable,
    Translocated,
    ReadOnlyVolume,
}

impl BundleLocation {
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

const QUIT_THEN_MOVE: &str =
    "Quit Lucidos, drag it into your Applications folder, and open it from there.";

fn stop_service_then_move() -> String {
    format!(
        "Choose \"{}\" from the Lucidos menu or the menu bar icon. Then drag Lucidos into \
         your Applications folder, and open it from there.",
        crate::QUIT_AND_STOP_SERVICE
    )
}

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

pub fn launch_refusal(location: &BundleLocation) -> Option<String> {
    location.problem().map(|problem| {
        format!(
            "{problem} From there it cannot keep its background service running or update \
             itself.\n\n{QUIT_THEN_MOVE}"
        )
    })
}

pub fn update_blocker(location: &BundleLocation, same_device_as_temp: bool) -> Option<String> {
    let instruction = stop_service_then_move();
    if let Some(problem) = location.problem() {
        return Some(format!(
            "{problem} It cannot replace itself there. {instruction}"
        ));
    }
    (!same_device_as_temp).then(|| {
        format!(
            "Lucidos can only update itself on your startup disk, and it is on another disk. \
             {instruction}"
        )
    })
}

pub fn remote_install_blocker(
    install_blocker: Option<String>,
    writable_in_place: bool,
) -> Option<String> {
    install_blocker.or_else(|| {
        (!writable_in_place).then(|| {
            "Updating Lucidos here needs an administrator password, so it can only be done \
             at this Mac."
                .to_string()
        })
    })
}

pub fn writable_in_place(bundle: &Path) -> bool {
    bundle
        .parent()
        .is_some_and(|parent| user_may_write(parent) && user_may_write(bundle))
}

fn user_may_write(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call.
    unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 }
}

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

pub fn current(bundle: &Path) -> BundleLocation {
    classify(bundle, is_read_only_volume(bundle).unwrap_or(false))
}

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

pub fn same_device(a: &Path, b: &Path) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;

    Some(std::fs::metadata(a).ok()?.dev() == std::fs::metadata(b).ok()?.dev())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn every_launch_refusal_ends_on_quit_then_move() {
        for location in [BundleLocation::Translocated, BundleLocation::ReadOnlyVolume] {
            let message = launch_refusal(&location).unwrap_or_default();
            assert!(message.ends_with(QUIT_THEN_MOVE), "{location:?}: {message}");
        }
    }

    #[test]
    fn every_update_blocker_names_the_real_quit() {
        for (location, same_device) in [
            (BundleLocation::Translocated, true),
            (BundleLocation::Translocated, false),
            (BundleLocation::ReadOnlyVolume, true),
            (BundleLocation::ReadOnlyVolume, false),
            (BundleLocation::Stable, false),
        ] {
            let message = update_blocker(&location, same_device).unwrap_or_default();
            assert!(
                message.ends_with(&stop_service_then_move()),
                "{location:?}, same device {same_device}: {message}"
            );
            assert!(
                message.contains(crate::QUIT_AND_STOP_SERVICE),
                "{location:?}, same device {same_device}: {message}"
            );
            assert!(
                !message.contains(QUIT_THEN_MOVE),
                "{location:?}, same device {same_device}: {message}"
            );
        }
    }

    #[test]
    fn a_stable_bundle_on_the_startup_disk_can_update() {
        assert_eq!(update_blocker(&BundleLocation::Stable, true), None);
    }

    #[test]
    fn a_stable_bundle_on_another_disk_cannot_update() {
        let message = update_blocker(&BundleLocation::Stable, false).unwrap_or_default();
        assert!(message.contains("another disk"), "{message}");
    }

    #[test]
    fn two_paths_in_one_directory_share_a_device() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        std::fs::create_dir_all(&a).expect("create a");
        assert_eq!(same_device(&a, tmp.path()), Some(true));
    }

    #[test]
    fn a_missing_path_has_no_device_answer() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(same_device(&tmp.path().join("missing"), tmp.path()), None);
    }

    #[test]
    fn the_temp_dir_is_on_a_writable_volume() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(is_read_only_volume(tmp.path()), Some(false));
    }

    #[test]
    fn a_bundle_this_user_can_move_has_no_remote_blocker() {
        assert_eq!(remote_install_blocker(None, true), None);
    }

    #[test]
    fn a_bundle_this_user_cannot_move_blocks_a_remote_install() {
        let message = remote_install_blocker(None, false).unwrap_or_default();
        assert!(message.contains("administrator password"), "{message}");
    }

    #[test]
    fn the_install_blocker_wins_over_the_writability_check() {
        let local = update_blocker(&BundleLocation::ReadOnlyVolume, true);
        assert_eq!(remote_install_blocker(local.clone(), false), local);
    }

    #[test]
    fn a_bundle_in_a_folder_this_user_owns_is_writable_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("Lucidos.app");
        std::fs::create_dir_all(&bundle).expect("create the bundle");
        assert!(writable_in_place(&bundle));
    }

    #[test]
    fn a_bundle_in_a_read_only_folder_is_not_writable_in_place() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("Applications");
        let bundle = parent.join("Lucidos.app");
        std::fs::create_dir_all(&bundle).expect("create the bundle");
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555))
            .expect("make the folder read-only");
        let writable = writable_in_place(&bundle);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755))
            .expect("restore the folder so the temp dir can be removed");
        assert!(!writable);
    }

    #[test]
    fn a_missing_bundle_is_not_writable_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!writable_in_place(&tmp.path().join("Lucidos.app")));
    }
}
