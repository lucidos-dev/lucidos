mod failure_breaker_tests {
    use super::super::{generic_breaker_action, next_failure_streak, BreakerAction, MAX_ITERATIONS};

    /// Simulate a run of single-tool-call iterations on the SAME (tool, key),
    /// returning the breaker action observed on each iteration BEFORE the
    /// proposed call runs. `outcomes[i]` is whether iteration i's call failed.
    /// Mirrors the loop wiring: the streak uses the PREVIOUS call's error, the
    /// action is read from the new streak, and on a Warn/Break the proposed call
    /// is skipped (so `last_call_was_error` is NOT updated that iteration).
    fn run_same_key(outcomes: &[bool]) -> Vec<BreakerAction> {
        let mut streak = 0usize;
        let mut last_call_was_error = false;
        let mut actions = Vec::new();
        for (i, &failed) in outcomes.iter().enumerate() {
            let is_repeat = i > 0; // same key every iteration after the first
            streak = next_failure_streak(is_repeat, last_call_was_error, streak);
            let action = generic_breaker_action(streak);
            actions.push(action);
            // Warn/Break skip executing the proposed call → no result recorded.
            if action == BreakerAction::None {
                last_call_was_error = failed;
            }
        }
        actions
    }

    #[test]
    fn successful_repetition_never_trips() {
        // Ten productive identical calls in a row (each succeeds) — e.g. ten
        // `psql` queries bucketed under `psql`. The breaker must stay silent.
        let actions = run_same_key(&[false; 10]);
        assert!(actions.iter().all(|a| *a == BreakerAction::None));
    }

    #[test]
    fn three_consecutive_failures_warn() {
        // fail, fail, fail → the 3rd iteration warns (2 prior failures + repeat).
        let actions = run_same_key(&[true, true, true]);
        assert_eq!(
            actions,
            vec![BreakerAction::None, BreakerAction::None, BreakerAction::Warn]
        );
    }

    #[test]
    fn five_consecutive_failures_break() {
        // The 3rd & 4th warn (proposed call skipped, streak preserved), the 5th
        // hard-breaks. The model "ignored" the warnings by retrying.
        let actions = run_same_key(&[true, true, true, true, true]);
        assert_eq!(
            actions,
            vec![
                BreakerAction::None,
                BreakerAction::None,
                BreakerAction::Warn,
                BreakerAction::Warn,
                BreakerAction::Break,
            ]
        );
    }

    #[test]
    fn success_midstreak_resets() {
        // A success that actually executes (streak still below the warn
        // threshold) breaks the failure streak. fail, SUCCESS, fail, fail: the
        // success at the 2nd call resets, so the run never exceeds 2 consecutive
        // failures and never warns...
        let with_success = run_same_key(&[true, false, true, true]);
        assert!(
            with_success.iter().all(|a| *a == BreakerAction::None),
            "a mid-streak success (that executes) must reset the failure streak: {with_success:?}"
        );

        // ...whereas the SAME four attempts all failing warn on the 3rd. The
        // single success is the only difference — proving the reset is real.
        let all_fail = run_same_key(&[true, true, true, true]);
        assert_eq!(
            all_fail,
            vec![
                BreakerAction::None,
                BreakerAction::None,
                BreakerAction::Warn,
                BreakerAction::Warn,
            ]
        );
    }

    #[test]
    fn non_repeat_resets_streak() {
        // Two prior failures then a DIFFERENT call: not a repeat → streak resets
        // to 1, so no action even though the previous call failed.
        assert_eq!(next_failure_streak(false, true, 2), 1);
        assert_eq!(generic_breaker_action(1), BreakerAction::None);
    }

    #[test]
    fn streak_grows_only_on_failing_repeat() {
        // Repeat after a failure grows; repeat after a success resets to 1.
        assert_eq!(next_failure_streak(true, true, 4), 5);
        assert_eq!(next_failure_streak(true, false, 4), 1);
        assert_eq!(next_failure_streak(false, true, 4), 1);
        assert_eq!(next_failure_streak(false, false, 4), 1);
    }

    #[test]
    fn action_thresholds_at_boundaries() {
        assert_eq!(generic_breaker_action(0), BreakerAction::None);
        assert_eq!(generic_breaker_action(2), BreakerAction::None);
        assert_eq!(generic_breaker_action(3), BreakerAction::Warn);
        assert_eq!(generic_breaker_action(4), BreakerAction::Warn);
        assert_eq!(generic_breaker_action(5), BreakerAction::Break);
        assert_eq!(generic_breaker_action(100), BreakerAction::Break);
    }

    #[test]
    fn break_threshold_far_below_iteration_cap() {
        // The breaker fires long before the outer MAX_ITERATIONS backstop.
        const { assert!(5 < MAX_ITERATIONS) };
    }
}
