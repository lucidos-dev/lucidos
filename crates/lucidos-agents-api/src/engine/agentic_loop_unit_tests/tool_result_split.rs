mod tool_result_split_tests {
    use super::super::{
        build_tool_result_blocks, holds_answer_images, holds_explicitly_requested_image,
        parse_app_capture_marker, split_tool_result, strip_app_capture_marker, take_answer_images,
        ToolOutput,
    };
    use crate::core::store::synthesize_tool_use_id;
    use crate::engine::tools::files::{parse_image_content_marker, EXPLICIT_IMAGE_RESULT_TEXT};
    use crate::llm::ContentBlock;

    const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    fn event_id() -> uuid::Uuid {
        uuid::Uuid::parse_str("090b688b-f0dd-4fed-9047-0ad54d76b2a4").unwrap()
    }

    fn out(tool_use_id: &str, text: String) -> ToolOutput {
        ToolOutput {
            tool_use_id: tool_use_id.to_string(),
            text,
            event_id: Some(event_id()),
            images: Vec::new(),
        }
    }

    fn tool_result_contents(blocks: &[ContentBlock]) -> Vec<&str> {
        blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect()
    }

    fn image_content_sentinel() -> String {
        format!("[IMAGE_CONTENT:image/png]\n{}", PNG_1X1)
    }

    fn app_capture_sentinel() -> String {
        format!("[APP_CAPTURE:{}]\nDOM snapshot:\n<html>hi</html>", PNG_1X1)
    }

    fn images_in(blocks: &[ContentBlock]) -> usize {
        blocks
            .iter()
            .filter(|b| matches!(b, ContentBlock::Image { .. }))
            .count()
    }

    #[test]
    fn explicit_image_sentinel_survives_to_the_model() {
        let split = split_tool_result(&image_content_sentinel());

        let parsed = parse_image_content_marker(&split.llm_text);
        assert!(
            parsed.is_some(),
            "the model-facing text must still parse as an image sentinel, got: {}",
            split.llm_text
        );
        let (media_type, b64) = parsed.unwrap();
        assert_eq!(media_type, "image/png");
        assert_eq!(b64, PNG_1X1, "the bytes must reach the model unmodified");
    }

    #[test]
    fn explicit_image_bytes_never_reach_the_event() {
        let split = split_tool_result(&image_content_sentinel());

        assert!(
            !split.event_text().contains(PNG_1X1),
            "base64 must not be persisted, got: {}",
            split.event_text()
        );
        assert!(
            split
                .event_text()
                .contains("omitted, not embedded in event"),
            "the event text must be the stub, got: {}",
            split.event_text()
        );
        assert!(split.images.is_empty());
    }

    #[test]
    fn app_capture_reaches_the_model_but_its_bytes_do_not_reach_the_event() {
        let split = split_tool_result(&app_capture_sentinel());

        let parsed = parse_app_capture_marker(&split.llm_text);
        assert!(
            parsed.is_some(),
            "the model-facing text must still parse as a capture sentinel"
        );
        assert_eq!(parsed.unwrap().0, PNG_1X1);

        assert!(
            !split.event_text().contains(PNG_1X1),
            "screenshot base64 must not be persisted, got: {}",
            split.event_text()
        );
        assert!(
            split.event_text().contains("<html>hi</html>"),
            "the DOM text is the part worth persisting, got: {}",
            split.event_text()
        );
    }

    #[test]
    fn app_capture_stub_names_the_media_type_and_size() {
        let stub = strip_app_capture_marker(&app_capture_sentinel()).expect("must match");

        assert!(stub.starts_with("[screenshot image/png, "), "got: {}", stub);
        assert!(
            stub.contains("omitted, not embedded in event]"),
            "got: {}",
            stub
        );
        assert!(
            stub.ends_with("DOM snapshot:\n<html>hi</html>"),
            "got: {}",
            stub
        );
    }

    #[test]
    fn app_capture_stub_returns_none_for_non_matching_input() {
        assert!(strip_app_capture_marker("plain tool result").is_none());
        assert!(strip_app_capture_marker("[APP_CAPTURE:abc no bracket").is_none());
    }

    #[test]
    fn generated_image_moves_its_bytes_into_the_event_images_array() {
        let split = split_tool_result(&format!("[GENERATED_IMAGE:{}]\nDone.", PNG_1X1));

        assert_eq!(split.images, vec![PNG_1X1.to_string()]);
        assert_eq!(split.llm_text, "Done.");
        assert_eq!(split.event_text(), "Done.");
    }

    #[test]
    fn malformed_generated_image_marker_passes_through() {
        let raw = "[GENERATED_IMAGE:abc no terminator";
        let split = split_tool_result(raw);

        assert_eq!(split.llm_text, raw);
        assert_eq!(split.event_text(), raw);
        assert!(split.images.is_empty());
    }

    #[test]
    fn plain_result_passes_through_on_both_sides() {
        let split = split_tool_result("total 4\ndrwxr-xr-x  2 u  staff");

        assert_eq!(split.llm_text, split.event_text());
        assert_eq!(split.llm_text, "total 4\ndrwxr-xr-x  2 u  staff");
        assert!(split.images.is_empty());
    }

    #[test]
    fn the_question_tool_s_answer_images_come_off_its_result() {
        let raw = format!(
            "[ANSWER_IMAGES:{},{}]\n{{\"Q\":\"A\"}}",
            "a".repeat(64),
            "b".repeat(64)
        );
        let (text, hashes) =
            take_answer_images(crate::llm::tool_names::ASK_USER_QUESTION, raw.clone());
        assert_eq!(hashes, vec!["a".repeat(64), "b".repeat(64)]);
        assert_eq!(text, "{\"Q\":\"A\"}");
    }

    #[test]
    fn another_tool_s_lookalike_marker_is_left_alone() {
        let raw = format!("[ANSWER_IMAGES:{}]\noutput", "a".repeat(64));
        let (text, hashes) = take_answer_images("run_bash", raw.clone());
        assert!(hashes.is_empty());
        assert_eq!(text, raw);
    }

    #[test]
    fn tool_output_images_follow_the_results() {
        let with_image = ToolOutput {
            images: vec![crate::engine::types::ChatImage {
                base64: PNG_1X1.to_string(),
                mime_type: "image/png".to_string(),
            }],
            ..out("call_1", "{\"Q\":\"A\"}".to_string())
        };
        let blocks = build_tool_result_blocks(
            &[with_image, out("call_2", "plain".to_string())],
            "Results.",
        );

        assert_eq!(images_in(&blocks), 1);
        let first_image = blocks
            .iter()
            .position(|b| matches!(b, ContentBlock::Image { .. }))
            .unwrap();
        let last_result = blocks
            .iter()
            .rposition(|b| matches!(b, ContentBlock::ToolResult { .. }))
            .unwrap();
        assert!(
            first_image > last_result,
            "images must come after every result"
        );
    }

    #[test]
    fn an_answer_s_images_pin_their_message() {
        let with_image = ToolOutput {
            images: vec![crate::engine::types::ChatImage {
                base64: PNG_1X1.to_string(),
                mime_type: "image/png".to_string(),
            }],
            ..out("call_1", "{\"Q\":\"A\"}".to_string())
        };

        assert!(holds_answer_images(&[
            out("call_2", "plain".to_string()),
            with_image
        ]));
        assert!(!holds_answer_images(&[out("call_2", "plain".to_string())]));
    }

    #[test]
    fn redaction_reaches_both_the_model_and_the_event() {
        let mut split = split_tool_result("[PLUGIN_INSTALL_CONFIRM]{\"overwrites\":[]}");
        split.redact("Waiting for the user to confirm.".to_string());

        assert_eq!(split.llm_text, "Waiting for the user to confirm.");
        assert_eq!(split.event_text(), "Waiting for the user to confirm.");
    }

    #[test]
    fn explicit_image_output_builds_a_vision_block() {
        let split = split_tool_result(&image_content_sentinel());
        let blocks = build_tool_result_blocks(&[out("call_1", split.llm_text)], "Results.");

        assert_eq!(
            images_in(&blocks),
            1,
            "the model must receive actual image bytes, got blocks: {:?}",
            blocks
        );
        assert!(blocks.iter().any(|b| matches!(
            b,
            ContentBlock::ToolResult { content, .. }
                if content.starts_with(EXPLICIT_IMAGE_RESULT_TEXT)
        )));
    }

    #[test]
    fn explicit_image_output_is_pinned() {
        let split = split_tool_result(&image_content_sentinel());
        let blocks = build_tool_result_blocks(&[out("call_1", split.llm_text)], "Results.");

        assert!(
            holds_explicitly_requested_image(&blocks),
            "an explicitly requested image must pin its message against trim pass 0"
        );
    }

    #[test]
    fn app_capture_output_reaches_vision_but_is_not_pinned() {
        let split = split_tool_result(&app_capture_sentinel());
        let blocks = build_tool_result_blocks(&[out("call_1", split.llm_text)], "Results.");

        assert_eq!(images_in(&blocks), 1, "the capture must reach vision");
        assert!(blocks.iter().any(|b| matches!(
            b,
            ContentBlock::ToolResult { content, .. } if content.contains("<html>hi</html>")
        )));
        assert!(
            !holds_explicitly_requested_image(&blocks),
            "an ambient capture must age out after one call, so it must not pin"
        );
    }

    #[test]
    fn failed_screenshot_capture_produces_no_image_block() {
        let raw = super::super::format_capture_result("", "", "rasterize failed: image decode");
        let split = split_tool_result(&raw);
        let blocks = build_tool_result_blocks(&[out("call_1", split.llm_text)], "Results.");

        assert_eq!(
            images_in(&blocks),
            0,
            "an empty screenshot must not become a block"
        );
        assert!(blocks.iter().any(|b| matches!(
            b,
            ContentBlock::ToolResult { content, .. } if content.contains("rasterize failed")
        )));
    }

    #[test]
    fn tool_result_blocks_precede_every_image_block() {
        let explicit = split_tool_result(&image_content_sentinel()).llm_text;
        let capture = split_tool_result(&app_capture_sentinel()).llm_text;
        let blocks = build_tool_result_blocks(
            &[
                out("call_1", explicit),
                out("call_2", "plain output".to_string()),
                out("call_3", capture),
            ],
            "Results.",
        );

        let last_tool_result = blocks
            .iter()
            .rposition(|b| matches!(b, ContentBlock::ToolResult { .. }))
            .expect("there must be tool results");
        let first_image = blocks
            .iter()
            .position(|b| matches!(b, ContentBlock::Image { .. }))
            .expect("there must be images");

        assert!(
            last_tool_result < first_image,
            "every ToolResult must come before the first Image, got: {:?}",
            blocks
        );
        assert_eq!(images_in(&blocks), 2, "both sentinels must reach vision");
        assert!(
            matches!(blocks.last(), Some(ContentBlock::Text { text }) if text == "Results."),
            "the instruction text closes the message"
        );
    }

    #[test]
    fn empty_batch_still_carries_the_instruction() {
        let blocks = build_tool_result_blocks(&[], "Results.");

        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "Results."));
    }

    #[test]
    fn a_live_tool_result_states_its_own_event_id() {
        let blocks = build_tool_result_blocks(&[out("call_1", "done".to_string())], "Results.");

        assert_eq!(
            tool_result_contents(&blocks),
            vec!["done\n[evt-090b688bf0dd4fed90470ad54d76b2a4]"],
        );
    }

    #[test]
    fn the_address_is_the_form_every_reader_already_accepts() {
        let blocks = build_tool_result_blocks(&[out("call_1", "done".to_string())], "Results.");
        let content = tool_result_contents(&blocks)[0];

        let rendered = synthesize_tool_use_id(&event_id());
        assert!(
            content.ends_with(&format!("\n[{rendered}]")),
            "got: {content}"
        );
        let inner = rendered.strip_prefix("evt-").expect("evt- prefix");
        assert_eq!(uuid::Uuid::parse_str(inner).unwrap(), event_id());
    }

    #[test]
    fn the_address_never_reaches_the_persisted_event() {
        let split = split_tool_result("total 4\ndrwxr-xr-x  2 u  staff");

        assert!(
            !split.event_text().contains("evt-"),
            "the event text must carry no address, got: {}",
            split.event_text()
        );
    }

    #[test]
    fn a_result_with_no_event_id_is_left_unmarked() {
        let blocks = build_tool_result_blocks(
            &[ToolOutput {
                tool_use_id: "call_1".to_string(),
                text: "done".to_string(),
                event_id: None,
                images: Vec::new(),
            }],
            "Results.",
        );

        assert_eq!(tool_result_contents(&blocks), vec!["done"]);
    }

    #[test]
    fn both_image_branches_state_the_address_too() {
        let explicit = split_tool_result(&image_content_sentinel()).llm_text;
        let capture = split_tool_result(&app_capture_sentinel()).llm_text;
        let blocks = build_tool_result_blocks(
            &[out("call_1", explicit), out("call_2", capture)],
            "Results.",
        );

        let suffix = format!("\n[{}]", synthesize_tool_use_id(&event_id()));
        for content in tool_result_contents(&blocks) {
            assert!(content.ends_with(&suffix), "unmarked branch: {content}");
        }
    }

    #[test]
    fn an_explicitly_viewed_image_still_pins_once_the_address_is_appended() {
        let split = split_tool_result(&image_content_sentinel());
        let blocks = build_tool_result_blocks(&[out("call_1", split.llm_text)], "Results.");

        assert!(
            tool_result_contents(&blocks)[0].contains("evt-"),
            "this test is only meaningful with the address present"
        );
        assert!(
            holds_explicitly_requested_image(&blocks),
            "the pin must survive the appended address"
        );
    }
}
