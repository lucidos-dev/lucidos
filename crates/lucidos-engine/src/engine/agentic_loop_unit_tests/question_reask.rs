mod question_reask_tests {
    use super::super::{should_force_question_reask, MAX_QUESTION_REASK};

    #[test]
    fn no_force_when_last_ask_succeeded() {
        // The previous iteration had no failed ask (success, or no ask at all):
        // the model's prose answer is legitimate, never force a re-ask.
        assert!(!should_force_question_reask(false, 0));
        assert!(!should_force_question_reask(false, MAX_QUESTION_REASK - 1));
    }

    #[test]
    fn forces_while_budget_remains() {
        // A failed ask followed by prose forces a re-ask, for each force up to
        // the cap.
        for forced in 0..MAX_QUESTION_REASK {
            assert!(
                should_force_question_reask(true, forced),
                "should force at forced={forced}"
            );
        }
    }

    #[test]
    fn stops_forcing_at_cap() {
        // Once the per-response budget is spent, fall through to a normal prose
        // finalization so a stuck question path can't trap the loop.
        assert!(!should_force_question_reask(true, MAX_QUESTION_REASK));
        assert!(!should_force_question_reask(true, MAX_QUESTION_REASK + 5));
    }

    #[test]
    fn cap_is_bounded_and_below_iteration_cap() {
        // Sanity: the re-ask cap is a small positive bound, far under the outer
        // MAX_ITERATIONS backstop. Both operands are consts, so assert at compile time.
        const { assert!(MAX_QUESTION_REASK >= 1) };
        const { assert!(MAX_QUESTION_REASK < super::super::MAX_ITERATIONS) };
    }
}
