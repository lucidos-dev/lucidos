mod flush_text_tests {
    use super::super::effective_flush_text;

    #[test]
    fn flush_uses_streamed_buffer_when_present() {
        assert_eq!(
            effective_flush_text(None, "streamed answer", Some("full content")),
            "streamed answer"
        );
    }

    #[test]
    fn flush_falls_back_to_content_when_buffer_empty() {
        assert_eq!(
            effective_flush_text(None, "", Some("the answer")),
            "the answer"
        );
    }

    #[test]
    fn flush_prefers_cleaned_inline_repair_text_over_buffer() {
        assert_eq!(
            effective_flush_text(Some("clean"), "<ask_user_question>…", Some("x")),
            "clean"
        );
    }

    #[test]
    fn flush_is_empty_when_nothing_available() {
        assert_eq!(effective_flush_text(None, "", None), "");
    }

    #[test]
    fn flush_prefers_the_spliced_text_over_the_buffer() {
        assert_eq!(
            effective_flush_text(
                Some("Reading the file now."),
                "Reading the file now.\n[WORKING UNDERSTANDING]\nprivate\n[/WORKING UNDERSTANDING]",
                Some("Reading the file now."),
            ),
            "Reading the file now."
        );
    }

    #[test]
    fn a_reply_that_was_only_the_document_flushes_nothing() {
        assert_eq!(
            effective_flush_text(
                Some(""),
                "[WORKING UNDERSTANDING]\nprivate\n[/WORKING UNDERSTANDING]",
                None,
            ),
            ""
        );
    }
}

mod reply_text_tests {
    use crate::engine::chat::process::working_understanding as wu;
    use crate::llm::provider::LlmResponse;

    const SPAN: &str = "[WORKING UNDERSTANDING]\nnotes\n[/WORKING UNDERSTANDING]";

    fn reply(content: Option<&str>, model_only: Option<&str>) -> LlmResponse {
        LlmResponse {
            content: content.map(str::to_string),
            tool_calls: vec![],
            stop_reason: None,
            usage: None,
            stable_prefix_fingerprint: None,
            served_model: None,
            provider: None,
            thinking_chars: None,
            thinking_blocks: None,
            unknown_sse_dropped: 0,
            model_only_text: model_only.map(str::to_string),
            progress_notes: Vec::new(),
            prefill: None,
        }
    }

    fn parse(response: &LlmResponse) -> wu::ParsedReply {
        wu::parse_message(wu::ASSISTANT_ROLE, response.history_text().unwrap_or(""))
    }

    #[test]
    fn a_printable_reply_carries_its_document() {
        assert!(parse(&reply(Some(SPAN), None)).wrote_something());
    }

    #[test]
    fn a_model_only_reply_carries_its_document_too() {
        let gemini = reply(None, Some(SPAN));
        assert!(
            gemini.content.is_none(),
            "the narration stays off the screen"
        );
        assert!(parse(&gemini).wrote_something());
    }

    #[test]
    fn a_reply_with_no_text_at_all_writes_nothing() {
        assert!(!parse(&reply(None, None)).wrote_something());
    }
}

mod bookkeeping_alone_tests {
    use super::super::reply_was_bookkeeping_alone;

    #[test]
    fn a_document_and_nothing_else_is_bookkeeping() {
        assert!(reply_was_bookkeeping_alone(true, None));
        assert!(reply_was_bookkeeping_alone(true, Some("")));
    }

    #[test]
    fn a_document_beside_an_answer_is_an_answer() {
        assert!(!reply_was_bookkeeping_alone(
            true,
            Some("here is the result")
        ));
    }

    #[test]
    fn an_empty_reply_that_wrote_nothing_is_not_bookkeeping() {
        assert!(!reply_was_bookkeeping_alone(false, None));
        assert!(!reply_was_bookkeeping_alone(false, Some("")));
    }

    #[test]
    fn the_loop_pushes_the_carry_on_as_a_user_message() {
        let run = include_str!("../agentic_loop/run.rs");
        let at = run
            .find("MessageContent::Text(NOTED_CARRY_ON.to_string())")
            .expect("the loop pushes NOTED_CARRY_ON");
        let literal = &run[run[..at].rfind("Message {").unwrap()..at];
        assert!(literal.contains("role: \"user\".to_string()"), "{literal}");
    }
}
