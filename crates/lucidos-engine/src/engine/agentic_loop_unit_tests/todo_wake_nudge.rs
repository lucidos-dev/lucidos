mod todo_wake_nudge_tests {
    use super::super::{
        should_nudge_unwatched_turn, todo_wake_nudge_instruction, MAX_TODO_WAKE_NUDGE,
    };

    #[test]
    fn nudges_a_turn_that_leaves_work_open_with_nothing_to_wake_it() {
        // The reported shape: the agent wrote "Wait for X", armed nothing, and
        // was about to end the turn saying it was watching.
        assert!(should_nudge_unwatched_turn(Some(1), false, 0));
        assert!(should_nudge_unwatched_turn(Some(9), false, 0));
    }

    #[test]
    fn a_covered_thread_is_never_nudged() {
        // A live event wait or an unfinished background task each re-open the
        // thread, so the agent parked rather than walked away. Telling it
        // nothing is watching would be the false alarm that makes the whole
        // gate untrustworthy.
        assert!(!should_nudge_unwatched_turn(Some(1), true, 0));
        assert!(!should_nudge_unwatched_turn(Some(50), true, 0));
    }

    #[test]
    fn a_turn_with_no_open_work_is_never_nudged() {
        // Zero covers every shape the probe collapses into it: a thread that
        // never wrote a list, an empty list, an all-completed list, and one
        // already settled to `Abandoned`. All four are ordinary turns that must
        // not pay an extra round.
        assert!(!should_nudge_unwatched_turn(Some(0), false, 0));
        assert!(!should_nudge_unwatched_turn(Some(0), true, 0));
    }

    #[test]
    fn an_unreadable_probe_does_not_nudge() {
        // A probe that could not run is UNKNOWN, never "open"
        // (`.claude/rules/rust.md`). Nudging on unknown would let one database
        // blip add a round to every chat turn. The settle asks the same
        // question again moments later, at the terminator.
        assert!(!should_nudge_unwatched_turn(None, false, 0));
        assert!(!should_nudge_unwatched_turn(None, true, 0));
    }

    #[test]
    fn the_gate_fires_at_most_once_per_turn() {
        // The bound is what keeps a non-complying model from trapping the loop.
        // At the cap the turn finalizes and the settle records `Abandoned`,
        // which is exactly the behaviour that predates this gate.
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
        // Same sanity the re-ask guard pins, and for the same reason: what must
        // not drift is the unconfigured relationship to the outer backstop.
        const { assert!(MAX_TODO_WAKE_NUDGE >= 1) };
        const { assert!(MAX_TODO_WAKE_NUDGE < crate::core::DEFAULT_MAX_TOOL_CALLS) };
    }

    #[test]
    fn the_instruction_offers_three_equal_options_and_states_the_count() {
        let text = todo_wake_nudge_instruction(3, false);

        assert!(text.contains('3'), "states the open count: {text}");
        // The three ways out, each nameable. A gate that only said "you are not
        // watching" would leave the model to guess which of them is wanted.
        assert!(text.contains("await_event"), "offers arming: {text}");
        assert!(text.contains("todo_write"), "offers settling: {text}");
        assert!(
            text.contains("hand back to the user"),
            "offers handing back: {text}"
        );
        // The claim it exists to stop.
        assert!(
            text.contains("do not tell the user you are watching"),
            "forbids the unbacked claim: {text}"
        );
    }

    /// The mode withdraws `todo_write` whole, so the nudge may not name it.
    /// An offered action the model cannot take is a round spent on a refusal.
    #[test]
    fn under_the_mode_the_nudge_names_the_heading_and_not_the_tool() {
        let text = todo_wake_nudge_instruction(2, true);

        assert!(
            !text.contains("todo_write"),
            "names a withdrawn tool: {text}"
        );
        assert!(text.contains("[TODO]"), "names the heading: {text}");
        // The other two ways out are unchanged: neither depends on the mode.
        assert!(text.contains("await_event"), "still offers arming: {text}");
        assert!(
            text.contains("hand back to the user"),
            "still offers handing back: {text}"
        );
    }

    #[test]
    fn the_instruction_hands_over_no_ready_made_user_facing_sentence() {
        // The failure mode `APPLY_VERIFY_DEV_ADDENDUM` had: a quotable sentence
        // in the prompt gets repeated at the user as a finding, with the work
        // behind it never done. Option 3 therefore says to decide the wording,
        // and supplies none.
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
