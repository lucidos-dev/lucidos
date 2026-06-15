mod tool_turn_text_tests {
    use super::super::suppress_tool_turn_text;
    use crate::llm::provider::{LlmResponse, ToolCall};

    fn response(content: Option<&str>, tool_calls: Vec<ToolCall>) -> LlmResponse {
        LlmResponse {
            content: content.map(str::to_string),
            tool_calls,
            stop_reason: Some("STOP".to_string()),
            output_tokens: None,
            input_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            thinking_chars: None,
            unknown_sse_dropped: 0,
        }
    }

    fn tool_call() -> ToolCall {
        ToolCall {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "notes.md"}),
            thought_signature: None,
        }
    }

    #[test]
    fn suppresses_text_attached_to_tool_call_turns() {
        let mut resp = response(
            Some("No introductory transition, no \"Okay, I see\". Just act."),
            vec![tool_call()],
        );

        assert!(suppress_tool_turn_text(&mut resp));
        assert!(resp.content.is_none());
        assert_eq!(resp.tool_calls.len(), 1);
    }

    #[test]
    fn leaves_final_answer_text_without_tool_calls() {
        let mut resp = response(Some("Done."), vec![]);

        assert!(!suppress_tool_turn_text(&mut resp));
        assert_eq!(resp.content.as_deref(), Some("Done."));
    }

    #[test]
    fn removes_whitespace_only_text_on_tool_call_turns_without_reporting_visible_text() {
        let mut resp = response(Some("   \n\t"), vec![tool_call()]);

        assert!(!suppress_tool_turn_text(&mut resp));
        assert!(resp.content.is_none());
    }
}
