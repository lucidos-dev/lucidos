mod turn_end_decisions_tests {
    use super::super::calls_only_turn_end_decisions;
    use crate::llm::provider::ToolCall;

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: format!("toolu_{name}"),
            name: name.to_string(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        }
    }

    /// Only a round whose calls are all turn-end decisions can end the turn on
    /// its text. Any other call still needs its result read.
    #[test]
    fn only_a_decision_only_round_qualifies() {
        assert!(calls_only_turn_end_decisions(&[call("request_read")]));
        assert!(calls_only_turn_end_decisions(&[call("wake_parent")]));
        assert!(calls_only_turn_end_decisions(&[
            call("request_read"),
            call("wake_parent")
        ]));
        assert!(!calls_only_turn_end_decisions(&[
            call("wake_parent"),
            call("write_file")
        ]));
        assert!(!calls_only_turn_end_decisions(&[]));
        assert!(!calls_only_turn_end_decisions(&[
            call("request_read"),
            call("write_file")
        ]));
        assert!(!calls_only_turn_end_decisions(&[call("write_file")]));
    }
}
