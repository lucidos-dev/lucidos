use std::ffi::{OsStr, OsString};

#[derive(Debug, PartialEq, Eq)]
pub enum LaunchRole {
    Version,
    Service,
    DisposableService(Vec<OsString>),
    CaptureClientRelaunchLog,
    RecordClientRelaunch(RelaunchOutcome),
    Client,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelaunchOutcome {
    PreviousClientStillRunning,
    ClientObserved,
    AttemptsExhausted,
}

impl RelaunchOutcome {
    const ALL: [Self; 3] = [
        Self::PreviousClientStillRunning,
        Self::ClientObserved,
        Self::AttemptsExhausted,
    ];

    pub(crate) fn arg(self) -> &'static str {
        match self {
            Self::PreviousClientStillRunning => "--record-relaunch-client-still-running",
            Self::ClientObserved => "--record-relaunch-client-observed",
            Self::AttemptsExhausted => "--record-relaunch-attempts-exhausted",
        }
    }
}

pub fn launch_role(args: impl IntoIterator<Item = OsString>) -> LaunchRole {
    let flags: Vec<OsString> = args.into_iter().skip(1).collect();
    let has = |flag: &str| flags.iter().any(|a| a == OsStr::new(flag));
    if has("--version") || has("-V") {
        LaunchRole::Version
    } else if has("--disposable-service") || has("--disposable-supervisor") {
        LaunchRole::DisposableService(flags)
    } else if has("--capture-client-relaunch-log") {
        LaunchRole::CaptureClientRelaunchLog
    } else if let Some(outcome) = RelaunchOutcome::ALL
        .into_iter()
        .find(|outcome| has(outcome.arg()))
    {
        LaunchRole::RecordClientRelaunch(outcome)
    } else if has("--service") {
        LaunchRole::Service
    } else {
        LaunchRole::Client
    }
}

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
    fn disposable_launch_never_enters_the_persistent_service_or_client() {
        assert!(matches!(
            launch_role(argv(&["--disposable-service", "/tmp/run", "5300"])),
            LaunchRole::DisposableService(_)
        ));
        assert!(matches!(
            launch_role(argv(&["--service", "--disposable-service"])),
            LaunchRole::DisposableService(_)
        ));
        assert_eq!(
            launch_role(argv(&["--disposable-service", "--version"])),
            LaunchRole::Version
        );
    }

    #[test]
    fn relaunch_capture_is_headless_and_version_still_wins() {
        assert_eq!(
            launch_role(argv(&["--capture-client-relaunch-log"])),
            LaunchRole::CaptureClientRelaunchLog
        );
        assert_eq!(
            launch_role(argv(&["--capture-client-relaunch-log", "--version"])),
            LaunchRole::Version
        );
    }

    #[test]
    fn each_watcher_outcome_selects_a_headless_role() {
        for outcome in RelaunchOutcome::ALL {
            assert_eq!(
                launch_role(argv(&[outcome.arg()])),
                LaunchRole::RecordClientRelaunch(outcome)
            );
        }
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
