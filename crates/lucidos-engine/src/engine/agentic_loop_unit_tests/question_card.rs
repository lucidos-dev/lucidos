mod question_card_tests {
    use super::super::TOOL_RESULTS_INSTRUCTION;

    #[test]
    fn the_tool_result_instruction_never_claims_the_user_saw_the_results() {
        // It said "the user already read it" after every tool round. The user
        // never sees a tool result, so the model took it as leave to skip the
        // report.
        assert!(!TOOL_RESULTS_INSTRUCTION.contains("already read"));
        assert!(TOOL_RESULTS_INSTRUCTION.contains("never sees tool"));
    }

    /// The rule lives in `question_card_gate`, but the loop decides when to
    /// ask it. Pin the three conditions and the non-error refusal.
    #[test]
    fn the_loop_asks_the_shared_rule_before_raising_a_card() {
        let run = include_str!("../agentic_loop/run.rs");
        for needle in [
            "&& human_can_answer",
            "&& !spoke_this_round",
            "question_card_gate::refuse_card(",
            "question_card_gate::CARD_REFUSAL",
        ] {
            assert!(run.contains(needle), "run.rs must contain `{needle}`");
        }
    }
}
