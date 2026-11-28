mod todo_wake_nudge_tests {
    use super::super::{
        should_nudge_unwatched_turn, todo_wake_nudge_instruction, MAX_TODO_WAKE_NUDGE,
    };

    #[test]
    fn nudges_a_turn_that_leaves_work_open_with_nothing_to_wake_it() {
        assert!(should_nudge_unwatched_turn(Some(1), false, 0));
        assert!(should_nudge_unwatched_turn(Some(9), false, 0));
    }

    #[test]
    fn a_covered_thread_is_never_nudged() {
        assert!(!should_nudge_unwatched_turn(Some(1), true, 0));
        assert!(!should_nudge_unwatched_turn(Some(50), true, 0));
    }

    #[test]
    fn a_turn_with_no_open_work_is_never_nudged() {
        assert!(!should_nudge_unwatched_turn(Some(0), false, 0));
        assert!(!should_nudge_unwatched_turn(Some(0), true, 0));
    }

    #[test]
    fn an_unreadable_probe_does_not_nudge() {
        assert!(!should_nudge_unwatched_turn(None, false, 0));
        assert!(!should_nudge_unwatched_turn(None, true, 0));
    }

    #[test]
    fn the_gate_fires_at_most_once_per_turn() {
        for forced in 0..MAX_TODO_WAKE_NUDGE {
            assert!(
                should_nudge_unwatched_turn(Some(1), false, forced),
                "should nudge at forced={forced}"
            );
        }
        assert!(!should_nudge_unwatched_turn(
            Some(1),
            false,
            MAX_TODO_WAKE_NUDGE
        ));
        assert!(!should_nudge_unwatched_turn(
            Some(1),
            false,
            MAX_TODO_WAKE_NUDGE + 5
        ));
    }

    #[test]
    fn the_cap_is_bounded_and_below_the_default_tool_call_cap() {
        const { assert!(MAX_TODO_WAKE_NUDGE >= 1) };
        assert!((MAX_TODO_WAKE_NUDGE as f64) < crate::core::prefs::MAX_TOOL_CALLS.default_number());
    }

    #[test]
    fn the_instruction_offers_three_equal_options_and_states_the_count() {
        let text = todo_wake_nudge_instruction(3, false);

        assert!(text.contains('3'), "states the open count: {text}");
        assert!(text.contains("await_event"), "offers arming: {text}");
        assert!(text.contains("todo_write"), "offers settling: {text}");
        assert!(
            text.contains("hand back to the user"),
            "offers handing back: {text}"
        );
        assert!(
            text.contains("do not tell the user you are watching"),
            "forbids the unbacked claim: {text}"
        );
    }

    #[test]
    fn the_instruction_names_child_threads_among_what_re_opens_a_thread() {
        let text = todo_wake_nudge_instruction(1, false);
        assert!(text.contains("child thread"), "names child threads: {text}");
    }

    #[test]
    fn under_the_mode_the_nudge_names_the_heading_and_not_the_tool() {
        let text = todo_wake_nudge_instruction(2, true);

        assert!(
            !text.contains("todo_write"),
            "names a withdrawn tool: {text}"
        );
        assert!(text.contains("[TODO]"), "names the heading: {text}");
        assert!(text.contains("await_event"), "still offers arming: {text}");
        assert!(
            text.contains("hand back to the user"),
            "still offers handing back: {text}"
        );
    }

    #[test]
    fn the_instruction_hands_over_no_ready_made_user_facing_sentence() {
        let text = todo_wake_nudge_instruction(1, false);

        assert!(
            text.contains("deciding for yourself how to say"),
            "leaves the wording to the model: {text}"
        );
        assert!(
            !text.contains("Tell the user \""),
            "supplies no quotable sentence: {text}"
        );
        assert!(
            !text.contains("I am not watching"),
            "supplies no quotable sentence: {text}"
        );
    }
}
