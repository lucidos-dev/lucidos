mod flush_text_tests {
    use super::super::effective_flush_text;

    #[test]
    fn flush_uses_streamed_buffer_when_present() {
        // Claude path: tokens streamed into `raw`. Use it verbatim — this is
        // also the assistant's preamble on a tool-call turn ("Let me do X…").
        assert_eq!(
            effective_flush_text(None, "streamed answer", Some("full content")),
            "streamed answer"
        );
    }

    #[test]
    fn flush_falls_back_to_content_when_buffer_empty() {
        // Defensive: a provider that left `raw` empty but carried prose in the
        // response body still flushes that prose.
        assert_eq!(
            effective_flush_text(None, "", Some("the answer")),
            "the answer"
        );
    }

    #[test]
    fn flush_prefers_cleaned_inline_repair_text_over_buffer() {
        // Inline-question-repair: the tag-stripped text wins regardless of the
        // raw buffer (which may still hold the un-stripped tag).
        assert_eq!(
            effective_flush_text(Some("clean"), "<ask_user_question>…", Some("x")),
            "clean"
        );
    }

    #[test]
    fn flush_is_empty_when_nothing_available() {
        // Model went straight to a tool call with no prose at all.
        assert_eq!(effective_flush_text(None, "", None), "");
    }

    /// Invariant 31. The raw buffer still holds the working understanding: the
    /// streaming callback appends before it suppresses. So the spliced text has
    /// to win, or the user watches the model type its private notes at them.
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

    /// The same, for a reply that was ONLY the document. Nothing reaches the
    /// user, and the loop reads that as bookkeeping rather than an answer.
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

/// Invariant 9, at the provider boundary. A write can never be silently lost.
mod reply_text_tests {
    use crate::engine::chat::process::working_understanding as wu;
    use crate::llm::provider::LlmResponse;

    const SPAN: &str = "[WORKING UNDERSTANDING]\nnotes\n[/WORKING UNDERSTANDING]";

    fn reply(content: Option<&str>, model_only: Option<&str>) -> LlmResponse {
        LlmResponse {
            content: content.map(str::to_string),
            tool_calls: vec![],
            stop_reason: None,
            output_tokens: None,
            input_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            thinking_chars: None,
            unknown_sse_dropped: 0,
            model_only_text: model_only.map(str::to_string),
        }
    }

    /// What the loop does: parse `history_text`, never `content`.
    fn parse(response: &LlmResponse) -> wu::ParsedReply {
        wu::parse_message(wu::ASSISTANT_ROLE, response.history_text().unwrap_or(""))
    }

    /// Anthropic and OpenAI keep their text in `content`.
    #[test]
    fn a_printable_reply_carries_its_document() {
        assert!(parse(&reply(Some(SPAN), None)).wrote_something());
    }

    /// Gemini narrates its plan beside a `functionCall` and keeps it off the
    /// screen by leaving `content` empty. Parsing `content` would ignore every
    /// document and every keep it writes on a tool-call round.
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

/// Invariant 32. A reply carrying only the document never ends the turn.
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

    /// An empty reply with no document is the empty-completion case, which the
    /// classifier below the branch diagnoses. Reading it as bookkeeping would
    /// spin the turn on nothing.
    #[test]
    fn an_empty_reply_that_wrote_nothing_is_not_bookkeeping() {
        assert!(!reply_was_bookkeeping_alone(false, None));
        assert!(!reply_was_bookkeeping_alone(false, Some("")));
    }
}
