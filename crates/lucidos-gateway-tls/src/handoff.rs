//! What a direct client gets over plain HTTP on loopback (ADR 0745).
//!
//! Only three answers, and none reads a cookie or calls an API:
//!
//! - a navigation gets the hand-off page, which carries this origin's unsent
//!   work to the https origin (`public/outbox-handoff.js`);
//! - a same-origin `POST` to [`CLEAR_PATH`] gets `Clear-Site-Data`, for this
//!   http origin alone;
//! - everything else gets a `301` to `https://localhost:<port><path>`.
//!
//! Only plain loopback serves the clear endpoint. On an https origin it would
//! let any page wipe the outbox.

use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::Peer;

/// The clear endpoint. Equal to `names.clearPath` in `outbox-handoff.js`.
pub(crate) const CLEAR_PATH: &str = "/~/outbox-handoff/clear";

/// The export script, embedded so the page needs no static directory.
const OUTBOX_HANDOFF_JS: &str = include_str!("../../lucidos-app/public/outbox-handoff.js");

/// The page loads nothing: its script and style are inline, and its one
/// request is the same-origin clear.
const PAGE_POLICY: &str = "default-src 'none'; script-src 'unsafe-inline'; \
                           style-src 'unsafe-inline'; connect-src 'self'";

/// Scheme and host of the origin every answer points to.
const HTTPS_LOOPBACK: &str = "https://localhost";

pub(crate) fn respond(req: &Request) -> Response {
    let Some(peer) = req.extensions().get::<Peer>() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let headers = req.headers();
    if req.uri().path() == CLEAR_PATH && req.method() == Method::POST && same_origin(headers) {
        return (
            StatusCode::OK,
            [
                (header::CACHE_CONTROL, "no-store"),
                (
                    header::HeaderName::from_static("clear-site-data"),
                    "\"cache\", \"storage\"",
                ),
            ],
        )
            .into_response();
    }
    if is_navigation(req.method(), headers) {
        return (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
                (header::CONTENT_SECURITY_POLICY, PAGE_POLICY),
            ],
            page(),
        )
            .into_response();
    }
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let location = format!("{HTTPS_LOOPBACK}:{}{path}", peer.local.port());
    match HeaderValue::try_from(location) {
        Ok(location) => (
            StatusCode::MOVED_PERMANENTLY,
            [
                (header::LOCATION, location),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
        )
            .into_response(),
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    }
}

/// A browser marks a same-origin fetch with `Sec-Fetch-Site`. A request
/// without it (an old browser, or not a browser) clears nothing.
fn same_origin(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-site")
        .is_some_and(|site| site == "same-origin")
}

/// A top-level page load: `Sec-Fetch-Dest: document`, or an `Accept` asking
/// for HTML from a browser too old to send it.
fn is_navigation(method: &Method, headers: &HeaderMap) -> bool {
    if method != Method::GET && method != Method::HEAD {
        return false;
    }
    match headers.get("sec-fetch-dest") {
        Some(dest) => dest == "document",
        None => headers
            .get(header::ACCEPT)
            .and_then(|accept| accept.to_str().ok())
            .is_some_and(|accept| accept.contains("text/html")),
    }
}

fn page() -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Lucidos</title>
<style>
:root {{ color-scheme: light dark; font-family: system-ui, sans-serif; }}
body {{ margin: 0; min-height: 100vh; display: grid; place-items: center; }}
main {{ display: grid; gap: 1rem; justify-items: center; max-width: 28rem; padding: 2rem; text-align: center; }}
.action-btn {{ font: inherit; padding: 0.5rem 1.25rem; border-radius: 0.5rem; cursor: pointer; }}
</style>
</head>
<body>
<main><p class="boot-splash-status">Moving Lucidos to a secure address…</p></main>
<script>{OUTBOX_HANDOFF_JS}</script>
<script>window.lucidosOutboxHandoff.start();</script>
</body>
</html>
"#
    )
}
