//! The sentinel-to-vision chain, end to end.
//!
//! Every test here covers wiring that used to be inline in `run_agentic_loop`'s
//! body and therefore unreachable from any test. That gap is not incidental: it
//! is how `read_file` on an image and `view_image` came to send the model a
//! stub reading "omitted, not embedded in event" instead of the picture, and
//! stayed that way from 2026-04-26 to 2026-07-30 with a fully green suite. The
//! unit tests that existed asserted on hand-built blocks and never once ran the
//! path that builds them.

mod tool_result_split_tests {
    use super::super::{
        build_tool_result_blocks, holds_explicitly_requested_image, parse_app_capture_marker,
        split_tool_result, strip_app_capture_marker, ToolOutput,
    };
    use crate::core::store::synthesize_tool_use_id;
    use crate::engine::tools::files::{parse_image_content_marker, EXPLICIT_IMAGE_RESULT_TEXT};
    use crate::llm::ContentBlock;

    /// A 1x1 transparent PNG. Small enough that `fit_for_llm` passes it through
    /// untouched, real enough that the mime sniffer recognises it.
    const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    /// Fixed `ToolCalled` event id, so a test can assert on the exact address
    /// the model is handed.
    fn event_id() -> uuid::Uuid {
        uuid::Uuid::parse_str("090b688b-f0dd-4fed-9047-0ad54d76b2a4").unwrap()
    }

    /// One tool output with its `ToolCalled` address, the shape every live call
    /// has. Tests use it so the whole suite runs the marker path.
    fn out(tool_use_id: &str, text: String) -> ToolOutput {
        ToolOutput {
            tool_use_id: tool_use_id.to_string(),
            text,
            event_id: Some(event_id()),
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

    // -----------------------------------------------------------------------
    // The split itself
    // -----------------------------------------------------------------------

    /// THE regression. The persistence stub must not reach the model: the raw
    /// sentinel has to survive `split_tool_result` intact, because the block
    /// builder is what turns it into vision. Stripping it first is a silent
    /// blinding, since the stub is valid prose the model reads as a statement
    /// that the image was withheld.
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

    /// The other half of the same split: the event keeps a stub, never the
    /// base64. One thread accumulated 262 inlined images and reached 46 MB,
    /// which froze iOS PWAs re-fetching the snapshot.
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

    /// Same two properties for the ambient capture sentinel, which had the
    /// mirror-image defect: it reached the model correctly but persisted its
    /// full screenshot, producing 1.53 MB event rows.
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

    /// `strip_app_capture_marker` names the media type and size, mirroring the
    /// image stub, so a human reading the step-detail modal can tell what was
    /// dropped rather than seeing an unexplained gap.
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

    /// A generated image keeps its existing shape: the bytes go to the event's
    /// `images` array for the frontend to render, and the model is not shown
    /// its own synthesised output back. Deliberately unchanged by this fix.
    #[test]
    fn generated_image_moves_its_bytes_into_the_event_images_array() {
        let split = split_tool_result(&format!("[GENERATED_IMAGE:{}]\nDone.", PNG_1X1));

        assert_eq!(split.images, vec![PNG_1X1.to_string()]);
        assert_eq!(split.llm_text, "Done.");
        assert_eq!(split.event_text(), "Done.");
    }

    /// A malformed generated-image marker (no closing bracket) falls through
    /// verbatim rather than being silently truncated.
    #[test]
    fn malformed_generated_image_marker_passes_through() {
        let raw = "[GENERATED_IMAGE:abc no terminator";
        let split = split_tool_result(raw);

        assert_eq!(split.llm_text, raw);
        assert_eq!(split.event_text(), raw);
        assert!(split.images.is_empty());
    }

    /// The overwhelmingly common case: no sentinel, both sides identical.
    #[test]
    fn plain_result_passes_through_on_both_sides() {
        let split = split_tool_result("total 4\ndrwxr-xr-x  2 u  staff");

        assert_eq!(split.llm_text, split.event_text());
        assert_eq!(split.llm_text, "total 4\ndrwxr-xr-x  2 u  staff");
        assert!(split.images.is_empty());
    }

    /// The confirm-flow redaction has to land on BOTH sides. It exists so the
    /// model sees a one-line wait notice instead of parseable JSON it would act
    /// on, and the event should record the same thing the model saw.
    #[test]
    fn redaction_reaches_both_the_model_and_the_event() {
        let mut split = split_tool_result("[PLUGIN_INSTALL_CONFIRM]{\"overwrites\":[]}");
        split.redact("Waiting for the user to confirm.".to_string());

        assert_eq!(split.llm_text, "Waiting for the user to confirm.");
        assert_eq!(split.event_text(), "Waiting for the user to confirm.");
    }

    // -----------------------------------------------------------------------
    // The split feeding the block builder
    // -----------------------------------------------------------------------

    /// The whole chain for an explicitly requested image: sentinel to split to
    /// blocks. The model must end up with a real `ContentBlock::Image`, not
    /// prose about one.
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

    /// And the pin fires, so trim pass 0 keeps the bytes for the rest of the
    /// turn. Without this the model's very next tool call blinds it again,
    /// which is what made `view_image` unable to do its one job.
    #[test]
    fn explicit_image_output_is_pinned() {
        let split = split_tool_result(&image_content_sentinel());
        let blocks = build_tool_result_blocks(&[out("call_1", split.llm_text)], "Results.");

        assert!(
            holds_explicitly_requested_image(&blocks),
            "an explicitly requested image must pin its message against trim pass 0"
        );
    }

    /// An ambient capture also reaches vision, but must NOT pin: it snapshots
    /// state that changes under the model, so a stale screenshot surviving the
    /// whole turn would mislead it about current state.
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

    /// A capture whose screenshot failed carries no marker at all, so it must
    /// fall through to plain text. Wrapping an empty screenshot would build a
    /// `ContentBlock::Image { data: "" }` and earn a 400 `image cannot be
    /// empty` from the provider.
    #[test]
    fn failed_screenshot_capture_produces_no_image_block() {
        let raw = super::super::format_capture_result("", "html2canvas failed on oklab()");
        let split = split_tool_result(&raw);
        let blocks = build_tool_result_blocks(&[out("call_1", split.llm_text)], "Results.");

        assert_eq!(
            images_in(&blocks),
            0,
            "an empty screenshot must not become a block"
        );
        assert!(blocks.iter().any(|b| matches!(
            b,
            ContentBlock::ToolResult { content, .. } if content.contains("html2canvas failed")
        )));
    }

    /// Every `ToolResult` block must precede every `Image` block. The Claude
    /// API validates that tool_result blocks immediately follow the assistant's
    /// tool_use blocks, so interleaving produces `tool_use ids were found
    /// without tool_result blocks` 400s. A mixed batch is where that ordering
    /// is easiest to break.
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

    /// The instruction is always appended, even when nothing was produced.
    #[test]
    fn empty_batch_still_carries_the_instruction() {
        let blocks = build_tool_result_blocks(&[], "Results.");

        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "Results."));
    }

    // -----------------------------------------------------------------------
    // The event address (ADR 0085 Decision 9)
    // -----------------------------------------------------------------------

    /// A live result states its own `ToolCalled` event id, so the model can
    /// note the address of something the turn boundary is about to drop. The
    /// provider's `tool_use_id` cannot serve: it means nothing after the turn.
    #[test]
    fn a_live_tool_result_states_its_own_event_id() {
        let blocks = build_tool_result_blocks(&[out("call_1", "done".to_string())], "Results.");

        assert_eq!(
            tool_result_contents(&blocks),
            vec!["done\n[evt-090b688bf0dd4fed90470ad54d76b2a4]"],
        );
    }

    /// The address is rendered by the SAME function the resume path uses for
    /// its synthetic `tool_use_id`. Two renderers would drift, and the model
    /// would then hold two spellings of one id with no way to tell them apart.
    #[test]
    fn the_address_is_the_form_every_reader_already_accepts() {
        let blocks = build_tool_result_blocks(&[out("call_1", "done".to_string())], "Results.");
        let content = tool_result_contents(&blocks)[0];

        let rendered = synthesize_tool_use_id(&event_id());
        assert!(
            content.ends_with(&format!("\n[{rendered}]")),
            "got: {content}"
        );
        // The bracketed body must parse back to the originating event id, which
        // is what makes it a pointer rather than a decoration.
        let inner = rendered.strip_prefix("evt-").expect("evt- prefix");
        assert_eq!(uuid::Uuid::parse_str(inner).unwrap(), event_id());
    }

    /// Nothing is retained. The address is appended when the wire blocks are
    /// built, downstream of the `ToolResult` emit. So the event payload holds
    /// what it always held, and the pair still vanishes at the boundary.
    #[test]
    fn the_address_never_reaches_the_persisted_event() {
        let split = split_tool_result("total 4\ndrwxr-xr-x  2 u  staff");

        assert!(
            !split.event_text().contains("evt-"),
            "the event text must carry no address, got: {}",
            split.event_text()
        );
    }

    /// A `ToolCalled` whose emit failed has no address to state. The result
    /// goes out unmarked, rather than carrying a placeholder the model would
    /// dereference into a "not found".
    #[test]
    fn a_result_with_no_event_id_is_left_unmarked() {
        let blocks = build_tool_result_blocks(
            &[ToolOutput {
                tool_use_id: "call_1".to_string(),
                text: "done".to_string(),
                event_id: None,
            }],
            "Results.",
        );

        assert_eq!(tool_result_contents(&blocks), vec!["done"]);
    }

    /// Every branch of the builder states the address, not just the plain one.
    /// The two sentinel branches replace the content wholesale, so each needed
    /// the append wiring separately.
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

    /// The pin survives the append. `holds_explicitly_requested_image` matched
    /// `EXPLICIT_IMAGE_RESULT_TEXT` for equality. Appending the address would
    /// then have unpinned every explicitly viewed image, blinding the model on
    /// its very next tool call.
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
