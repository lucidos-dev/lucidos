//! `GET /api/v1/fonts` and `GET /api/v1/fonts/<file>`: the font catalog, and
//! the vendored fonts served to app iframes by the local engine rather than
//! fetched from Google.
//!
//! Unit tests in `core/fonts.rs` and `api/fonts.rs` cover the embedded bytes and
//! the stylesheet text. Only a booted engine can show that the two are actually
//! ROUTED. It also shows that the stylesheet's relative `url()` resolves to the
//! sibling that serves the font. A missing route fails silently in the browser: the `<link>`
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

/// Every font the catalog calls vendored has a stylesheet, and every `url()`
/// in it resolves to a woff2 the engine serves.
#[tokio::test]
async fn every_vendored_font_in_the_catalog_is_routed() {
    let client = http_client();
    let catalog: serde_json::Value = client
        .get(format!("{}/api/v1/fonts", base_url()))
        .send()
        .await
        .expect("catalog request failed")
        .json()
        .await
        .expect("the catalog is JSON");
    assert_eq!(catalog["follow_theme"], "theme");
    let fonts = catalog["fonts"].as_array().expect("a fonts array");
    let vendored: Vec<&str> = fonts
        .iter()
        .filter(|f| f["source"] == "vendored")
        .filter_map(|f| f["id"].as_str())
        .collect();
    assert!(vendored.contains(&"fira-code") && vendored.contains(&"geist"));

    for id in vendored {
        let css_url = format!("{}/api/v1/fonts/{id}.css", base_url());
        let css = client
            .get(&css_url)
            .send()
            .await
            .expect("stylesheet request failed")
            .text()
            .await
            .expect("stylesheet body");
        for src in css.split("url('").skip(1) {
            let file = src.split('\'').next().expect("a closed url()");
            let font_url = reqwest::Url::parse(&css_url).unwrap().join(file).unwrap();
            let resp = client
                .get(font_url.clone())
                .send()
                .await
                .expect("font request");
            assert_eq!(resp.status(), 200, "{id}: {font_url}");
            let bytes = resp.bytes().await.expect("font body");
            assert_eq!(&bytes[..4], b"wOF2", "{id}: {font_url}");
        }
    }
}

fn allow_origin(resp: &reqwest::Response) -> Option<&str> {
    resp.headers()
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
}

/// An app frame's origin is `null`, and a font fetch is CORS-mode. Without the
/// header Chromium refuses the woff2 and every app renders in system mono. The
/// request carries what the browser's does, so the same-origin gate sees it too.
#[tokio::test]
async fn an_app_frame_may_read_fira_code_across_its_opaque_origin() {
    let client = http_client();
    for file in ["fira-code.css", "fira-code-6.2.woff2"] {
        let url = format!("{}/api/v1/fonts/{file}", base_url());
        let resp = client
            .get(&url)
            .header("origin", "null")
            .header("sec-fetch-site", "cross-site")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-dest", "font")
            .send()
            .await
            .expect("font request failed");
        assert_eq!(resp.status(), 200, "{url}");
        assert_eq!(allow_origin(&resp), Some("*"), "{url}");

        let resp = client
            .request(reqwest::Method::OPTIONS, &url)
            .header("origin", "null")
            .header("access-control-request-method", "GET")
            .header("sec-fetch-site", "cross-site")
            .send()
            .await
            .expect("preflight failed");
        assert!(resp.status().is_success(), "{url}: {}", resp.status());
        assert_eq!(allow_origin(&resp), Some("*"), "{url}");
    }
}

/// The grant is the fonts' alone. `sdk.js` loads as a no-cors tag and needs
/// none, and the API proper must never offer a cross-origin read.
#[tokio::test]
async fn no_other_api_route_offers_a_cross_origin_read() {
    let client = http_client();
    for path in [
        "/api/v1/sdk.js",
        "/api/v1/sdk-iframe.css",
        "/api/v1/fonts",
        "/api/v1/health",
        "/api/v1/threads/list",
    ] {
        let resp = client
            .get(format!("{}{path}", base_url()))
            .header("origin", "null")
            .send()
            .await
            .expect("request failed");
        assert_eq!(allow_origin(&resp), None, "{path}");
    }
}
