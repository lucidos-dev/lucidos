//! Which of an app frame's own files it may load across its opaque origin.
//!
//! An app frame runs at an opaque origin (ADR 0227), so its own `/app/<id>/…`
//! and `/data/…` files are cross-origin to it. A font and a module script are
//! CORS-mode loads, and Chromium refuses both without a grant. A plain `*` is
//! out: direct to a loopback engine `/data/` has no gate, so any web page could
//! `fetch()` the user's files.
//!
//! So the grant follows `Sec-Fetch-Dest`, which page script cannot forge. A
//! font load hands the page no bytes, and a `fetch()` is `empty`. A module
//! hands its caller its exports, so a `script` grant needs a gate in front: it
//! is given only to a request the gateway forwarded (ADR 0289).
//!
//! Two headers ride with a grant, and each is load-bearing:
//!
//! - **`Vary: Sec-Fetch-Dest`.** Without it Chrome serves a later `fetch()`
//!   from the copy a font load cached, grant included, and the page reads it.
//! - **`nosniff`.** The grant unmasks a `<script crossorigin>`'s errors. Without
//!   `nosniff` a text file runs as a script, and its `SyntaxError` quotes it.

use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// May this load cross the frame's origin?
///
/// A page cannot set `X-Forwarded-Prefix` on a load, so its presence means the
/// gateway forwarded the request after checking the device or the frame
/// capability. A non-browser client can forge it, and gains nothing: CORS never
/// bound such a client. Everything else is refused, including `style` (the
/// CSSOM reads it) and `image` (a canvas reads it).
fn is_granted(headers: &HeaderMap) -> bool {
    match headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()) {
        Some("font") => true,
        Some("script") => super::base_path::forwarded_prefix(headers) != "/",
        _ => false,
    }
}

/// Grant a font or gated script load across origins, and say what the answer
/// varied on.
pub(super) async fn grant_font_and_script_loads(request: Request, next: Next) -> Response {
    let granted = is_granted(request.headers());
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.append(header::VARY, HeaderValue::from_static("sec-fetch-dest"));
    if granted {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt as _;

    /// How the request reached the engine.
    #[derive(Clone, Copy, Debug)]
    enum Via {
        Gateway,
        Direct,
    }

    async fn answer(dest: Option<&str>, via: Via) -> Response {
        let app = axum::Router::new()
            .route(
                "/app/habit-tracker/main.js",
                axum::routing::get(|| async { "ok" }),
            )
            .layer(axum::middleware::from_fn(grant_font_and_script_loads));
        let mut request = axum::http::Request::builder()
            .uri("/app/habit-tracker/main.js")
            .header(header::ORIGIN, "null");
        if let Some(dest) = dest {
            request = request.header("sec-fetch-dest", dest);
        }
        if let Via::Gateway = via {
            request = request.header("x-forwarded-prefix", "/dev/");
        }
        app.oneshot(request.body(axum::body::Body::empty()).unwrap())
            .await
            .expect("the router answers")
    }

    fn allow_origin(response: &Response) -> Option<&HeaderValue> {
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
    }

    fn assert_granted(response: &Response, what: &str) {
        assert_eq!(
            allow_origin(response),
            Some(&HeaderValue::from_static("*")),
            "{what}"
        );
        assert_eq!(
            response.headers().get(header::X_CONTENT_TYPE_OPTIONS),
            Some(&HeaderValue::from_static("nosniff")),
            "a grant must not let a text file run as a script: {what}"
        );
    }

    fn varies_on_destination(response: &Response) -> bool {
        response.headers().get_all(header::VARY).iter().any(|v| {
            v.to_str()
                .is_ok_and(|v| v.eq_ignore_ascii_case("sec-fetch-dest"))
        })
    }

    #[tokio::test]
    async fn a_font_loads_across_origins_however_it_arrived() {
        for via in [Via::Gateway, Via::Direct] {
            assert_granted(&answer(Some("font"), via).await, &format!("{via:?}"));
        }
    }

    #[tokio::test]
    async fn a_module_script_loads_across_origins_behind_the_gateway() {
        assert_granted(&answer(Some("script"), Via::Gateway).await, "script");
    }

    /// A direct hit has no gate in front of `/data/`. An `import()` there would
    /// hand any web page the exports of the user's modules.
    #[tokio::test]
    async fn a_script_hitting_the_engine_directly_is_not_granted() {
        assert_eq!(
            allow_origin(&answer(Some("script"), Via::Direct).await),
            None
        );
    }

    /// Every one of these hands the page something it can read, gate or not.
    /// `empty` is a `fetch()`, which is the whole threat on an ungated `/data/`.
    #[tokio::test]
    async fn nothing_that_hands_the_page_bytes_is_granted() {
        for via in [Via::Gateway, Via::Direct] {
            for dest in [
                None,
                Some("empty"),
                Some("style"),
                Some("image"),
                Some("json"),
                Some("iframe"),
            ] {
                let response = answer(dest, via).await;
                assert_eq!(allow_origin(&response), None, "{dest:?} {via:?}");
            }
        }
    }

    /// A cached refusal must not stand in for a grant. The reverse is the read
    /// the module doc describes.
    #[tokio::test]
    async fn every_answer_varies_on_the_destination() {
        for dest in [None, Some("empty"), Some("font"), Some("script")] {
            let response = answer(dest, Via::Direct).await;
            assert!(varies_on_destination(&response), "{dest:?}");
        }
    }

    /// Browsers send the token lowercase. A spelling nobody sends is refused
    /// rather than matched loosely.
    #[tokio::test]
    async fn the_destination_is_matched_exactly() {
        assert_eq!(allow_origin(&answer(Some("Font"), Via::Direct).await), None);
        assert_eq!(
            allow_origin(&answer(Some("font, empty"), Via::Direct).await),
            None
        );
    }
}
