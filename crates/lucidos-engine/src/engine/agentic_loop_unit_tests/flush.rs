mod should_flush_tests {
    use super::super::{is_bad_image_description, should_flush};

    // --- Paragraph breaks ---

    #[test]
    fn flushes_on_double_newline() {
        assert!(should_flush("Hello world\n\n"));
    }

    #[test]
    fn no_flush_on_single_newline() {
        assert!(!should_flush("Hello world\n"));
    }

    #[test]
    fn no_flush_mid_paragraph() {
        assert!(!should_flush("Hello world"));
    }

    #[test]
    fn flushes_on_multiple_paragraphs() {
        assert!(should_flush("First paragraph.\n\nSecond paragraph.\n\n"));
    }

    // --- Code fence close ---

    #[test]
    fn flushes_on_code_fence_close() {
        assert!(should_flush("```rust\nfn main() {}\n```\n"));
    }

    #[test]
    fn no_flush_on_code_fence_open() {
        assert!(!should_flush("```rust\n"));
    }

    #[test]
    fn no_flush_on_code_fence_without_trailing_newline() {
        assert!(!should_flush("```rust\ncode\n```"));
    }

    // --- Heading after newline ---

    #[test]
    fn flushes_on_heading_followed_by_newline() {
        assert!(should_flush("Some text\n## Heading\n"));
    }

    #[test]
    fn flushes_on_h1_followed_by_newline() {
        assert!(should_flush("Intro\n# Title\n"));
    }

    #[test]
    fn no_flush_on_heading_without_trailing_newline() {
        assert!(!should_flush("Some text\n## Heading"));
    }

    #[test]
    fn flushes_on_first_line_heading() {
        // A complete heading line should always flush
        assert!(should_flush("# Title\n"));
    }

    // --- Horizontal rules ---

    #[test]
    fn flushes_on_dash_horizontal_rule() {
        assert!(should_flush("Some text\n---\n"));
    }

    #[test]
    fn flushes_on_asterisk_horizontal_rule() {
        assert!(should_flush("Some text\n***\n"));
    }

    #[test]
    fn no_flush_on_partial_horizontal_rule() {
        assert!(!should_flush("Some text\n---"));
    }

    // --- Edge cases ---

    #[test]
    fn no_flush_on_empty_string() {
        assert!(!should_flush(""));
    }

    #[test]
    fn no_flush_on_whitespace_only() {
        assert!(!should_flush("   "));
    }

    #[test]
    fn no_flush_on_single_char() {
        assert!(!should_flush("a"));
    }

    #[test]
    fn flushes_long_text_ending_with_paragraph_break() {
        let mut text = "A".repeat(5000);
        text.push_str("\n\n");
        assert!(should_flush(&text));
    }

    #[test]
    fn no_flush_on_long_text_without_boundary() {
        let text = "A".repeat(5000);
        assert!(!should_flush(&text));
    }

    // --- Combinations ---

    #[test]
    fn flushes_code_block_then_paragraph() {
        assert!(should_flush("```\ncode\n```\n\nNext paragraph\n\n"));
    }

    #[test]
    fn flushes_heading_in_middle_of_text() {
        assert!(should_flush("First part\n## Section\n"));
    }

    // --- List items (should NOT flush) ---

    #[test]
    fn no_flush_on_list_item() {
        assert!(!should_flush("- item 1\n"));
    }

    #[test]
    fn no_flush_on_numbered_list() {
        assert!(!should_flush("1. item\n"));
    }

    // --- is_bad_image_description ---

    #[test]
    fn rejects_gemini_no_image_response() {
        assert!(is_bad_image_description(
            "Please provide the images you would like me to describe. I do not see any images attached to your message."
        ));
    }

    #[test]
    fn rejects_contraction_variant() {
        assert!(is_bad_image_description(
            "I don't see any images in the message."
        ));
    }

    #[test]
    fn rejects_no_image_provided() {
        assert!(is_bad_image_description(
            "No image was provided for analysis."
        ));
    }

    #[test]
    fn accepts_valid_description() {
        assert!(!is_bad_image_description(
            "A screenshot of a calendar invitation showing a meeting titled 'Standup' on March 17, 2026 at 09:00-09:15."
        ));
    }

    #[test]
    fn accepts_ocr_description() {
        assert!(!is_bad_image_description(
            "The image shows a document with the text: 'Møte med Alex, 14. mars kl 10:00-11:00'"
        ));
    }
}

mod empty_completion_hint_tests {
    use super::super::empty_completion_hint;

    // Background: when the LLM call returns end_turn with no text and no
    // tool calls, the engine emits ResponseFailed with a hint. The original
    // hint hard-coded "model decided no action was needed" for end_turn,
    // which is a lie when output_tokens > 0 — the model DID generate, the
    // parser just didn't recognize the SSE shape. These tests pin down the
    // four diagnostic branches so the next provider-stream change shows up
    // as a precise message instead of a misleading one.

    #[test]
    fn truly_silent_end_turn_says_no_action_needed() {
        // output_tokens <= 16 = pure structural overhead, nothing generated.
        let hint = empty_completion_hint("end_turn", 5, 0, 0);
        assert!(
            hint.contains("no action was needed"),
            "intentional silence must say 'no action was needed', got: {}",
            hint
        );
    }

    #[test]
    fn large_output_with_unknown_drops_blames_parser() {
        // The failure mode this fix targets: 2222 output tokens, nothing
        // captured, and the SSE accumulator flagged unknown shapes — the
        // hint must say so instead of claiming the model intended silence.
        let hint = empty_completion_hint("end_turn", 2222, 0, 3);
        assert!(
            hint.contains("dropped unknown SSE shapes"),
            "parser miss with dropped shapes must call them out, got: {}",
            hint
        );
        assert!(
            !hint.contains("no action was needed"),
            "must NOT claim intentional silence, got: {}",
            hint
        );
    }

    #[test]
    fn large_output_without_unknown_drops_still_blames_parser() {
        // Tokens generated but nothing captured AND no unknowns flagged means
        // a known block type carried unexpected payload shape. Still a parser
        // miss, not intentional silence.
        let hint = empty_completion_hint("end_turn", 2222, 0, 0);
        assert!(
            hint.contains("couldn't classify"),
            "parser miss without dropped shapes must still call out the gap, got: {}",
            hint
        );
        assert!(
            !hint.contains("no action was needed"),
            "must NOT claim intentional silence, got: {}",
            hint
        );
    }

    #[test]
    fn thinking_only_says_thought_but_no_output() {
        // Model thought (visibly, via thinking blocks) but produced no text
        // or tool call. Different failure than "silently dropped" — useful to
        // distinguish for debugging "is the model giving up?".
        let hint = empty_completion_hint("end_turn", 100, 4096, 0);
        assert!(
            hint.contains("thought but produced no text"),
            "thinking-only must mention the thought, got: {}",
            hint
        );
    }

    #[test]
    fn max_tokens_says_truncated() {
        let hint = empty_completion_hint("max_tokens", 8192, 0, 0);
        assert!(
            hint.contains("truncated"),
            "max_tokens must say truncated, got: {}",
            hint
        );
    }

    #[test]
    fn other_stop_reasons_have_no_hint() {
        let hint = empty_completion_hint("some_future_reason", 0, 0, 0);
        assert!(
            hint.is_empty(),
            "unknown stop_reason with no other signals must have no hint, got: {}",
            hint
        );
    }

    #[test]
    fn refusal_says_model_declined() {
        // Real-world payload from the "stop_reason: refusal" report: the
        // provider's streaming safety classifier stopped the turn and withheld
        // the content the model had begun generating, so output_tokens is
        // non-zero (207) while nothing reached the engine.
        let hint = empty_completion_hint("refusal", 207, 0, 0);
        assert!(
            hint.contains("declined"),
            "refusal must say the model declined, got: {}",
            hint
        );
        // Must NOT misattribute the empty content to a parser / provider
        // stream-shape change — that was the misleading message users saw.
        assert!(
            !hint.contains("couldn't classify"),
            "refusal must NOT blame the parser, got: {}",
            hint
        );
        assert!(
            !hint.contains("stream-shape"),
            "refusal must NOT blame a stream-shape change, got: {}",
            hint
        );
    }

    #[test]
    fn refusal_wins_even_with_zero_output_tokens() {
        // A refusal that withholds everything before any tokens are billed
        // must still be reported as a refusal, not as intentional silence.
        let hint = empty_completion_hint("refusal", 0, 0, 0);
        assert!(
            hint.contains("declined"),
            "refusal must say the model declined regardless of output_tokens, got: {}",
            hint
        );
    }
}
