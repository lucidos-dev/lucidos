use std::io::{self, Read, Write};
use std::path::PathBuf;

use crate::http::{client as http_client, send_expect_success};
use crate::workspace::{BoxError, Workspace};

const DATA_PREFIXES: &[&str] = &[
    "artifacts/",
    "apps/",
    "knowhow/",
    "triggers/",
    "config/",
    "auth-modules/",
    "scripts/",
    "themes/",
    "fonts/",
];

pub(crate) fn normalize_data_path(relative: &str) -> Result<String, BoxError> {
    let trimmed = relative.trim();
    if trimmed.is_empty() {
        return Err("path is empty".into());
    }
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(format!("path must be relative, got {:?}", relative).into());
    }
    if trimmed.split(['/', '\\']).any(|seg| seg == "..") {
        return Err(format!("path may not contain '..' segments, got {:?}", relative).into());
    }
    Ok(normalize(trimmed))
}

pub(crate) fn resolve_data_path(ws: &Workspace, relative: &str) -> Result<PathBuf, BoxError> {
    Ok(ws.data_dir().join(normalize_data_path(relative)?))
}

fn chat_link(normalized: &str, size: Option<(usize, usize)>) -> String {
    let label = normalized.rsplit('/').next().unwrap_or(normalized);
    let image = is_image(label);
    let target = match size {
        Some((w, h))
            if image && (1..=MAX_HINT_SIDE).contains(&w) && (1..=MAX_HINT_SIDE).contains(&h) =>
        {
            format!("{normalized}#{w}x{h}")
        }
        _ => normalized.to_string(),
    };
    let bang = if image { "!" } else { "" };
    if target.contains(char::is_whitespace) {
        format!("{bang}[{label}](<{target}>)")
    } else {
        format!("{bang}[{label}]({target})")
    }
}

const MAX_HINT_SIDE: usize = 99_999;

fn drawn_size(bytes: &[u8]) -> Option<(usize, usize)> {
    const EXIF_MARKERS: [&[u8]; 3] = [b"Exif\0\0", b"eXIf", b"EXIF"];
    let head = &bytes[..bytes.len().min(64 * 1024)];
    if EXIF_MARKERS
        .iter()
        .any(|marker| head.windows(marker.len()).any(|w| w == *marker))
    {
        return None;
    }
    let size = imagesize::blob_size(bytes).ok()?;
    Some((size.width, size.height))
}

const IMAGE_NOT_SHOWN_YET: &str = "The user cannot see this picture yet. If a question card \
     comes next, put the `![...]` line ON the card: in its question, or in an option's \
     `preview`. For a choice, give each option its own picture of only that option. \
     Your words before any tool call reach the user only as a short summary, which drops \
     the picture. Otherwise paste the line in the reply that ends your turn.";

const LINK_NOT_SHOWN_YET: &str = "The user cannot see this file yet. If a question card comes \
     next, put the link line ON the card, in its question. An option shows a link as plain \
     text. Your words before any tool call reach the user only as a short summary, which \
     drops the link. Otherwise paste the line in the reply that ends your turn.";

fn not_shown_yet(normalized: &str) -> Option<&'static str> {
    if is_image(normalized) {
        Some(IMAGE_NOT_SHOWN_YET)
    } else if normalized.starts_with("artifacts/") {
        Some(LINK_NOT_SHOWN_YET)
    } else {
        None
    }
}

fn is_image(file_name: &str) -> bool {
    const IMAGE_EXTENSIONS: &[&str] = &["gif", "jpeg", "jpg", "png", "svg", "webp"];
    file_name.rsplit_once('.').is_some_and(|(_, ext)| {
        IMAGE_EXTENSIONS
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(known))
    })
}

fn normalize(path: &str) -> String {
    if DATA_PREFIXES.iter().any(|p| path.starts_with(p)) {
        path.to_string()
    } else {
        format!("artifacts/{}", path)
    }
}

pub(crate) fn cmd_path(ws: &Workspace, relative: &str, mkdir: bool) -> Result<(), BoxError> {
    let abs = resolve_data_path(ws, relative)?;
    if mkdir {
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dir {}: {}", parent.display(), e))?;
        }
    }
    println!("{}", abs.display());
    Ok(())
}

pub(crate) enum WriteSource {
    Stdin,
    File(PathBuf),
}

fn encode_path_segments(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

pub(crate) fn cmd_write(
    ws: &Workspace,
    relative: &str,
    source: WriteSource,
) -> Result<(), BoxError> {
    let normalized = normalize_data_path(relative)?;
    let abs = ws.data_dir().join(&normalized);

    let bytes = match source {
        WriteSource::Stdin => {
            let mut buf = Vec::new();
            io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| format!("Failed to read stdin: {}", e))?;
            buf
        }
        WriteSource::File(path) => std::fs::read(&path)
            .map_err(|e| format!("Failed to read source file {}: {}", path.display(), e))?,
    };

    let size = is_image(&normalized).then(|| drawn_size(&bytes)).flatten();

    let url = format!(
        "{}/api/v1/data/{}",
        ws.base_url(),
        encode_path_segments(&normalized)
    );
    let req = http_client()?
        .put(&url)
        .header("Content-Type", "application/octet-stream")
        .body(bytes);
    let reply = send_expect_success("PUT", &url, req)?;

    let mut stderr = io::stderr();
    if let Some(warning) = write_warning(&reply) {
        writeln!(stderr, "{warning}")
            .map_err(|e| format!("Failed to write status to stderr: {}", e))?;
    }
    if let Some(reminder) = not_shown_yet(&normalized) {
        writeln!(stderr, "{reminder}")
            .map_err(|e| format!("Failed to write status to stderr: {}", e))?;
    }
    writeln!(stderr, "{}", abs.display())
        .map_err(|e| format!("Failed to write status to stderr: {}", e))?;

    println!("{}", chat_link(&normalized, size));
    Ok(())
}

fn write_warning(reply: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(reply).ok()?;
    value.get("warning")?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws_at(root: PathBuf) -> Workspace {
        Workspace {
            root,
            api_port: 0,
            proto: "https".to_string(),
            api_base_override: None,
        }
    }

    #[test]
    fn keeps_known_prefix() {
        let ws = ws_at(PathBuf::from("/ws"));
        let abs = resolve_data_path(&ws, "artifacts/ua/report.html").unwrap();
        assert_eq!(abs, PathBuf::from("/ws/data/artifacts/ua/report.html"));
    }

    #[test]
    fn keeps_each_known_prefix() {
        let ws = ws_at(PathBuf::from("/ws"));
        for prefix in DATA_PREFIXES {
            let path = format!("{}{}", prefix, "child/file.txt");
            let abs = resolve_data_path(&ws, &path).unwrap();
            assert_eq!(abs, PathBuf::from(format!("/ws/data/{}", path)));
        }
    }

    fn quoted_list<'a>(source: &'a str, declaration: &str, quote: char) -> Vec<&'a str> {
        let (_, rest) = source
            .split_once(declaration)
            .unwrap_or_else(|| panic!("{declaration} is gone"));
        let (list, _) = rest.split_once("];").expect("the list must close");
        let mut entries: Vec<&str> = list.split(quote).skip(1).step_by(2).collect();
        entries.sort_unstable();
        entries
    }

    #[test]
    fn every_data_prefix_list_matches_the_engine() {
        const ENGINE: &str = include_str!("../../lucidos-core/src/core/data_prefixes.rs");
        const FRONTEND: &str = include_str!("../../lucidos-app/src/utils/dataPathPrefixes.ts");
        let engine = quoted_list(ENGINE, "pub const MUTABLE_PREFIXES: &[&str] = &[", '"');
        assert!(engine.contains(&"themes/"), "parsed: {engine:?}");

        let mut cli = DATA_PREFIXES.to_vec();
        cli.sort_unstable();
        assert_eq!(cli, engine, "the CLI's DATA_PREFIXES");

        let frontend = quoted_list(
            FRONTEND,
            "export const DATA_PATH_PREFIXES: readonly string[] = [",
            '\'',
        );
        let mut served = engine.clone();
        served.push("system-knowhow/");
        served.sort_unstable();
        assert_eq!(frontend, served, "the frontend's DATA_PATH_PREFIXES");
    }

    #[test]
    fn keeps_a_theme_path() {
        assert_eq!(
            normalize_data_path("themes/harbour.json").unwrap(),
            "themes/harbour.json"
        );
    }

    #[test]
    fn prepends_artifacts_when_missing_prefix() {
        let ws = ws_at(PathBuf::from("/ws"));
        let abs = resolve_data_path(&ws, "report.html").unwrap();
        assert_eq!(abs, PathBuf::from("/ws/data/artifacts/report.html"));
    }

    #[test]
    fn rejects_absolute_path() {
        let ws = ws_at(PathBuf::from("/ws"));
        assert!(resolve_data_path(&ws, "/etc/passwd").is_err());
    }

    #[test]
    fn rejects_dot_dot_segment() {
        let ws = ws_at(PathBuf::from("/ws"));
        assert!(resolve_data_path(&ws, "artifacts/../../etc/passwd").is_err());
    }

    #[test]
    fn rejects_empty_path() {
        let ws = ws_at(PathBuf::from("/ws"));
        assert!(resolve_data_path(&ws, "   ").is_err());
    }

    #[test]
    fn encode_path_segments_keeps_separators_and_unreserved_chars() {
        assert_eq!(
            encode_path_segments("artifacts/pr-review/pr_1582/index.html"),
            "artifacts/pr-review/pr_1582/index.html"
        );
    }

    #[test]
    fn encode_path_segments_escapes_a_space() {
        assert_eq!(
            encode_path_segments("artifacts/quarterly report.md"),
            "artifacts/quarterly%20report.md"
        );
    }

    #[test]
    fn encode_path_segments_escapes_the_fragment_and_query_markers() {
        assert_eq!(
            encode_path_segments("artifacts/a#b?c.md"),
            "artifacts/a%23b%3Fc.md"
        );
    }

    #[test]
    fn encode_path_segments_escapes_non_ascii_bytewise() {
        assert_eq!(
            encode_path_segments("artifacts/å.md"),
            "artifacts/%C3%A5.md"
        );
    }

    #[test]
    fn normalize_data_path_prepends_artifacts_when_missing_prefix() {
        assert_eq!(
            normalize_data_path("report.html").unwrap(),
            "artifacts/report.html"
        );
    }

    #[test]
    fn normalize_data_path_keeps_known_prefix() {
        assert_eq!(normalize_data_path("knowhow/x.md").unwrap(), "knowhow/x.md");
    }

    #[test]
    fn chat_link_uses_basename_label_and_bare_path_target() {
        assert_eq!(
            chat_link("artifacts/ticket-workflow/node-types-and-attributes.md", None),
            "[node-types-and-attributes.md](artifacts/ticket-workflow/node-types-and-attributes.md)"
        );
    }

    #[test]
    fn chat_link_handles_top_level_file() {
        assert_eq!(
            chat_link("artifacts/report.html", None),
            "[report.html](artifacts/report.html)"
        );
    }

    #[test]
    fn chat_link_for_an_image_is_markdown_image_syntax() {
        assert_eq!(
            chat_link("artifacts/design/options.png", None),
            "![options.png](artifacts/design/options.png)"
        );
    }

    #[test]
    fn chat_link_matches_image_extensions_case_insensitively() {
        for path in ["artifacts/a.JPG", "artifacts/a.jpeg", "artifacts/a.webp"] {
            assert!(chat_link(path, None).starts_with("!["), "{path}");
        }
    }

    #[test]
    fn chat_link_with_a_space_brackets_the_target() {
        assert_eq!(
            chat_link("artifacts/quarterly chart.png", None),
            "![quarterly chart.png](<artifacts/quarterly chart.png>)"
        );
        assert_eq!(
            chat_link("artifacts/quarterly report.html", None),
            "[quarterly report.html](<artifacts/quarterly report.html>)"
        );
    }

    #[test]
    fn chat_link_for_a_measured_image_carries_a_size_hint() {
        assert_eq!(
            chat_link("artifacts/design/options.png", Some((1600, 1200))),
            "![options.png](artifacts/design/options.png#1600x1200)"
        );
        assert_eq!(
            chat_link("artifacts/quarterly chart.png", Some((800, 600))),
            "![quarterly chart.png](<artifacts/quarterly chart.png#800x600>)"
        );
    }

    #[test]
    fn chat_link_drops_a_size_the_frontend_would_not_read() {
        for size in [(0, 10), (10, 0), (100_000, 10)] {
            assert_eq!(
                chat_link("artifacts/a.png", Some(size)),
                "![a.png](artifacts/a.png)"
            );
        }
    }

    #[test]
    fn chat_link_for_a_non_image_ignores_a_size() {
        assert_eq!(
            chat_link("artifacts/report.html", Some((10, 10))),
            "[report.html](artifacts/report.html)"
        );
    }

    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&w.to_be_bytes());
        png.extend_from_slice(&h.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        png
    }

    #[test]
    fn a_png_header_gives_the_size_the_hint_carries() {
        assert_eq!(drawn_size(&png_header(640, 480)), Some((640, 480)));
    }

    #[test]
    fn a_picture_carrying_exif_gets_no_size() {
        let mut png = png_header(640, 480);
        png.extend_from_slice(b"\0\0\0\x08eXIfMM\0\x2a\0\0\0\x08");
        assert_eq!(drawn_size(&png), None);

        let mut jpeg = b"\xFF\xD8\xFF\xE1\0\x10Exif\0\0MM\0\x2a".to_vec();
        jpeg.extend_from_slice(&png_header(640, 480));
        assert_eq!(drawn_size(&jpeg), None);
    }

    #[test]
    fn the_picture_reminder_sends_a_picture_before_a_card_onto_the_card() {
        let card = IMAGE_NOT_SHOWN_YET
            .find("ON the card")
            .expect("names the card");
        let reply = IMAGE_NOT_SHOWN_YET
            .find("reply that ends your turn")
            .expect("names the turn-ending reply");
        assert!(card < reply, "the card comes first: {IMAGE_NOT_SHOWN_YET}");
        assert!(IMAGE_NOT_SHOWN_YET.contains("`preview`"));
        assert!(IMAGE_NOT_SHOWN_YET.contains("short summary"));
        assert!(!IMAGE_NOT_SHOWN_YET.contains("same message"));
    }

    #[test]
    fn the_card_gate_reads_the_same_picture_extensions() {
        const GATE: &str =
            include_str!("../../lucidos-agents-api/src/engine/question_card_gate.rs");
        const LIST: &str = r#"&["gif", "jpeg", "jpg", "png", "svg", "webp"]"#;
        assert!(GATE.contains(LIST), "the gate's `is_picture` list drifted");
        assert!(include_str!("data.rs").contains(LIST), "`is_image` drifted");
    }

    #[test]
    fn every_artifact_gets_a_reminder_and_other_files_none() {
        assert_eq!(
            not_shown_yet("artifacts/design/a.png"),
            Some(IMAGE_NOT_SHOWN_YET)
        );
        assert_eq!(
            not_shown_yet("artifacts/explainer.html"),
            Some(LINK_NOT_SHOWN_YET)
        );
        assert_eq!(not_shown_yet("knowhow/notes.md"), None);
        let card = LINK_NOT_SHOWN_YET
            .find("ON the card")
            .expect("names the card");
        let reply = LINK_NOT_SHOWN_YET
            .find("reply that ends your turn")
            .expect("names the turn-ending reply");
        assert!(card < reply, "the card comes first: {LINK_NOT_SHOWN_YET}");
        assert!(LINK_NOT_SHOWN_YET.contains("short summary"));
    }

    #[test]
    fn chat_link_for_a_non_image_with_an_image_like_name_stays_a_link() {
        assert_eq!(
            chat_link("artifacts/png-notes.md", None),
            "[png-notes.md](artifacts/png-notes.md)"
        );
    }

    #[test]
    fn a_write_reply_warning_is_read_and_its_absence_is_none() {
        let warned = r#"{"success":true,"path":"knowhow/a.md","warning":"too big"}"#;
        assert_eq!(write_warning(warned).as_deref(), Some("too big"));
        assert_eq!(write_warning(r#"{"success":true}"#), None);
        assert_eq!(write_warning("not json"), None);
    }
}
