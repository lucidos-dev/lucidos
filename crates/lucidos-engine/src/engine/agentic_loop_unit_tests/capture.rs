mod parse_app_capture_marker_tests {
    use super::super::parse_app_capture_marker;

    #[test]
    fn parses_screenshot_and_dom() {
        let s = "[APP_CAPTURE:abc123base64]\n<html><body>hi</body></html>";
        let (b64, dom) = parse_app_capture_marker(s).expect("must match");
        assert_eq!(b64, "abc123base64");
        assert_eq!(dom, "<html><body>hi</body></html>");
    }

    #[test]
    fn returns_none_for_missing_prefix() {
        assert!(parse_app_capture_marker("plain tool result").is_none());
    }

    #[test]
    fn returns_none_for_missing_close_bracket() {
        // No `]\n` after the prefix → can't separate screenshot from DOM.
        assert!(parse_app_capture_marker("[APP_CAPTURE:abc no bracket").is_none());
    }

    #[test]
    fn empty_dom_text_is_allowed() {
        let (b64, dom) = parse_app_capture_marker("[APP_CAPTURE:b64]\n").expect("must match");
        assert_eq!(b64, "b64");
        assert_eq!(dom, "");
    }
}

mod format_capture_result_tests {
    use super::super::{format_capture_result, parse_app_capture_marker};

    #[test]
    fn round_trips_screenshot_and_dom_when_capture_succeeds() {
        let formatted = format_capture_result("abc123base64", "<html><body>hi</body></html>");
        let (b64, dom) = parse_app_capture_marker(&formatted).expect("must round-trip");
        assert_eq!(b64, "abc123base64");
        assert_eq!(dom, "DOM snapshot:\n<html><body>hi</body></html>");
    }

    #[test]
    fn omits_marker_when_screenshot_is_empty() {
        // SDK sets `screenshot = ""` when html2canvas fails (e.g. an
        // `oklab(...)` CSS color it can't parse) and puts the failure in `dom`.
        // Emitting `[APP_CAPTURE:]` (empty marker) would make the agentic loop
        // build an empty Image content block, which the Anthropic API rejects
        // with `400 "image cannot be empty"`. Drop the marker entirely so the
        // tool-result text falls through to the plain-text path and the LLM
        // still sees the failure reason.
        let formatted = format_capture_result(
            "",
            "Error: Attempting to parse an unsupported color function \"oklab\"",
        );
        assert!(
            !formatted.starts_with("[APP_CAPTURE:"),
            "empty screenshot must NOT be wrapped in an [APP_CAPTURE:] marker; got {:?}",
            formatted
        );
        assert!(parse_app_capture_marker(&formatted).is_none());
        assert!(formatted.contains("Error: Attempting to parse"));
    }
}

mod sniff_image_media_type_tests {
    use super::super::sniff_image_media_type;
    use base64::Engine;

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn detects_jpeg_from_magic() {
        let jpeg = b64(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F']);
        assert_eq!(sniff_image_media_type(&jpeg), "image/jpeg");
    }

    #[test]
    fn detects_png_from_magic() {
        let png = b64(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00]);
        assert_eq!(sniff_image_media_type(&png), "image/png");
    }

    #[test]
    fn detects_webp_from_magic() {
        // RIFF????WEBP — sniff_image_mime checks bytes [0..4] and [8..12].
        let webp = b64(&[
            b'R', b'I', b'F', b'F', 0x00, 0x00, 0x00, 0x00, b'W', b'E', b'B', b'P',
        ]);
        assert_eq!(sniff_image_media_type(&webp), "image/webp");
    }

    #[test]
    fn detects_gif_from_magic() {
        let gif = b64(&[b'G', b'I', b'F', b'8', b'9', b'a', 0x00, 0x00, 0x00]);
        assert_eq!(sniff_image_media_type(&gif), "image/gif");
    }

    #[test]
    fn detects_heic_from_ftyp_brand() {
        // ????ftyp + brand at offset 8-11.
        let heic = b64(&[
            0x00, 0x00, 0x00, 0x20, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c',
        ]);
        assert_eq!(sniff_image_media_type(&heic), "image/heic");
    }

    #[test]
    fn defaults_to_png_for_unknown() {
        // Unknown prefix — caller is feeding us our own captures, so PNG is the
        // safer default (the Anthropic API will reject either way; we pick the
        // pre-fix hardcoded value to keep behaviour identical for non-image input).
        assert_eq!(sniff_image_media_type(&b64(&[0; 16])), "image/png");
        assert_eq!(sniff_image_media_type(""), "image/png");
    }
}
