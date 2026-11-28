mod question_card_tests {
    use super::super::{instruction_after_round, TOOL_RESULTS_INSTRUCTION};

    #[test]
    fn the_tool_result_instruction_never_claims_the_user_saw_the_results() {
        assert!(!TOOL_RESULTS_INSTRUCTION.contains("already read"));
        assert!(TOOL_RESULTS_INSTRUCTION.contains("never sees tool"));
    }

    #[test]
    fn the_loop_asks_the_shared_rule_before_raising_a_card() {
        let run = include_str!("../agentic_loop/run.rs");
        for needle in [
            "tn::ASK_USER_QUESTION && human_can_answer",
            "question_card_gate::refuse_card(",
            "text: &round_text,",
            "progress_notes: &response.progress_notes,",
            "refusal.text()",
            "agent_question::card_message(&tool_call.arguments),",
        ] {
            assert!(run.contains(needle), "run.rs must contain `{needle}`");
        }
    }

    #[test]
    fn a_refused_card_never_reaches_the_walk_that_shows_its_message() {
        let run = include_str!("../agentic_loop/run.rs");
        let gate = run.find("let card_refusal =").unwrap();
        let after_gate = &run[gate..];
        let refused = after_gate
            .find("if let Some(refusal) = card_refusal {")
            .unwrap();
        let walk = after_gate.find(".handle_special_tool(").unwrap();
        assert!(refused < walk);
        let refused_branch = &after_gate[refused..walk];
        assert!(refused_branch.contains("continue;"));
    }

    #[test]
    fn a_quiet_answer_ends_the_turn_before_the_next_model_call() {
        let run = include_str!("../agentic_loop/run.rs");
        assert!(run.contains("quiet_answer |= asked.quiet;"));
        let quiet = run
            .find("if quiet_answer && injection_rx.is_empty() {")
            .unwrap();
        let next_round = run.find("let mut assistant_blocks = Vec::new();").unwrap();
        assert!(quiet < next_round);
        let branch = &run[quiet..next_round];
        assert!(branch.contains("ResponseGenerated"));
        assert!(branch.contains("return Ok(terminal_result("));
    }

    #[test]
    fn a_quiet_answer_ends_the_turn_on_the_quiet_reply() {
        let run = include_str!("../agentic_loop/run.rs");
        let quiet = run
            .find("if quiet_answer && injection_rx.is_empty() {\n                self.gate_chat_turn_end(")
            .unwrap();
        let next_round = run.find("let mut assistant_blocks = Vec::new();").unwrap();
        let branch = &run[quiet..next_round];
        assert!(branch.contains("&quiet_reply,"));
        assert!(branch.contains("text: quiet_reply.clone(),"));
        assert!(branch.contains("terminal_result(\n                    quiet_reply,"));
    }

    const NOTE: &str = "I've drafted the refund message and the two links it needs.";

    #[test]
    fn a_progress_note_round_tells_the_model_what_the_user_saw() {
        let text = instruction_after_round(TOOL_RESULTS_INSTRUCTION, &[NOTE.to_string()], false);
        assert!(text.starts_with("Part of what you wrote before these tool calls"));
        assert!(text.contains(&format!("\"{NOTE}\"")));
        assert!(text.contains("The full text behind it never reached them"));
        assert!(text.ends_with(TOOL_RESULTS_INSTRUCTION));
    }

    #[test]
    fn a_round_without_a_progress_note_keeps_the_instruction_unchanged() {
        assert_eq!(
            instruction_after_round(TOOL_RESULTS_INSTRUCTION, &[], false),
            TOOL_RESULTS_INSTRUCTION
        );
    }

    #[test]
    fn a_card_round_leaves_the_note_to_the_card_gate() {
        assert_eq!(
            instruction_after_round(TOOL_RESULTS_INSTRUCTION, &[NOTE.to_string()], true),
            TOOL_RESULTS_INSTRUCTION
        );
    }

    #[test]
    fn the_loop_puts_the_note_line_in_the_tool_result_message() {
        let run = include_str!("../agentic_loop/run.rs");
        let line = run
            .find("let instruction = instruction_after_round(")
            .unwrap();
        let build = run
            .find("build_tool_result_blocks(&tool_outputs, &instruction)")
            .unwrap();
        assert!(line < build);
    }
}
