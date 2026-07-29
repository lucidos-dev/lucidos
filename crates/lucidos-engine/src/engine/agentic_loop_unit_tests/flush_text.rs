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
}
