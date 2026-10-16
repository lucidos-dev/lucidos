//! The main check as clients see it. `GET /api/v1/changes`, every
//! `ChangesUpdated` frame and `GET /api/v1/main-check` carry it.
//!
//! The driver publishes it whenever it changes, so it is always the driver's
//! own state. A restart starts it idle until recovery restores the check.

use super::state::MainCheck;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where the main check stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MainCheckState {
    /// Nothing runs, no red is open, and the last check did not pass.
    #[default]
    Idle,
    Running,
    /// The last check passed, and nothing has landed since.
    Passed,
    /// The last check failed, and its fix thread owns it.
    Red,
    /// This checkout cannot run the check. `summary` says why.
    Unavailable,
}

/// The main check right now. Fields a state has nothing for are absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainCheckSnapshot {
    pub state: MainCheckState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_thread_id: Option<Uuid>,
    /// What failed, for a red check; why the check cannot run, when
    /// unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl MainCheckSnapshot {
    /// The snapshot of `check`. `unavailable` is why this checkout cannot run
    /// the check, and `run_url` is the running check's run once known.
    pub(crate) fn of(check: &MainCheck, unavailable: Option<&str>, run_url: Option<&str>) -> Self {
        if let Some(running) = check.running() {
            return Self {
                state: MainCheckState::Running,
                check_id: Some(running.check_id),
                from_sha: Some(running.from_sha.clone()),
                to_sha: Some(running.to_sha.clone()),
                run_url: run_url.map(str::to_string),
                ..Self::default()
            };
        }
        if let Some(red) = check.red() {
            return Self {
                state: MainCheckState::Red,
                check_id: Some(red.range.check_id),
                from_sha: Some(red.range.from_sha.clone()),
                to_sha: Some(red.range.to_sha.clone()),
                run_url: red.run_url.clone(),
                fix_thread_id: Some(red.fix_thread_id),
                summary: Some(red.summary.clone()),
            };
        }
        if let Some(reason) = unavailable {
            return Self {
                state: MainCheckState::Unavailable,
                summary: Some(reason.to_string()),
                ..Self::default()
            };
        }
        match check.passed() {
            Some(passed) => Self {
                state: MainCheckState::Passed,
                check_id: Some(passed.check_id),
                from_sha: Some(passed.from_sha.clone()),
                to_sha: Some(passed.to_sha.clone()),
                ..Self::default()
            },
            None => Self::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::state::Verdict;
    use super::*;

    fn id(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn running() -> MainCheck {
        let mut check = MainCheck::default();
        check.landed(Some("a"), false, Some("b"), id(1));
        check
    }

    #[test]
    fn an_idle_check_is_idle_and_names_nothing() {
        assert_eq!(
            MainCheckSnapshot::of(&MainCheck::default(), None, None),
            MainCheckSnapshot::default()
        );
    }

    /// A running check names its range, and its run once the run started.
    #[test]
    fn a_running_check_names_its_range_and_run() {
        let snapshot = MainCheckSnapshot::of(&running(), Some("no remote"), Some("https://r"));
        assert_eq!(snapshot.state, MainCheckState::Running);
        assert_eq!(snapshot.check_id, Some(id(1)));
        assert_eq!(snapshot.from_sha.as_deref(), Some("a"));
        assert_eq!(snapshot.to_sha.as_deref(), Some("b"));
        assert_eq!(snapshot.run_url.as_deref(), Some("https://r"));
        assert_eq!(snapshot.summary, None, "running outranks unavailable");
    }

    /// Red names what failed and the thread fixing it. It outranks unavailable,
    /// which only stops the next check.
    #[test]
    fn a_red_check_names_its_summary_and_fix_thread() {
        let mut check = running();
        check.ended(
            id(1),
            Verdict::Failed {
                summary: "Failed: engine.".into(),
                tail: String::new(),
                run_url: Some("https://r".into()),
                fix_thread_id: id(9),
            },
            Some("b"),
            id(2),
        );
        let snapshot = MainCheckSnapshot::of(&check, Some("no remote"), None);
        assert_eq!(snapshot.state, MainCheckState::Red);
        assert_eq!(snapshot.fix_thread_id, Some(id(9)));
        assert_eq!(snapshot.summary.as_deref(), Some("Failed: engine."));
        assert_eq!(snapshot.run_url.as_deref(), Some("https://r"));
    }

    #[test]
    fn an_unavailable_check_says_why() {
        let snapshot = MainCheckSnapshot::of(&MainCheck::default(), Some("no remote"), None);
        assert_eq!(snapshot.state, MainCheckState::Unavailable);
        assert_eq!(snapshot.summary.as_deref(), Some("no remote"));
    }

    #[test]
    fn a_passed_check_names_the_range_it_cleared() {
        let mut check = running();
        check.ended(id(1), Verdict::Passed, Some("b"), id(2));
        let snapshot = MainCheckSnapshot::of(&check, None, None);
        assert_eq!(snapshot.state, MainCheckState::Passed);
        assert_eq!(snapshot.to_sha.as_deref(), Some("b"));
    }

    /// The wire shape clients read: snake_case states, absent empty fields.
    #[test]
    fn the_wire_shape_is_snake_case_and_omits_what_a_state_lacks() {
        let json = serde_json::to_value(MainCheckSnapshot::default()).unwrap();
        assert_eq!(json, serde_json::json!({ "state": "idle" }));
        let json = serde_json::to_value(MainCheckSnapshot::of(&running(), None, None)).unwrap();
        assert_eq!(json["state"], "running");
        assert_eq!(json["check_id"], id(1).to_string());
        assert!(json.get("run_url").is_none());
    }
}
