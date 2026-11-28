mod failure_breaker_tests {
    use super::super::{generic_breaker_action, next_failure_streak, BreakerAction};

    fn run_same_key(outcomes: &[bool]) -> Vec<BreakerAction> {
        let mut streak = 0usize;
        let mut last_call_was_error = false;
        let mut actions = Vec::new();
        for (i, &failed) in outcomes.iter().enumerate() {
            let is_repeat = i > 0;
            streak = next_failure_streak(is_repeat, last_call_was_error, streak);
            let action = generic_breaker_action(streak);
            actions.push(action);
            if action == BreakerAction::None {
                last_call_was_error = failed;
            }
        }
        actions
    }

    #[test]
    fn successful_repetition_never_trips() {
        let actions = run_same_key(&[false; 10]);
        assert!(actions.iter().all(|a| *a == BreakerAction::None));
    }

    #[test]
    fn three_consecutive_failures_warn() {
        let actions = run_same_key(&[true, true, true]);
        assert_eq!(
            actions,
            vec![
                BreakerAction::None,
                BreakerAction::None,
                BreakerAction::Warn
            ]
        );
    }

    #[test]
    fn five_consecutive_failures_break() {
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
        let with_success = run_same_key(&[true, false, true, true]);
        assert!(
            with_success.iter().all(|a| *a == BreakerAction::None),
            "a mid-streak success (that executes) must reset the failure streak: {with_success:?}"
        );

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
        assert_eq!(next_failure_streak(false, true, 2), 1);
        assert_eq!(generic_breaker_action(1), BreakerAction::None);
    }

    #[test]
    fn streak_grows_only_on_failing_repeat() {
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
    fn break_threshold_far_below_the_default_tool_call_cap() {
        assert!(5.0 < crate::core::prefs::MAX_TOOL_CALLS.default_number());
    }

    #[test]
    fn the_round_backstop_leaves_room_for_every_bounded_non_tool_path() {
        use super::super::{
            MAX_PROSE_QUESTION_NUDGE, MAX_QUESTION_REASK, MAX_TODO_WAKE_NUDGE, NON_TOOL_ROUND_SLACK,
        };
        const BREAKER_WARNINGS: usize = 3;
        const BOUNDED: usize =
            MAX_QUESTION_REASK + MAX_PROSE_QUESTION_NUDGE + MAX_TODO_WAKE_NUDGE + BREAKER_WARNINGS;
        const { assert!(NON_TOOL_ROUND_SLACK >= 10 * BOUNDED) };
        const { assert!(NON_TOOL_ROUND_SLACK <= 1_000) };
    }

    #[test]
    fn the_loop_ends_a_turn_at_the_round_backstop() {
        let run = include_str!("../agentic_loop/run.rs");
        assert!(run.contains("rounds > max_tool_calls.saturating_add(NON_TOOL_ROUND_SLACK)"));
        assert!(run.contains("Some(round_backstop_message(rounds))"));
    }
}
