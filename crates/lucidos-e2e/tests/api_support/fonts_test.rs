//! `GET /api/v1/fonts/fira-code.{css,woff2}`: the vendored default UI font,
//! served to app iframes by the local engine rather than fetched from Google.
//!
//! Unit tests in `api/sdk_fonts.rs` cover the embedded bytes and the stylesheet
//! text. What only a booted engine can show is that the two are actually ROUTED,
//! and that the stylesheet's relative `url()` resolves to the sibling that
//! serves the font. A missing route fails silently in the browser: the `<link>`
//! 404s and every app quietly renders in system mono.

use crate::support::{base_url, http_client};

#[tokio::test]
async fn fira_code_stylesheet_points_at_a_font_that_is_really_there() {
    let client = http_client();

    let css_url = format!("{}/api/v1/fonts/fira-code.css", base_url());
    let resp = client
        .get(&css_url)
        .send()
        .await
        .expect("Fira Code stylesheet request failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/css; charset=utf-8"),
        "a stylesheet served as anything else is ignored by the browser"
    );
    let css = resp.text().await.expect("stylesheet body");
    assert!(css.contains("@font-face"), "got:\n{css}");

    // Resolve the src the way a browser does: relative to the stylesheet's own
    // URL. That is what makes this work under a gateway prefix, and following it
    // for real is the only way to prove the two routes agree.
    let src = css
        .split("url('")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .expect("the @font-face must carry a url()");
    let font_url = reqwest::Url::parse(&css_url)
        .expect("stylesheet url")
        .join(src)
        .expect("the src must resolve against the stylesheet");

    let resp = client
        .get(font_url.clone())
        .send()
        .await
        .unwrap_or_else(|e| panic!("font request to {font_url} failed: {e}"));
    assert_eq!(resp.status(), 200, "the @font-face src 404s: {font_url}");
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("font/woff2")
    );

    // Length first: an empty body would make the magic-number slice below panic
    // on an index rather than say what was wrong.
    let bytes = resp.bytes().await.expect("font body");
    assert!(
        bytes.len() > 50_000,
        "FiraCode-VF.woff2 should be ~113 KB, got {} bytes",
        bytes.len()
    );
    assert_eq!(&bytes[..4], b"wOF2", "not a woff2 payload");
}
