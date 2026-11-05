//! What this process was started to be, decided from argv before any Tauri or
//! AppKit init.
//!
//! The one place the bundle binary reads its own flags. Anything it does not
//! recognise is a client, so a flag that should exit early has to be named
//! here. ADR 0471.

use std::ffi::{OsStr, OsString};

#[derive(Debug, PartialEq, Eq)]
pub enum LaunchRole {
    /// `--version` / `-V`: print [`version_line`] and exit, touching nothing.
    Version,
    /// `--service`: the headless launchd role (`desktop::run_service`).
    Service,
    /// Every other launch: the GUI client.
    Client,
}

/// Decide the role from a full argv, executable path included.
///
/// Version wins over service, because a version query must never boot
/// anything.
pub fn launch_role(args: impl IntoIterator<Item = OsString>) -> LaunchRole {
    // argv[0] is the executable, and a bundle could live under a directory
    // named like a flag.
    let flags: Vec<OsString> = args.into_iter().skip(1).collect();
    let has = |flag: &str| flags.iter().any(|a| a == OsStr::new(flag));
    if has("--version") || has("-V") {
        LaunchRole::Version
    } else if has("--service") {
        LaunchRole::Service
    } else {
        LaunchRole::Client
    }
}

/// The `--version` answer: one bare line on stdout, the gateway's shape.
pub fn version_line() -> String {
    format!(
        "Lucidos {} (lucidos-app {})",
        crate::LUCIDOS_RELEASE,
        env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<OsString> {
        std::iter::once(OsString::from(
            "/Applications/Lucidos.app/Contents/MacOS/lucidos-app",
        ))
        .chain(words.iter().map(OsString::from))
        .collect()
    }

    #[test]
    fn a_version_query_never_boots_a_client() {
        assert_eq!(launch_role(argv(&["--version"])), LaunchRole::Version);
        assert_eq!(launch_role(argv(&["-V"])), LaunchRole::Version);
        assert_eq!(
            launch_role(argv(&["--service", "--version"])),
            LaunchRole::Version
        );
    }

    #[test]
    fn the_service_flag_selects_the_headless_role() {
        assert_eq!(launch_role(argv(&["--service"])), LaunchRole::Service);
        assert_eq!(
            launch_role(argv(&["--other", "--service"])),
            LaunchRole::Service
        );
    }

    #[test]
    fn every_other_launch_is_a_client() {
        assert_eq!(launch_role(argv(&[])), LaunchRole::Client);
        assert_eq!(launch_role(argv(&["--login"])), LaunchRole::Client);
        // Not a prefix match.
        assert_eq!(launch_role(argv(&["--versions"])), LaunchRole::Client);
    }

    #[test]
    fn the_executable_path_is_never_read_as_a_flag() {
        assert_eq!(
            launch_role(vec![OsString::from("--service")]),
            LaunchRole::Client
        );
        assert_eq!(
            launch_role(vec![OsString::from("--version")]),
            LaunchRole::Client
        );
    }

    #[test]
    fn the_version_line_names_the_release() {
        let line = version_line();
        assert!(line.starts_with(&format!("Lucidos {} ", crate::LUCIDOS_RELEASE)));
        assert!(!line.contains('\n'));
    }
}
