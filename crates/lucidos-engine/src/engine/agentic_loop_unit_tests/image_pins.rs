mod image_pin_tests {
    use super::super::{
        holds_explicitly_requested_image, push_explicit_image_pin, MAX_PINNED_EXPLICIT_IMAGES,
    };
    use crate::engine::tools::files::EXPLICIT_IMAGE_RESULT_TEXT;
    use crate::llm::ContentBlock;

    fn tool_result(content: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: "t1".to_string(),
            content: content.to_string(),
        }
    }

    fn image_block() -> ContentBlock {
        ContentBlock::Image {
            source_type: "base64".to_string(),
            media_type: "image/png".to_string(),
            data: "abc".to_string(),
        }
    }

    /// The shape the loop builds for `view_image` / `read_file` on an image: the
    /// explicit-view result text plus the lifted image block.
    #[test]
    fn recognises_an_explicitly_requested_image() {
        let blocks = vec![
            tool_result(EXPLICIT_IMAGE_RESULT_TEXT),
            image_block(),
            ContentBlock::Text {
                text: "Results above.".to_string(),
            },
        ];
        assert!(holds_explicitly_requested_image(&blocks));
    }

    /// `capture_app` writes the page's DOM text as its result, so it is not
    /// recognised — ambient captures must keep aging out after one call.
    #[test]
    fn does_not_recognise_an_ambient_capture() {
        let blocks = vec![tool_result("Habit Tracker\nStreak: 4 days"), image_block()];
        assert!(!holds_explicitly_requested_image(&blocks));
    }

    #[test]
    fn does_not_recognise_a_plain_tool_result() {
        let blocks = vec![tool_result("files: a, b, c")];
        assert!(!holds_explicitly_requested_image(&blocks));
    }

    /// Pinned images are exempt from trim pass 0 by construction, and the model
    /// can issue as many image reads in one turn as its tool-call cap allows. A
    /// "describe every photo in this folder" turn would otherwise accumulate
    /// hundreds of un-strippable images and blow the context window. The cap
    /// releases the oldest pin, keeping the most recent window.
    #[test]
    fn pins_are_capped_and_release_oldest_first() {
        let mut pins = Vec::new();
        for idx in 0..(MAX_PINNED_EXPLICIT_IMAGES + 3) {
            push_explicit_image_pin(&mut pins, idx);
        }

        assert_eq!(
            pins.len(),
            MAX_PINNED_EXPLICIT_IMAGES,
            "pinned explicit images must stay bounded no matter how long the turn runs"
        );
        let expected: Vec<usize> = (3..(MAX_PINNED_EXPLICIT_IMAGES + 3)).collect();
        assert_eq!(
            pins, expected,
            "the most recent pins must survive; the oldest are released"
        );
    }

    #[test]
    fn pins_below_the_cap_are_all_kept() {
        let mut pins = Vec::new();
        for idx in 0..3 {
            push_explicit_image_pin(&mut pins, idx);
        }
        assert_eq!(pins, vec![0, 1, 2]);
    }
}
