//! The main check as clients see it. `GET /api/v1/changes`, every
//! `ChangesUpdated` frame and `GET /api/v1/main-check` carry it.
//!
//! The driver publishes it whenever it changes, so it is always the driver's
//! own state. A restart starts it idle until recovery restores the check.

use super::state::MainCheck;

pub use crate::engine::thread_events::main_check::{
    MainCheckSnapshot, MainCheckState, MainRepairSnapshot, RepairState,
};

/// The snapshot of `check`. `unavailable` is why this checkout cannot run
/// the check, and `run_url` is the running check's run once known.
pub(crate) fn snapshot_of(
    check: &MainCheck,
    unavailable: Option<&str>,
    run_url: Option<&str>,
) -> MainCheckSnapshot {
    MainCheckSnapshot {
        failures: check.red().map(|r| r.failures.clone()).unwrap_or_default(),
        flaky: check
            .flaky_fix()
            .map(|f| f.flaky.clone())
            .unwrap_or_default(),
        flaky_fix_thread_id: check.flaky_fix().map(|flaky| flaky.fix_thread_id),
        ..snapshot_of_state(check, unavailable, run_url)
    }
}

fn snapshot_of_state(
    check: &MainCheck,
    unavailable: Option<&str>,
    run_url: Option<&str>,
) -> MainCheckSnapshot {
    if let Some(red) = check.red() {
        return MainCheckSnapshot {
            state: MainCheckState::Red,
            check_id: Some(red.range.check_id),
            from_sha: Some(red.range.from_sha.clone()),
            to_sha: Some(red.range.to_sha.clone()),
            run_url: red.run_url.clone(),
            fix_thread_id: Some(red.fix_thread_id),
            summary: Some(red.summary.clone()),
            ..MainCheckSnapshot::default()
        };
    }
    if let Some(running) = check.running() {
        return MainCheckSnapshot {
            state: MainCheckState::Running,
            check_id: Some(running.check_id),
            from_sha: Some(running.from_sha.clone()),
            to_sha: Some(running.to_sha.clone()),
            run_url: run_url.map(str::to_string),
            ..MainCheckSnapshot::default()
        };
    }
    if let Some(reason) = unavailable {
        return MainCheckSnapshot {
            state: MainCheckState::Unavailable,
            summary: Some(reason.to_string()),
            ..MainCheckSnapshot::default()
        };
    }
    match check.passed() {
        Some(passed) => MainCheckSnapshot {
            state: MainCheckState::Passed,
            check_id: Some(passed.check_id),
            from_sha: Some(passed.from_sha.clone()),
            to_sha: Some(passed.to_sha.clone()),
            ..MainCheckSnapshot::default()
        },
        None => MainCheckSnapshot::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::state::Verdict;
    use super::*;
    use uuid::Uuid;

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
            snapshot_of(&MainCheck::default(), None, None),
            MainCheckSnapshot::default()
        );
    }

    /// A running check names its range, and its run once the run started.
    #[test]
    fn a_running_check_names_its_range_and_run() {
        let snapshot = snapshot_of(&running(), Some("no remote"), Some("https://r"));
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
                failures: Vec::new(),
                run_url: Some("https://r".into()),
                fix_thread_id: id(9),
            },
            Some("b"),
            id(2),
        );
        let snapshot = snapshot_of(&check, Some("no remote"), None);
        assert_eq!(snapshot.state, MainCheckState::Red);
        assert_eq!(snapshot.fix_thread_id, Some(id(9)));
        assert_eq!(snapshot.summary.as_deref(), Some("Failed: engine."));
        assert_eq!(snapshot.run_url.as_deref(), Some("https://r"));
    }

    #[test]
    fn an_unavailable_check_says_why() {
        let snapshot = snapshot_of(&MainCheck::default(), Some("no remote"), None);
        assert_eq!(snapshot.state, MainCheckState::Unavailable);
        assert_eq!(snapshot.summary.as_deref(), Some("no remote"));
    }

    #[test]
    fn a_passed_check_names_the_range_it_cleared() {
        let mut check = running();
        check.ended(id(1), Verdict::Passed, Some("b"), id(2));
        let snapshot = snapshot_of(&check, None, None);
        assert_eq!(snapshot.state, MainCheckState::Passed);
        assert_eq!(snapshot.to_sha.as_deref(), Some("b"));
    }

    /// The wire shape clients read: snake_case states, absent empty fields.
    #[test]
    fn the_wire_shape_is_snake_case_and_omits_what_a_state_lacks() {
        let json = serde_json::to_value(MainCheckSnapshot::default()).unwrap();
        assert_eq!(json, serde_json::json!({ "state": "idle" }));
        let json = serde_json::to_value(snapshot_of(&running(), None, None)).unwrap();
        assert_eq!(json["state"], "running");
        assert_eq!(json["check_id"], id(1).to_string());
        assert!(json.get("run_url").is_none());
    }
}

#[cfg(test)]
mod continued_red_tests {
    use super::*;
    use crate::engine::main_check::state::Verdict;
    use uuid::Uuid;

    #[test]
    fn checking_newer_main_keeps_red_visible() {
        let mut check = MainCheck::default();
        let first = Uuid::new_v4();
        let owner = Uuid::new_v4();
        check.landed(Some("a"), false, Some("b"), first);
        check.ended(
            first,
            Verdict::Failed {
                summary: "Failed: engine".into(),
                tail: String::new(),
                failures: Vec::new(),
                run_url: None,
                fix_thread_id: owner,
            },
            Some("b"),
            Uuid::new_v4(),
        );
        check.landed(Some("b"), false, Some("c"), Uuid::new_v4());
        assert!(check.running().is_some());
        let snapshot = snapshot_of(&check, None, None);
        assert_eq!(snapshot.state, MainCheckState::Red);
        assert_eq!(snapshot.fix_thread_id, Some(owner));
    }
}
