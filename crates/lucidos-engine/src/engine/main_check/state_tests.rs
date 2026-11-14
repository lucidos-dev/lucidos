//! The scheduler's rules, one test each: one check at a time, one red at a
//! time, coalescing, and nothing started by a clock or by itself.

use super::*;

fn id(n: u8) -> Uuid {
    Uuid::from_bytes([n; 16])
}

fn range(n: u8, from: &str, to: &str) -> CheckRange {
    CheckRange {
        check_id: id(n),
        from_sha: from.into(),
        to_sha: to.into(),
    }
}

fn failed(fix: u8) -> Verdict {
    Verdict::Failed {
        summary: "Failed: engine.".into(),
        tail: "test result: FAILED".into(),
        failures: Vec::new(),
        run_url: Some("https://example.invalid/runs/1".into()),
        fix_thread_id: id(fix),
    }
}

/// A check running from `a` to `b`, as check 1.
fn running_a_to_b() -> MainCheck {
    let mut check = MainCheck::default();
    assert_eq!(
        check.landed(Some("a"), false, Some("b"), id(1)),
        vec![Effect::Start(range(1, "a", "b"))]
    );
    check
}

/// A red check 1 from `a` to `b`, with fix thread 9.
fn red_a_to_b() -> MainCheck {
    let mut check = running_a_to_b();
    assert!(check.ended(id(1), failed(9), Some("b"), id(2)).is_empty());
    check
}

#[test]
fn a_landing_while_idle_starts_a_check_of_main_from_the_landing_base() {
    let check = running_a_to_b();
    assert_eq!(check.running(), Some(&range(1, "a", "b")));
    assert!(check.red().is_none());
}

#[test]
fn a_landing_while_a_check_runs_starts_nothing_and_marks_dirty() {
    let mut check = running_a_to_b();
    assert!(check.landed(Some("b"), false, Some("c"), id(2)).is_empty());
    assert!(check.landed(Some("c"), false, Some("d"), id(3)).is_empty());
    assert!(check.dirty());
    assert_eq!(
        check.running(),
        Some(&range(1, "a", "b")),
        "still one check"
    );
}

#[test]
fn a_pass_with_landings_behind_it_starts_one_check_covering_them_all() {
    let mut check = running_a_to_b();
    check.landed(Some("b"), false, Some("c"), id(2));
    check.landed(Some("c"), false, Some("d"), id(3));
    assert_eq!(
        check.ended(id(1), Verdict::Passed, Some("d"), id(4)),
        vec![Effect::Start(range(4, "b", "d"))],
        "the next check starts at the last verdict and covers both landings"
    );
    assert!(!check.dirty());
}

#[test]
fn a_pass_with_nothing_behind_it_leaves_the_check_passed() {
    let mut check = running_a_to_b();
    assert!(check
        .ended(id(1), Verdict::Passed, Some("b"), id(2))
        .is_empty());
    assert!(check.running().is_none());
    assert_eq!(check.passed(), Some(&range(1, "a", "b")));
    assert_eq!(
        check.landed(Some("b"), false, Some("c"), id(3)),
        vec![Effect::Start(range(3, "b", "c"))],
        "the next landing covers from the passed sha"
    );
    assert!(check.passed().is_none(), "a running check replaces passed");
}

#[test]
fn a_failure_opens_the_red_and_checks_landings_behind_it() {
    let mut check = running_a_to_b();
    check.landed(Some("b"), false, Some("c"), id(2));
    assert_eq!(
        check.ended(id(1), failed(9), Some("c"), id(3)),
        vec![Effect::Start(range(3, "b", "c"))]
    );
    let red = check.red().expect("red is open");
    assert_eq!(red.range, range(1, "a", "b"));
    assert_eq!(red.fix_thread_id, id(9));
    assert_eq!(check.running(), Some(&range(3, "b", "c")));
    assert!(!check.dirty());
}

#[test]
fn red_allows_new_checks_and_preserves_the_repair_owner() {
    let mut check = red_a_to_b();
    assert_eq!(
        check.landed(Some("b"), false, Some("c"), id(2)),
        vec![Effect::Start(range(2, "b", "c"))]
    );
    check.ended(id(2), failed(8), Some("c"), id(3));
    assert_eq!(check.red().unwrap().fix_thread_id, id(9));
    assert_eq!(check.red().unwrap().range.check_id, id(2));
}

#[test]
fn a_newer_pass_closes_red_with_its_own_reason() {
    let mut check = red_a_to_b();
    check.landed(Some("b"), false, Some("c"), id(2));
    assert_eq!(
        check.ended(id(2), Verdict::Passed, Some("c"), id(3)),
        vec![Effect::CloseRed {
            check_id: id(1),
            cause: RedCloseCause::PassedOnNewerMain
        }]
    );
    assert!(check.red().is_none());
}

#[test]
fn a_check_predating_the_fix_cannot_reopen_red() {
    let mut check = red_a_to_b();
    check.landed(Some("b"), false, Some("c"), id(2));
    assert_eq!(
        check.landed(Some("c"), true, Some("d"), id(3)),
        vec![Effect::CloseRed {
            check_id: id(1),
            cause: RedCloseCause::FixLanded
        }]
    );
    assert_eq!(
        check.ended(id(2), failed(8), Some("d"), id(4)),
        vec![Effect::Start(range(4, "c", "d"))]
    );
    assert!(check.red().is_none());
}

#[test]
fn a_landing_from_the_fix_thread_closes_the_red_and_starts_a_check() {
    let mut check = red_a_to_b();
    assert_eq!(
        check.landed(Some("b"), true, Some("d"), id(3)),
        vec![
            Effect::CloseRed {
                check_id: id(1),
                cause: RedCloseCause::FixLanded,
            },
            Effect::Start(range(3, "b", "d")),
        ]
    );
    assert!(check.red().is_none());
}

#[test]
fn the_maintainer_closing_the_red_starts_a_check_of_what_landed_meanwhile() {
    let mut check = red_a_to_b();
    check.landed(Some("b"), false, None, id(2));
    assert_eq!(
        check.close(Some("c"), id(3)),
        Ok(vec![
            Effect::CloseRed {
                check_id: id(1),
                cause: RedCloseCause::Closed,
            },
            Effect::Start(range(3, "b", "c")),
        ])
    );
}

#[test]
fn closing_a_red_with_nothing_landed_since_starts_nothing() {
    let mut check = red_a_to_b();
    assert_eq!(
        check.close(Some("b"), id(2)),
        Ok(vec![Effect::CloseRed {
            check_id: id(1),
            cause: RedCloseCause::Closed,
        }])
    );
    assert!(check.running().is_none());
    assert_eq!(check.close(Some("b"), id(3)), Err(CloseRefusal::NoRed));
}

#[test]
fn close_is_refused_with_no_red_open() {
    let mut idle = MainCheck::default();
    assert_eq!(idle.close(Some("a"), id(1)), Err(CloseRefusal::NoRed));
    let mut running = running_a_to_b();
    assert_eq!(running.close(Some("b"), id(2)), Err(CloseRefusal::NoRed));
    assert!(
        running.running().is_some(),
        "a refused close touches nothing"
    );
}

#[test]
fn an_abandoned_check_starts_nothing_even_with_landings_behind_it() {
    let mut check = running_a_to_b();
    check.landed(Some("b"), false, Some("c"), id(2));
    assert!(check
        .ended(id(1), Verdict::Abandoned, Some("c"), id(3))
        .is_empty());
    assert!(check.running().is_none());
    assert!(check.red().is_none());
    assert!(check.passed().is_none());
    assert_eq!(
        check.landed(Some("c"), false, Some("d"), id(4)),
        vec![Effect::Start(range(4, "a", "d"))],
        "the next landing starts it, still from the last verdict"
    );
}

#[test]
fn a_verdict_for_another_check_changes_nothing() {
    let mut check = running_a_to_b();
    let before = check.clone();
    assert!(check
        .ended(id(7), Verdict::Passed, Some("b"), id(2))
        .is_empty());
    assert!(check.ended(id(7), failed(9), Some("b"), id(3)).is_empty());
    assert_eq!(check, before);
}

#[test]
fn nothing_starts_when_main_has_not_moved_past_the_base() {
    let mut check = running_a_to_b();
    check.ended(id(1), Verdict::Passed, Some("b"), id(2));
    assert!(
        check.landed(None, false, Some("b"), id(3)).is_empty(),
        "a landing that left main at the last verdict has nothing to check"
    );
}

/// Git could not read `main` as a pass came in. The landing during the run
/// stays owed rather than read as "nothing new".
#[test]
fn a_pass_with_main_unknown_keeps_the_landings_behind_it_owed() {
    let mut check = running_a_to_b();
    check.landed(Some("b"), false, Some("c"), id(2));
    assert!(check.ended(id(1), Verdict::Passed, None, id(3)).is_empty());
    assert!(check.running().is_none());
    assert!(check.dirty(), "the landing during the run is still owed");
    assert_eq!(
        check.landed(Some("c"), false, Some("d"), id(4)),
        vec![Effect::Start(range(4, "b", "d"))],
        "the next start covers it"
    );
}

/// A fix landing closes its red even when no check can start.
#[test]
fn a_fix_landing_with_main_unknown_closes_the_red_and_starts_nothing() {
    let mut check = red_a_to_b();
    assert_eq!(
        check.landed(Some("b"), true, None, id(2)),
        vec![Effect::CloseRed {
            check_id: id(1),
            cause: RedCloseCause::FixLanded,
        }]
    );
    assert!(check.red().is_none());
    assert!(check.running().is_none());
    assert!(check.dirty(), "the fix itself is still unchecked");
    assert_eq!(
        check.landed(Some("c"), false, Some("d"), id(3)),
        vec![Effect::Start(range(3, "b", "d"))]
    );
}

#[test]
fn closing_the_red_with_main_unknown_keeps_what_landed_owed() {
    let mut check = red_a_to_b();
    check.landed(Some("b"), false, None, id(2));
    assert_eq!(
        check.close(None, id(3)),
        Ok(vec![Effect::CloseRed {
            check_id: id(1),
            cause: RedCloseCause::Closed,
        }])
    );
    assert!(check.dirty(), "what landed while red is still owed");
}

#[test]
fn at_most_one_check_runs_and_at_most_one_red_is_open_across_a_long_day() {
    let mut check = MainCheck::default();
    let mut next = 0u8;
    let mut fresh = || {
        next += 1;
        id(next)
    };
    let mut main = 0u32;
    for round in 0..60u32 {
        main += 1;
        let tip = format!("m{main}");
        let pre = format!("m{}", main - 1);
        let from_fix = round % 7 == 3;
        check.landed(Some(&pre), from_fix, Some(&tip), fresh());
        if let Some(running) = check.running().cloned() {
            let verdict = match round % 4 {
                0 => Verdict::Passed,
                1 => Verdict::Abandoned,
                2 => failed(200),
                _ => continue,
            };
            check.ended(running.check_id, verdict, Some(&tip), fresh());
        }
        if round % 11 == 10 {
            let _ = check.close(Some(&tip), fresh());
        }
    }
}

fn passed_flaky(fix: u8) -> Verdict {
    Verdict::PassedWithFlakyFix {
        run_url: None,
        flaky: Vec::new(),
        fix_thread_id: id(fix),
    }
}

/// A pass with flaky tests is a pass: it opens the flaky fix thread and
/// holds no check, so the next landing starts one at once.
#[test]
fn a_flaky_pass_opens_a_flaky_fix_thread_and_holds_no_check() {
    let mut check = running_a_to_b();
    assert!(check
        .ended(id(1), passed_flaky(8), Some("b"), id(2))
        .is_empty());
    assert_eq!(check.flaky_fix().map(|f| f.fix_thread_id), Some(id(8)));
    assert!(check.red().is_none());
    assert_eq!(check.passed(), Some(&range(1, "a", "b")));
    assert_eq!(
        check.landed(Some("b"), false, Some("c"), id(3)),
        vec![Effect::Start(range(3, "b", "c"))],
        "the flaky thread blocks nothing"
    );
    assert!(
        check.flaky_fix().is_some(),
        "and an unrelated landing keeps it open"
    );
}

/// Only a change from the flaky fix thread closes it.
#[test]
fn only_a_landing_from_the_flaky_thread_closes_it() {
    let mut check = running_a_to_b();
    check.ended(id(1), passed_flaky(8), Some("b"), id(2));
    assert!(!check.close_flaky_for(id(5)));
    assert!(check.flaky_fix().is_some());
    assert!(check.close_flaky_for(id(8)));
    assert!(check.flaky_fix().is_none());
}

/// The flaky thread and the red are independent: a red opens beside an open
/// flaky thread, and neither closes the other.
#[test]
fn a_red_reuses_the_open_flaky_repair() {
    let mut check = running_a_to_b();
    check.ended(id(1), passed_flaky(8), Some("b"), id(2));
    check.landed(Some("b"), false, Some("c"), id(3));
    check.ended(id(3), failed(9), Some("c"), id(4));
    assert_eq!(check.red().unwrap().fix_thread_id, id(8));
    assert_eq!(check.flaky_fix().map(|f| f.fix_thread_id), Some(id(8)));
    check.landed(Some("c"), true, Some("d"), id(5));
    assert!(check.red().is_none(), "the red's fix closes the red");
    assert!(
        check.flaky_fix().is_some(),
        "and leaves the flaky thread open"
    );
}

#[test]
fn flaky_findings_attach_to_the_red_owner() {
    let mut check = red_a_to_b();
    check.landed(Some("b"), false, Some("c"), id(2));
    check.ended(id(2), passed_flaky(8), Some("c"), id(3));
    assert!(check.red().is_none());
    assert_eq!(check.flaky_fix().unwrap().fix_thread_id, id(9));
}

#[test]
fn closing_red_preserves_landings_owed_behind_a_running_check() {
    let mut check = red_a_to_b();
    check.landed(Some("b"), false, Some("c"), id(2));
    check.landed(Some("c"), false, Some("d"), id(3));
    assert_eq!(
        check.close(Some("d"), id(4)),
        Ok(vec![Effect::CloseRed {
            check_id: id(1),
            cause: RedCloseCause::Closed
        }])
    );
    assert!(check.dirty());
    assert_eq!(
        check.ended(id(2), Verdict::Passed, Some("d"), id(5)),
        vec![Effect::Start(range(5, "c", "d"))]
    );
}

#[test]
fn interrupting_an_older_run_does_not_supersede_the_next_check() {
    let mut check = red_a_to_b();
    check.landed(Some("b"), false, Some("c"), id(2));
    check.landed(Some("c"), true, Some("d"), id(3));
    check.take_interrupted();
    assert_eq!(check.superseded_by_fix(None), None);
    check.landed(Some("d"), false, Some("e"), id(4));
    check.ended(id(4), failed(8), Some("e"), id(5));
    assert_eq!(check.red().unwrap().range.check_id, id(4));
}

#[test]
fn a_late_fix_landing_does_not_hide_failure_of_current_main() {
    let mut check = red_a_to_b();
    check.landed(Some("b"), false, Some("d"), id(2));
    check.landed(Some("c"), true, Some("d"), id(3));
    check.ended(id(2), failed(8), Some("d"), id(4));
    assert_eq!(check.red().unwrap().range.check_id, id(2));
    assert_eq!(check.red_notice_id(), Some(id(2)));
}
