mod read_decision_tests {
    use super::super::calls_only_the_read_decision;
    use crate::llm::provider::ToolCall;

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: format!("toolu_{name}"),
            name: name.to_string(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        }
    }

    /// Only a round whose calls are all the read decision can end the turn on
    /// its text. Any other call still needs its result read.
    #[test]
    fn only_a_decision_only_round_qualifies() {
        assert!(calls_only_the_read_decision(&[call("request_read")]));
        assert!(!calls_only_the_read_decision(&[]));
        assert!(!calls_only_the_read_decision(&[
            call("request_read"),
            call("write_file")
        ]));
        assert!(!calls_only_the_read_decision(&[call("write_file")]));
    }
}
