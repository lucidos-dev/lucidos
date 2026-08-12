//! Fira Code, served to app iframes from the local engine.
//!
//! Fira Code is the DEFAULT UI font, and the default has to work on a workspace
//! with no internet. Inter / JetBrains Mono / IBM Plex Mono are still fetched
//! from Google Fonts on demand, which is fine for a font the user opted into;
//! the default is different in kind, because a Lucidos workspace is a
//! self-contained local install and its ordinary appearance must not depend on
//! a third-party origin being reachable. Off the CDN, an offline or air-gapped
//! workspace would render every app in the browser's generic `monospace`, and
//! every boot would announce itself to Google.
//!
//! **One copy of the bytes in the tree.** The host bundle gets the font through
//! Vite's asset graph (the `@font-face` at the top of
//! `crates/lucidos-app/src/styles/global/base.css`, hashed into `assets/` and so
//! covered by the service worker's shell cache). App iframes are outside that
//! bundle, so the engine `include_bytes!`s the SAME file the host stylesheet
//! points at, the same way `api/sdk.rs` `include_str!`s the app's
//! `shared-components.css`. A cross-crate include is a compile-time file read,
//! so the packaged binary carries the font with no runtime file dependency.

use super::*;

/// The `@font-face` rule apps load. A real file rather than an inline string so
/// `styles/__tests__/engine-served-css-parses.test.ts` can postcss-parse it:
/// nothing else does, since `cargo build` sees opaque bytes and the file is
/// outside the Vite graph.
const FIRA_CODE_CSS: &str = include_str!("sdk_fonts_fira_code.css");

/// The variable font itself, weights 300-700 in one file (Fira Code 6.2, SIL
/// Open Font License 1.1). Same path the host's `@font-face` resolves.
const FIRA_CODE_WOFF2: &[u8] =
    include_bytes!("../../../lucidos-app/src/assets/fonts/FiraCode-VF.woff2");

/// The upstream release these bytes came from. It is IN THE SERVED FILENAME,
/// which is what lets the bytes be cached as `immutable` below: upgrading the
/// font changes the URL, so no client can be left on the old glyphs. Bump it
/// here and in the stylesheet together (`css_and_route_agree_on_the_filename`
/// fails otherwise).
const FIRA_CODE_VERSION: &str = "6.2";

fn woff2_route() -> String {
    format!("/fonts/fira-code-{FIRA_CODE_VERSION}.woff2")
}

/// The stylesheet is the MUTABLE pointer at the immutable bytes, so it gets a
/// short life. An hour is generous for a request that never leaves the machine,
/// and it bounds how long a warm client can keep pointing at a superseded font.
const CSS_CACHE_CONTROL: &str = "public, max-age=3600";

/// The bytes are content-versioned by their own filename, so a year with no
/// revalidation is a promise we can actually keep.
const WOFF2_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// GET /api/v1/fonts/fira-code.css
pub(super) async fn serve_fira_code_css() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, CSS_CACHE_CONTROL),
        ],
        FIRA_CODE_CSS,
    )
        .into_response()
}

/// GET /api/v1/fonts/fira-code-<version>.woff2
pub(super) async fn serve_fira_code_woff2() -> Response {
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, WOFF2_CACHE_CONTROL),
        ],
        FIRA_CODE_WOFF2,
    )
        .into_response()
}

/// Routes for the locally-served web fonts.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/fonts/fira-code.css", get(serve_fira_code_css))
        .route(&woff2_route(), get(serve_fira_code_woff2))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The font must actually be in the binary. An `include_bytes!` of a file
    /// that got lost in a rebase still compiles if the path resolves to an empty
    /// file, and the failure downstream is silent: apps just render system mono.
    #[test]
    fn the_font_is_embedded_and_is_a_woff2() {
        assert!(
            FIRA_CODE_WOFF2.len() > 50_000,
            "FiraCode-VF.woff2 should be ~113 KB, got {} bytes",
            FIRA_CODE_WOFF2.len()
        );
        // woff2 magic number, 'wOF2'.
        assert_eq!(&FIRA_CODE_WOFF2[..4], b"wOF2");
    }

    /// The `url()` in the stylesheet must stay relative. An absolute
    /// `/api/v1/fonts/...` resolves against the ORIGIN, so it would 404 on every
    /// workspace behind a gateway prefix (`/<slug>/api/v1/...`) while working
    /// perfectly on a root-mounted dev engine, which is the worst shape a bug
    /// can have.
    #[test]
    fn the_font_url_is_relative_to_the_stylesheet() {
        assert!(
            !FIRA_CODE_CSS.contains("url('/"),
            "an absolute url() breaks every gateway-prefixed workspace"
        );
    }

    /// The stylesheet is a static file and the route is built from a Rust
    /// const, so nothing but this holds the filename together. Drift is a 404
    /// for the font and an app rendered in system mono, from two files that each
    /// look right on their own.
    #[test]
    fn css_and_route_agree_on_the_filename() {
        let filename = woff2_route()
            .rsplit('/')
            .next()
            .expect("the route ends in a filename")
            .to_string();
        assert!(
            FIRA_CODE_CSS.contains(&format!("url('{filename}')")),
            "the @font-face src must be the bare sibling `{filename}`, got:\n{FIRA_CODE_CSS}"
        );
    }

    /// `woff2-variations` is a non-standard Safari-10-era format string that CSS
    /// Fonts 4 replaced with `tech(variations)`, and a UA that does not
    /// recognise a `format()` value SKIPS that source: the face would silently
    /// never load, and every app would render in the fallback stack with nothing
    /// in the console tying it to this file. Plain `woff2` is what every browser
    /// with variable-font support reads.
    #[test]
    fn the_face_declares_a_format_every_browser_knows() {
        // The DECLARATION, not the file: the comment above it names the wrong
        // spelling on purpose, to say why it is wrong.
        let src = FIRA_CODE_CSS
            .lines()
            .find(|l| l.trim_start().starts_with("src:"))
            .expect("the @font-face must carry a src");
        assert!(src.contains("format('woff2')"), "got: {src}");
        assert!(!src.contains("woff2-variations"), "got: {src}");
    }

    /// The bytes may promise a year only because their URL carries the version.
    /// Serving a future Fira Code at a stable path under this header would leave
    /// every warm client on the old glyphs, with no way to invalidate them.
    #[test]
    fn only_the_versioned_bytes_are_cached_immutably() {
        assert!(woff2_route().contains(FIRA_CODE_VERSION));
        assert!(WOFF2_CACHE_CONTROL.contains("immutable"));
        assert!(
            !CSS_CACHE_CONTROL.contains("immutable"),
            "the stylesheet is the mutable pointer that makes the above safe"
        );
    }

    /// The variable font carries the whole 300-700 range in one file, so the
    /// descriptor has to declare the range. A single `font-weight: 400` would
    /// make the browser synthesise bold from the regular instance instead of
    /// using the real one.
    #[test]
    fn the_face_declares_the_variable_weight_range() {
        assert!(FIRA_CODE_CSS.contains("font-weight: 300 700;"));
    }
}
