mod should_flush_tests {
    use super::super::{is_bad_image_description, should_flush};

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
        assert!(should_flush("# Title\n"));
    }

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

    #[test]
    fn flushes_code_block_then_paragraph() {
        assert!(should_flush("```\ncode\n```\n\nNext paragraph\n\n"));
    }

    #[test]
    fn flushes_heading_in_middle_of_text() {
        assert!(should_flush("First part\n## Section\n"));
    }

    #[test]
    fn no_flush_on_list_item() {
        assert!(!should_flush("- item 1\n"));
    }

    #[test]
    fn no_flush_on_numbered_list() {
        assert!(!should_flush("1. item\n"));
    }

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

mod classify_empty_completion_tests {
    use super::super::{
        classify_empty_completion, declined_partway_error, normalize_finish_reason, FinishClass,
    };

    #[test]
    fn a_refusal_after_partial_text_fails_the_turn() {
        for reason in ["refusal", "SAFETY", "content_filter"] {
            let error = declined_partway_error(Some(reason)).expect("a declined turn fails");
            assert!(error.contains("stopped partway"), "{reason}: {error}");
        }
    }

    #[test]
    fn a_turn_with_text_that_was_not_declined_is_an_answer() {
        for reason in [Some("end_turn"), Some("STOP"), Some("max_tokens"), None] {
            assert_eq!(declined_partway_error(reason), None, "{reason:?}");
        }
    }

    #[test]
    fn clean_stop_is_benign_across_providers() {
        for reason in ["end_turn", "STOP", "stop", "completed"] {
            let class = classify_empty_completion(reason, 5, false, 0);
            assert!(
                !class.is_error,
                "{reason} with no dropped output must be benign, hint: {}",
                class.hint
            );
            assert!(
                class.hint.contains("without producing text"),
                "{reason} benign hint should describe intentional silence, got: {}",
                class.hint
            );
        }
    }

    #[test]
    fn truncation_is_error_across_providers() {
        for reason in ["max_tokens", "MAX_TOKENS", "length", "max_output_tokens"] {
            let class = classify_empty_completion(reason, 8192, false, 0);
            assert!(class.is_error, "{reason} must be an error");
            assert!(
                class.hint.contains("truncated"),
                "{reason} must say truncated, got: {}",
                class.hint
            );
        }
    }

    #[test]
    fn safety_block_is_error_across_providers() {
        for reason in ["refusal", "SAFETY", "RECITATION", "content_filter"] {
            let class = classify_empty_completion(reason, 207, false, 0);
            assert!(class.is_error, "{reason} must be an error");
            assert!(
                class.hint.contains("declined"),
                "{reason} must say the model declined, got: {}",
                class.hint
            );
            assert!(
                !class.hint.contains("couldn't classify"),
                "{reason} must NOT blame the parser, got: {}",
                class.hint
            );
        }
    }

    #[test]
    fn safety_block_wins_even_with_zero_output_tokens() {
        let class = classify_empty_completion("refusal", 0, false, 0);
        assert!(class.is_error);
        assert!(class.hint.contains("declined"), "got: {}", class.hint);
    }

    #[test]
    fn dropped_output_with_unknown_shapes_is_error() {
        let class = classify_empty_completion("end_turn", 2222, false, 3);
        assert!(class.is_error, "dropped output must be an error");
        assert!(
            class.hint.contains("dropped unknown SSE shapes"),
            "must call out dropped shapes, got: {}",
            class.hint
        );
    }

    #[test]
    fn dropped_output_without_unknown_shapes_is_error() {
        let class = classify_empty_completion("end_turn", 2222, false, 0);
        assert!(class.is_error, "dropped output must be an error");
        assert!(
            class.hint.contains("couldn't classify"),
            "must call out the parser gap, got: {}",
            class.hint
        );
    }

    #[test]
    fn thinking_only_clean_stop_is_benign() {
        let class = classify_empty_completion("end_turn", 100, true, 0);
        assert!(
            !class.is_error,
            "thinking-then-silence on a clean stop is benign"
        );
        assert!(
            class.hint.contains("thought but produced no text"),
            "thinking-only must mention the thought, got: {}",
            class.hint
        );
    }

    #[test]
    fn thinking_blocks_without_thinking_text_are_still_a_thought() {
        let class = classify_empty_completion("end_turn", 2222, true, 0);
        assert!(!class.is_error, "got: {}", class.hint);
        assert!(class.hint.contains("thought but produced no text"));
    }

    #[test]
    fn unknown_stop_reason_is_error_failsafe() {
        for reason in ["some_future_reason", "stop_sequence", "unknown", "other"] {
            let class = classify_empty_completion(reason, 0, false, 0);
            assert!(class.is_error, "{reason} must fail-safe to an error");
            assert!(
                class.hint.is_empty(),
                "{reason} carries no specific hint, got: {}",
                class.hint
            );
        }
    }

    #[test]
    fn normalize_is_case_insensitive() {
        assert_eq!(normalize_finish_reason("STOP"), FinishClass::Clean);
        assert_eq!(normalize_finish_reason("end_turn"), FinishClass::Clean);
        assert_eq!(
            normalize_finish_reason("MAX_TOKENS"),
            FinishClass::Truncated
        );
        assert_eq!(normalize_finish_reason("safety"), FinishClass::Blocked);
        assert_eq!(
            normalize_finish_reason("stop_sequence"),
            FinishClass::Unknown
        );
    }
}
