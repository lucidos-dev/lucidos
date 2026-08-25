//! The engine's same-origin gate: what a *browser* may ask the engine to do.
//!
//! Full design:
//! `docs/plans/2026-08-19-webhooks-and-engines-off-the-network.md`.
//!
//! # This is not an authorizer
//!
//! Nothing here decides *who* is calling. The gateway owns that (ADR 0094), and
//! an engine binds loopback, so its callers are processes on this machine. The
//! one caller it cannot otherwise tell apart is a page on another origin,
//! driving that loopback port out of the user's own browser. CORS does not stop
//! such a request. It only stops the page reading the reply.
//!
//! So the question is narrow: did this come from our own origin?
//! `Sec-Fetch-Site` answers it unforgeably, since a browser sets it and page
//! JavaScript cannot. A caller with no fetch metadata is not a browser, and is
//! left to the bind topology.

//! # An app iframe passes, deliberately
//!
//! Apps are served from the engine's own origin, so an SDK call from inside one
//! reads as `same-origin`. That is the shipped contract: apps keep the user's
//! authority, and the ADR 0014 residual stays open. The gateway's control plane
//! also refuses an app-iframe `Referer`, and copying that here would break
//! every app.

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

/// May this request proceed, as far as browser origin goes?
///
/// `Sec-Fetch-Site` decides when present. It is a browser-set
/// [forbidden header](https://developer.mozilla.org/en-US/docs/Glossary/Forbidden_header_name)
/// that page JavaScript cannot forge, so no host reconstruction is needed. That
/// is what lets a same-origin app through with no usable `Host` to compare
/// `Origin` against, as with the direct-to-engine HTTP/2 PWA. HTTP/2 carries the
/// authority in `:authority`, so no `Host` header exists at all. Every current
/// browser sends fetch metadata (Chrome 76+, Firefox 90+, Safari 16.4+).
///
/// A browser old enough to omit it still sends `Origin`, which falls back to the
/// legacy `Origin == Host` comparison. That fallback holds **direct-to-engine**
/// only, where `Host` is the real client authority. Behind the gateway `Host` is
/// the internal upstream address, and no `x-forwarded-host` is injected. So a
/// no-fetch-metadata browser behind the gateway is deliberately unsupported, an
/// accepted and shrinking-population limitation
/// (`docs/plans/2026-07-22-credentialed-proxy-sec-fetch-authoritative.md`).
pub fn browser_request_allowed(headers: &HeaderMap) -> bool {
    let sec_fetch_site = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .map(str::to_ascii_lowercase);
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());

    // Sec-Fetch-Site present → it decides, unforgeably. `same-origin` / `none`
    // are safe; `same-site` / `cross-site` are foreign pages → reject.
    if let Some(site) = sec_fetch_site.as_deref() {
        return matches!(site, "same-origin" | "none");
    }

    // No fetch metadata. Either a non-browser client (also no Origin) → allow,
    // the bind topology is its boundary, or a legacy pre-fetch-metadata browser
    // that still sends `Origin`. Those are HTTP/1.1, so a plain `Host` header is
    // present; fall back to the same-origin comparison.
    let Some(origin) = origin else {
        return true;
    };
    let Some(host) = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    origin_authority_matches_host(origin, host)
}

/// Whether `origin` and `host` name the same authority, with the scheme's
/// default port filled in so `https://localhost` matches `localhost:443`.
pub(super) fn origin_authority_matches_host(origin: &str, host: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(origin) else {
        return false;
    };
    let Some(origin_host) = url.host_str() else {
        return false;
    };
    let origin_port = url.port_or_known_default();
    let origin_authority = match origin_port {
        Some(port) => format!("{origin_host}:{port}"),
        None => origin_host.to_string(),
    };
    let host = host.trim();
    origin_authority.eq_ignore_ascii_case(host) || origin_host.eq_ignore_ascii_case(host)
}

/// Refuse a browser request that came from another origin.
///
/// Layered over the whole `/api/v1` surface, so it runs before routing, and
/// before any handler resolves a credential or touches the database.
pub async fn enforce(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if browser_request_allowed(req.headers()) {
        return next.run(req).await;
    }
    // A silent 403 in front of every route is undebuggable from the client
    // side: a refused SSE stream just never connects. Say which request and
    // what it presented, since all three values are the whole decision.
    crate::log!(
        "[API] refused a cross-origin browser request to {} (sec-fetch-site: {:?}, origin: {:?})",
        req.uri().path(),
        req.headers().get("sec-fetch-site"),
        req.headers().get(axum::http::header::ORIGIN),
    );
    (
        StatusCode::FORBIDDEN,
        "cross-origin browser requests are not allowed",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    fn hm(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn a_client_with_no_fetch_metadata_is_not_a_browser_and_passes() {
        // The CLI, the python shim, a coding-agent session, an e2e run, and the
        // gateway's own hop. Their boundary is the loopback bind, not this.
        assert!(browser_request_allowed(&HeaderMap::new()));
        assert!(browser_request_allowed(&hm(&[("user-agent", "curl/8.0")])));
    }

    #[test]
    fn our_own_page_passes() {
        let h = hm(&[
            ("host", "localhost:5251"),
            ("origin", "http://localhost:5251"),
            ("sec-fetch-site", "same-origin"),
        ]);
        assert!(browser_request_allowed(&h));
    }

    #[test]
    fn a_top_level_navigation_passes() {
        // `none` is a typed URL or a bookmark. It has no initiating page to be
        // cross-origin from.
        assert!(browser_request_allowed(&hm(&[("sec-fetch-site", "none")])));
    }

    #[test]
    fn an_app_iframe_passes() {
        // Apps are same-origin with the engine and call the API through the
        // SDK. Refusing them here would break every app.
        let h = hm(&[
            ("host", "localhost:5173"),
            ("origin", "https://localhost:5173"),
            ("referer", "https://localhost:5173/app/habit-tracker/"),
            ("sec-fetch-site", "same-origin"),
        ]);
        assert!(browser_request_allowed(&h));
    }

    #[test]
    fn a_foreign_page_is_refused() {
        let h = hm(&[
            ("host", "localhost:5251"),
            ("origin", "https://evil.example"),
            ("sec-fetch-site", "cross-site"),
        ]);
        assert!(!browser_request_allowed(&h));
    }

    #[test]
    fn another_port_on_this_machine_is_refused() {
        // The reason this gate exists. `same-site` covers a page on another
        // port of localhost, which is the attack a loopback engine is open to.
        let h = hm(&[
            ("host", "localhost:5173"),
            ("origin", "http://localhost:5252"),
            ("sec-fetch-site", "same-site"),
        ]);
        assert!(!browser_request_allowed(&h));
    }

    #[test]
    fn fetch_metadata_wins_over_a_mismatched_origin() {
        // Behind the gateway the engine's Host is the internal address, so
        // Origin != Host is normal. Sec-Fetch-Site decides and no host
        // comparison runs.
        let h = hm(&[
            ("host", "127.0.0.1:51811"),
            ("origin", "https://localhost:5251"),
            ("sec-fetch-site", "same-origin"),
        ]);
        assert!(browser_request_allowed(&h));
    }

    #[test]
    fn http2_same_origin_passes_with_no_host_header() {
        // A direct-to-engine HTTP/2 request (the iOS PWA) carries its authority
        // in `:authority`, so there is no Host header to reconstruct from.
        let h = hm(&[
            ("origin", "https://localhost:5174"),
            ("sec-fetch-site", "same-origin"),
        ]);
        assert!(browser_request_allowed(&h));
    }

    #[test]
    fn a_legacy_browser_falls_back_to_origin_against_host() {
        let same = hm(&[
            ("host", "localhost:5173"),
            ("origin", "http://localhost:5173"),
        ]);
        assert!(browser_request_allowed(&same));

        let foreign = hm(&[
            ("host", "localhost:5173"),
            ("origin", "https://evil.example"),
        ]);
        assert!(!browser_request_allowed(&foreign));

        // Origin with nothing to compare against is refused rather than
        // guessed. A legacy browser always sends Host.
        let hostless = hm(&[("origin", "https://evil.example")]);
        assert!(!browser_request_allowed(&hostless));
    }

    #[test]
    fn a_default_port_is_filled_in_before_comparing() {
        assert!(origin_authority_matches_host(
            "https://localhost",
            "localhost:443"
        ));
        assert!(origin_authority_matches_host(
            "https://localhost",
            "localhost"
        ));
        assert!(origin_authority_matches_host(
            "http://LOCALHOST:80",
            "localhost:80"
        ));
        assert!(origin_authority_matches_host(
            "http://LOCALHOST:80",
            "localhost"
        ));
        assert!(!origin_authority_matches_host(
            "https://localhost",
            "localhost:444"
        ));
    }
}
