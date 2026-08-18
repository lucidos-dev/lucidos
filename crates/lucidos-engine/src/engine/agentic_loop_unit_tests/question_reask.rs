mod question_reask_tests {
    use super::super::{question_reask_cause, QuestionReaskCause, MAX_QUESTION_REASK};

    #[test]
    fn no_force_when_neither_cause_holds() {
        // No failed ask and no leaked tag: the model's prose answer is
        // legitimate, so never force a re-ask.
        assert_eq!(question_reask_cause(false, false, 0), None);
        assert_eq!(
            question_reask_cause(false, false, MAX_QUESTION_REASK - 1),
            None
        );
    }

    #[test]
    fn forces_while_budget_remains() {
        // Either cause forces a re-ask, for each force up to the cap.
        for forced in 0..MAX_QUESTION_REASK {
            assert_eq!(
                question_reask_cause(true, false, forced),
                Some(QuestionReaskCause::CallRejected),
                "a rejected call should force at forced={forced}"
            );
            assert_eq!(
                question_reask_cause(false, true, forced),
                Some(QuestionReaskCause::LeakedAsText),
                "a leaked tag should force at forced={forced}"
            );
        }
    }

    #[test]
    fn a_rejected_call_wins_over_a_leaked_tag() {
        // The more specific diagnosis: the model did reach the tool, so tell it
        // what the call got wrong rather than that it typed a tag.
        assert_eq!(
            question_reask_cause(true, true, 0),
            Some(QuestionReaskCause::CallRejected)
        );
    }

    #[test]
    fn stops_forcing_at_cap() {
        // Once the per-response budget is spent, fall through to a normal prose
        // finalization so a stuck question path can't trap the loop.
        assert_eq!(question_reask_cause(true, false, MAX_QUESTION_REASK), None);
        assert_eq!(question_reask_cause(false, true, MAX_QUESTION_REASK), None);
        assert_eq!(
            question_reask_cause(true, true, MAX_QUESTION_REASK + 5),
            None
        );
    }

    #[test]
    fn the_budget_is_shared_so_the_causes_cannot_alternate_past_the_cap() {
        // One budget, not one per cause. Otherwise a model that alternates
        // between a bad call and a typed tag would loop twice as long.
        let mut forced = 0;
        while question_reask_cause(forced % 2 == 0, forced % 2 == 1, forced).is_some() {
            forced += 1;
        }
        assert_eq!(forced, MAX_QUESTION_REASK);
    }

    #[test]
    fn each_cause_names_its_own_problem() {
        // Telling the model a call was rejected when it never made one sends it
        // looking for a mistake that isn't there.
        let rejected = QuestionReaskCause::CallRejected.instruction();
        let leaked = QuestionReaskCause::LeakedAsText.instruction();
        assert!(rejected.contains("rejected because a question object"));
        assert!(!rejected.contains("`<ask_user_question>` tag"));
        assert!(leaked.contains("typed an `<ask_user_question>` tag"));
        assert!(!leaked.contains("rejected"));
        for text in [rejected, leaked] {
            assert!(
                text.contains("ask_user_question"),
                "every instruction must name the tool to re-call"
            );
        }
    }

    #[test]
    fn cap_is_bounded_and_below_the_default_tool_call_cap() {
        // Sanity: the re-ask cap is a small positive bound, far under the outer
        // tool-call backstop. Both operands are consts, so assert at compile time.
        //
        // This guards the DEFAULT cap, not the configured one: the tool-call cap
        // is a user setting with no ceiling and a floor of 1, so a user who sets
        // it to 1 has deliberately chosen for the backstop to fire before this
        // guard. What must not drift is the unconfigured relationship.
        const { assert!(MAX_QUESTION_REASK >= 1) };
        const { assert!(MAX_QUESTION_REASK < crate::core::DEFAULT_MAX_TOOL_CALLS) };
    }
}
