//! `/api/v1/fonts`: the font catalog, and the vendored fonts served to app
//! frames from the local engine.
//!
//! A vendored font works on a workspace with no internet, and makes no request
//! to a third-party origin. So every catalog font is vendored or the device's
//! own, and a theme may suggest any of them (ADR 0077, ADR 0303).
//!
//! **One copy of the bytes in the tree.** The host bundle gets each font through
//! Vite's asset graph (the generated `styles/generated/font-faces.css`, hashed
//! into `assets/`). App frames are outside that bundle, so `core::fonts`
//! `include_bytes!`s the SAME files. A cross-crate include is a compile-time
//! read, so the packaged binary carries the fonts with no runtime file
//! dependency.

use super::*;

use crate::core::fonts::{self, FONT_CATALOG};
use crate::core::workspace_fonts;

/// The stylesheet is the MUTABLE pointer at the immutable bytes, so it gets a
/// short life. An hour is generous for a request that never leaves the machine.
/// It also bounds how long a warm client can keep pointing at a superseded font.
const CSS_CACHE_CONTROL: &str = "public, max-age=3600";

/// The bytes are versioned by their own filename, so a year with no
/// revalidation is a promise we can actually keep.
const WOFF2_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// The `@font-face` rules of one vendored font, as an app frame loads them.
///
/// Each `url()` is the bare sibling filename. A relative reference resolves
/// against whatever gateway prefix delivered the stylesheet
/// (`/<slug>/api/v1/fonts/<id>.css`), where an absolute `/api/v1/...` would 404
/// on every non-root workspace.
fn stylesheet(id: &str) -> Option<String> {
    let font = fonts::find(id)?;
    let rules = font.font_face_rules(|face| face.served.to_string());
    (!rules.is_empty()).then_some(rules)
}

fn woff2(file: &str) -> Option<&'static [u8]> {
    FONT_CATALOG
        .iter()
        .flat_map(|font| font.faces())
        .find(|face| face.served == file)
        .map(|face| face.bytes)
}

/// GET /api/v1/fonts/<id>.css and GET /api/v1/fonts/<id>-<version>….woff2
pub(super) async fn serve_font_file(Path(file): Path<String>) -> Response {
    if let Some(css) = file.strip_suffix(".css").and_then(stylesheet) {
        return (
            [
                (header::CONTENT_TYPE, "text/css; charset=utf-8"),
                (header::CACHE_CONTROL, CSS_CACHE_CONTROL),
            ],
            css,
        )
            .into_response();
    }
    if let Some(bytes) = woff2(&file) {
        return (
            [
                (header::CONTENT_TYPE, "font/woff2"),
                (header::CACHE_CONTROL, WOFF2_CACHE_CONTROL),
            ],
            bytes,
        )
            .into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

/// GET /api/v1/fonts: the font catalog and the workspace fonts, for Settings,
/// a theme editor and agents.
pub(super) async fn font_catalog(State(state): State<AppState>) -> Json<serde_json::Value> {
    let data_dir = state.workspace_path.join(crate::core::DATA_DIR);
    Json(fonts::catalog_json(&workspace_fonts::list(&data_dir)))
}

/// Routes for the font catalog and the locally-served fonts.
///
/// The font files are the one `/api/v1` asset that carries
/// `Access-Control-Allow-Origin`. An app frame runs at an opaque origin (ADR
/// 0227), and a font fetch is CORS-mode. Without the header Chromium refuses the
/// woff2, and every app renders in the fallback stack. `*` grants a read of
/// public bytes and forbids credentials. The layer also answers a preflight,
/// which a GET-only route would 405 (ADR 0289).
///
/// The catalog is registered after the layer, so it gets no grant: an app reads
/// it through the host bridge like any other route.
///
/// A workspace font is not served here. This tree needs no credential, and a
/// workspace font is the user's file, so it goes through the gated `/data`
/// mount (ADR 0308).
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/fonts/:file", get(serve_font_file))
        .layer(font_file_grant())
        .route("/fonts", get(font_catalog))
}

/// The cross-origin grant on the font files, described above.
fn font_file_grant() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([axum::http::Method::GET, axum::http::Method::HEAD])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vendored_ids() -> Vec<&'static str> {
        FONT_CATALOG
            .iter()
            .filter(|f| !f.faces().is_empty())
            .map(|f| f.id)
            .collect()
    }

    /// Every stylesheet and every woff2 an app frame may load.
    fn font_paths() -> Vec<String> {
        let mut paths = Vec::new();
        for font in FONT_CATALOG {
            if font.faces().is_empty() {
                continue;
            }
            paths.push(format!("/fonts/{}.css", font.id));
            for face in font.faces() {
                paths.push(format!("/fonts/{}", face.served));
            }
        }
        paths
    }

    /// The `url()` must stay relative. An absolute `/api/v1/fonts/...` resolves
    /// against the ORIGIN, so it would 404 on every workspace behind a gateway
    /// prefix while working on a root-mounted dev engine.
    #[test]
    fn every_font_url_is_relative_to_the_stylesheet() {
        for id in vendored_ids() {
            let css = stylesheet(id).unwrap();
            assert!(!css.contains("url('/"), "{id}: {css}");
        }
    }

    /// A UA that does not recognise a `format()` value SKIPS that source, so a
    /// non-standard `woff2-variations` would silently never load. Plain `woff2`
    /// is what every browser with variable-font support reads.
    #[test]
    fn every_face_declares_a_format_every_browser_knows() {
        for id in vendored_ids() {
            let css = stylesheet(id).unwrap();
            for src in css.lines().filter(|l| l.trim_start().starts_with("src:")) {
                assert!(src.contains("format('woff2')"), "{id}: {src}");
                assert!(!src.contains("woff2-variations"), "{id}: {src}");
            }
        }
    }

    /// A variable file declares its weight range, and a static file its one
    /// weight. A single weight on a variable file would make the browser
    /// synthesise bold instead of using the real one.
    #[test]
    fn every_face_declares_its_weight_range_or_its_static_weight() {
        for id in vendored_ids() {
            let css = stylesheet(id).unwrap();
            for weight in css
                .lines()
                .filter(|l| l.trim_start().starts_with("font-weight:"))
            {
                let value = weight
                    .trim()
                    .trim_start_matches("font-weight:")
                    .trim_end_matches(';')
                    .trim();
                let numbers: Vec<&str> = value.split(' ').collect();
                assert!(
                    matches!(numbers.len(), 1 | 2)
                        && numbers.iter().all(|n| n.parse::<u16>().is_ok()),
                    "{id}: {weight}"
                );
            }
        }
        assert!(stylesheet("fira-code")
            .unwrap()
            .contains("font-weight: 300 700;"));
    }

    /// A font with no variable version must declare every weight it ships,
    /// or the browser synthesises the missing ones.
    #[test]
    fn a_static_font_declares_each_weight_it_ships() {
        let css = stylesheet("ibm-plex-mono").unwrap();
        for weight in ["400", "500", "600", "700"] {
            assert!(css.contains(&format!("font-weight: {weight};")), "{css}");
        }
        let css = stylesheet("commit-mono").unwrap();
        for weight in ["400", "700"] {
            assert!(css.contains(&format!("font-weight: {weight};")), "{css}");
        }
    }

    /// The bytes may promise a year only because their URL carries the version.
    #[test]
    fn only_the_versioned_bytes_are_cached_immutably() {
        assert!(WOFF2_CACHE_CONTROL.contains("immutable"));
        assert!(
            !CSS_CACHE_CONTROL.contains("immutable"),
            "the stylesheet is the mutable pointer that makes the above safe"
        );
    }

    /// A device font has no stylesheet here. Serving an empty one would hide
    /// a caller that linked the wrong thing.
    #[test]
    fn only_a_vendored_font_has_a_stylesheet() {
        assert!(stylesheet("system").is_none());
        assert!(stylesheet("monospace").is_none());
        assert!(stylesheet("nope").is_none());
        assert!(stylesheet("inter").is_some());
    }

    async fn answer(method: &str, path: &str, extra: &[(&str, &str)]) -> Response {
        use tower::ServiceExt as _;
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header(header::ORIGIN, "null");
        for (name, value) in extra {
            request = request.header(*name, *value);
        }
        // The file routes as `router` builds them. The catalog needs the
        // workspace state, and the e2e suite covers it.
        Router::new()
            .route("/fonts/:file", get(serve_font_file))
            .layer(font_file_grant())
            .oneshot(request.body(axum::body::Body::empty()).unwrap())
            .await
            .expect("the router answers")
    }

    /// The URLs app frames already load keep working.
    #[tokio::test]
    async fn the_fira_code_urls_are_unchanged() {
        for path in ["/fonts/fira-code.css", "/fonts/fira-code-6.2.woff2"] {
            assert_eq!(answer("GET", path, &[]).await.status(), StatusCode::OK);
        }
        let css = stylesheet("fira-code").unwrap();
        assert!(css.contains("url('fira-code-6.2.woff2')"), "{css}");
    }

    /// Every url() a stylesheet names is a file this router serves. Drift is a
    /// 404 for the font and an app rendered in the fallback.
    #[tokio::test]
    async fn every_stylesheet_names_files_the_router_serves() {
        for path in font_paths() {
            let response = answer("GET", &path, &[]).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let expected = if path.ends_with(".css") {
                "text/css; charset=utf-8"
            } else {
                "font/woff2"
            };
            assert_eq!(
                response.headers().get(header::CONTENT_TYPE).unwrap(),
                expected,
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn an_unknown_file_is_not_found() {
        for path in [
            "/fonts/nope.css",
            "/fonts/system.css",
            "/fonts/fira-code-6.1.woff2",
            "/fonts/FiraCode-VF.woff2",
        ] {
            assert_eq!(
                answer("GET", path, &[]).await.status(),
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
    }

    /// An app frame's origin is `null`, and a font fetch is always CORS-mode.
    #[tokio::test]
    async fn an_opaque_origin_may_read_every_font_file() {
        for path in font_paths() {
            let response = answer("GET", &path, &[]).await;
            assert_eq!(
                response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
                Some(&HeaderValue::from_static("*")),
                "{path}"
            );
        }
    }

    /// Without a layer to answer it, a preflight reaches a GET-only route and
    /// gets 405, which the browser reads as a refusal of the font.
    #[tokio::test]
    async fn a_preflight_is_answered_rather_than_refused() {
        for path in font_paths() {
            let response = answer(
                "OPTIONS",
                &path,
                &[("access-control-request-method", "GET")],
            )
            .await;
            assert!(
                response.status().is_success(),
                "{path}: {}",
                response.status()
            );
            let headers = response.headers();
            assert_eq!(
                headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
                Some(&HeaderValue::from_static("*")),
                "{path}"
            );
            let methods = headers
                .get(header::ACCESS_CONTROL_ALLOW_METHODS)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            assert!(methods.contains("GET"), "{path}: {methods}");
        }
    }

    /// `*` is only safe because it forbids a credentialed read.
    #[tokio::test]
    async fn the_grant_never_covers_credentials() {
        for path in font_paths() {
            let get = answer("GET", &path, &[]).await;
            let preflight = answer(
                "OPTIONS",
                &path,
                &[("access-control-request-method", "GET")],
            )
            .await;
            for response in [get, preflight] {
                assert!(
                    response
                        .headers()
                        .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
                        .is_none(),
                    "{path}"
                );
            }
        }
    }
}
