//! Image size hints: a trailing `#<width>x<height>` on a markdown image's
//! source, in image pixels. The frontend renderer turns one into a reserved
//! box, so a picture does not grow its card when it loads
//! (`system-knowhow/glossary.md` § Image size hint).

use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

use crate::engine::thread_events::QuestionOption;
use crate::engine::LucidosEngine;

/// A markdown image's destination: `<a path>` (group 1) or `a-path` (group 2).
static IMAGE_DESTINATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"!\[[^\]\n]*\]\(\s*(?:<([^>\n]+)>|([^\s)<>]+))").expect("valid regex")
});

/// Fenced code and inline code spans. Markdown renders no image inside them,
/// so their text stays exactly as written.
static CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```[\s\S]*?```|~~~[\s\S]*?~~~|`[^`\n]+`").expect("valid regex"));

/// The largest side the frontend accepts in a hint (five digits).
const MAX_SIDE: u32 = 99_999;

/// Append a size hint to every image in `markdown` that `size_of` can measure.
/// `size_of` gets the percent-decoded path without its query. A source that
/// already carries a fragment keeps it and gets no hint.
pub(crate) fn add_size_hints(
    markdown: &str,
    size_of: impl Fn(&str) -> Option<(u32, u32)>,
) -> String {
    let code: Vec<_> = CODE.find_iter(markdown).map(|m| m.range()).collect();
    let mut out = String::with_capacity(markdown.len());
    let mut copied_to = 0;
    for caps in IMAGE_DESTINATION.captures_iter(markdown) {
        let Some(dest) = caps.get(1).or_else(|| caps.get(2)) else {
            continue;
        };
        if code.iter().any(|r| r.contains(&dest.start())) {
            continue;
        }
        let Some((w, h)) = hint_for(dest.as_str(), &size_of) else {
            continue;
        };
        out.push_str(&markdown[copied_to..dest.end()]);
        out.push_str(&format!("#{w}x{h}"));
        copied_to = dest.end();
    }
    out.push_str(&markdown[copied_to..]);
    out
}

fn hint_for(dest: &str, size_of: &impl Fn(&str) -> Option<(u32, u32)>) -> Option<(u32, u32)> {
    if dest.contains('#') {
        return None;
    }
    let path = dest.split('?').next().unwrap_or(dest);
    let decoded = urlencoding::decode(path).ok()?;
    size_of(&decoded).filter(|&(w, h)| (1..=MAX_SIDE).contains(&w) && (1..=MAX_SIDE).contains(&h))
}

/// The pixel size of the workspace picture at data-relative `path`, as the
/// browser draws it. `None` for anything but a known data prefix, a traversal,
/// a missing file, or a format without a raster header (SVG). `resolve` never
/// sees a rejected path.
fn measure_workspace_image(
    path: &str,
    resolve: impl Fn(&str) -> Option<PathBuf>,
) -> Option<(u32, u32)> {
    if !crate::core::is_known_data_prefix(path) || crate::core::is_path_traversal(path) {
        return None;
    }
    let mut decoder = image::ImageReader::open(resolve(path)?)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let (w, h) = image::ImageDecoder::dimensions(&decoder);
    // The browser applies the EXIF orientation, so a quarter turn swaps the
    // sides it draws. An unreadable orientation reads as none, as it does there.
    use image::metadata::Orientation::*;
    match image::ImageDecoder::orientation(&mut decoder) {
        Ok(Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH) => Some((h, w)),
        _ => Some((w, h)),
    }
}

impl LucidosEngine {
    /// `markdown` with a size hint on each workspace picture it shows.
    pub(crate) fn with_image_size_hints(&self, markdown: &str) -> String {
        add_size_hints(markdown, |path| {
            measure_workspace_image(path, |p| self.resolve_data_path(p).ok().map(|(_, abs)| abs))
        })
    }

    /// `option` with a size hint on each workspace picture in the markdown the
    /// card renders: its description and preview. The label is plain text.
    pub(crate) fn option_with_image_size_hints(&self, option: QuestionOption) -> QuestionOption {
        QuestionOption {
            description: option.description.map(|d| self.with_image_size_hints(&d)),
            preview: option.preview.map(|p| self.with_image_size_hints(&p)),
            ..option
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn fixed(path: &str) -> Option<(u32, u32)> {
        (path == "artifacts/x.png" || path == "artifacts/quarterly chart.png")
            .then_some((1600, 1200))
    }

    #[test]
    fn a_measurable_image_gets_a_hint() {
        assert_eq!(
            add_size_hints("see ![mockup](artifacts/x.png) here", fixed),
            "see ![mockup](artifacts/x.png#1600x1200) here"
        );
    }

    #[test]
    fn an_angle_bracketed_path_gets_the_hint_inside_the_brackets() {
        assert_eq!(
            add_size_hints("![c](<artifacts/quarterly chart.png>)", fixed),
            "![c](<artifacts/quarterly chart.png#1600x1200>)"
        );
    }

    #[test]
    fn the_path_is_measured_decoded_and_without_its_query() {
        assert_eq!(
            add_size_hints("![c](artifacts/quarterly%20chart.png?v=2)", fixed),
            "![c](artifacts/quarterly%20chart.png?v=2#1600x1200)"
        );
    }

    #[test]
    fn every_image_in_the_text_is_hinted() {
        assert_eq!(
            add_size_hints("![a](artifacts/x.png) and ![b](artifacts/x.png)", fixed),
            "![a](artifacts/x.png#1600x1200) and ![b](artifacts/x.png#1600x1200)"
        );
    }

    #[test]
    fn an_existing_fragment_or_hint_is_left_alone() {
        for md in [
            "![a](artifacts/x.png#detail)",
            "![a](artifacts/x.png#1600x1200)",
        ] {
            assert_eq!(add_size_hints(md, fixed), md);
        }
    }

    #[test]
    fn an_unmeasurable_image_and_a_plain_link_are_left_alone() {
        for md in [
            "![a](artifacts/missing.png)",
            "[a](artifacts/x.png)",
            "no pictures here",
        ] {
            assert_eq!(add_size_hints(md, fixed), md);
        }
    }

    #[test]
    fn an_image_quoted_as_code_is_left_as_written() {
        for md in [
            "`![a](artifacts/x.png)`",
            "```\n![a](artifacts/x.png)\n```",
            "~~~md\n![a](artifacts/x.png)\n~~~",
        ] {
            assert_eq!(add_size_hints(md, fixed), md);
        }
        assert_eq!(
            add_size_hints("`code` then ![a](artifacts/x.png)", fixed),
            "`code` then ![a](artifacts/x.png#1600x1200)"
        );
    }

    #[test]
    fn a_size_the_frontend_would_not_read_is_dropped() {
        for size in [(0, 10), (10, 0), (100_000, 10)] {
            assert_eq!(add_size_hints("![a](x)", |_| Some(size)), "![a](x)");
        }
    }

    #[test]
    fn a_path_outside_the_workspace_data_is_never_resolved() {
        let asked = RefCell::new(Vec::new());
        for path in [
            "artifacts/../../etc/passwd.png",
            "/etc/x.png",
            "https://example.com/x.png",
            ".lucidos/tmp/x.png",
            "elsewhere/x.png",
        ] {
            let size = measure_workspace_image(path, |p| {
                asked.borrow_mut().push(p.to_string());
                None
            });
            assert_eq!(size, None, "{path}");
        }
        assert!(asked.borrow().is_empty(), "resolved: {:?}", asked.borrow());
    }

    #[test]
    fn a_real_png_is_measured_from_its_header() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.png");
        image::RgbImage::new(30, 20).save(&file).unwrap();
        let size = measure_workspace_image("artifacts/x.png", |_| Some(file.clone()));
        assert_eq!(size, Some((30, 20)));
    }

    /// A phone photo is often stored landscape with an EXIF quarter turn. The
    /// browser draws it portrait, so the hint must be portrait too.
    #[test]
    fn an_exif_quarter_turn_swaps_the_sides() {
        let mut jpeg = Vec::new();
        image::RgbImage::new(30, 20)
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        // TIFF header, one IFD entry: Orientation (0x0112), SHORT, value 6.
        let mut payload = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01".to_vec();
        payload.extend_from_slice(b"\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0");
        let len = u16::try_from(payload.len() + 2).unwrap().to_be_bytes();
        let app1 = [&[0xFF, 0xE1], &len[..], &payload].concat();
        jpeg.splice(2..2, app1);

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("photo.jpg");
        std::fs::write(&file, jpeg).unwrap();
        let size = measure_workspace_image("artifacts/photo.jpg", |_| Some(file.clone()));
        assert_eq!(size, Some((20, 30)));
    }

    #[test]
    fn a_missing_or_non_raster_file_measures_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let svg = dir.path().join("x.svg");
        std::fs::write(
            &svg,
            "<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'/>",
        )
        .unwrap();
        assert_eq!(
            measure_workspace_image("artifacts/x.svg", |_| Some(svg.clone())),
            None
        );
        let missing = dir.path().join("gone.png");
        assert_eq!(
            measure_workspace_image("artifacts/gone.png", |_| Some(missing.clone())),
            None
        );
    }
}
