use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

pub fn browser_request_allowed(headers: &HeaderMap) -> bool {
    let sec_fetch_site = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .map(str::to_ascii_lowercase);
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());

    if let Some(site) = sec_fetch_site.as_deref() {
        return matches!(site, "same-origin" | "none");
    }

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
    if origin_authority.eq_ignore_ascii_case(host) {
        return true;
    }
    url.scheme() == "https" && url.port().is_none() && origin_host.eq_ignore_ascii_case(host)
}

fn is_public_app_asset(path: &str) -> bool {
    let path = path.strip_prefix("/api/v1").unwrap_or(path);
    matches!(
        path,
        "/sdk.js" | "/sdk-prefs.js" | "/sdk-iframe.css" | "/sdk-iframe-audio.js"
    ) || path.starts_with("/fonts/")
}

/// INVARIANT: Refuse a browser request that came from another origin.
///
/// Layered over the whole `/api/v1` surface, so it runs before routing, and
/// before any handler resolves a credential or touches the database.
pub async fn enforce(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if is_public_app_asset(req.uri().path()) || browser_request_allowed(req.headers()) {
        return next.run(req).await;
    }
    crate::log_legacy!(
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
        assert!(browser_request_allowed(&hm(&[("sec-fetch-site", "none")])));
    }

    #[test]
    fn an_app_in_its_own_tab_passes() {
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
        let h = hm(&[
            ("host", "localhost:5173"),
            ("origin", "http://localhost:5252"),
            ("sec-fetch-site", "same-site"),
        ]);
        assert!(!browser_request_allowed(&h));
    }

    #[test]
    fn fetch_metadata_wins_over_a_mismatched_origin() {
        let h = hm(&[
            ("host", "127.0.0.1:51811"),
            ("origin", "https://localhost:5251"),
            ("sec-fetch-site", "same-origin"),
        ]);
        assert!(browser_request_allowed(&h));
    }

    #[test]
    fn http2_same_origin_passes_with_no_host_header() {
        let h = hm(&[
            ("origin", "https://localhost:5174"),
            ("sec-fetch-site", "same-origin"),
        ]);
        assert!(browser_request_allowed(&h));
    }

    #[test]
    fn a_handshake_direct_to_the_engine_is_judged_by_its_origin() {
        let ours = hm(&[
            ("host", "localhost:5173"),
            ("origin", "https://localhost:5173"),
            ("connection", "keep-alive, Upgrade"),
            ("upgrade", "websocket"),
        ]);
        assert!(browser_request_allowed(&ours));

        let another_port = hm(&[
            ("host", "localhost:5173"),
            ("origin", "http://localhost:5252"),
            ("connection", "keep-alive, Upgrade"),
            ("upgrade", "websocket"),
        ]);
        assert!(!browser_request_allowed(&another_port));
    }

    #[test]
    fn a_handshake_behind_the_gateway_is_not_this_gates_to_judge() {
        let wrong_authority = hm(&[
            ("host", "127.0.0.1:5173"),
            ("origin", "https://localhost:5251"),
        ]);
        assert!(!browser_request_allowed(&wrong_authority));

        let judged_already = hm(&[("host", "127.0.0.1:5173")]);
        assert!(browser_request_allowed(&judged_already));
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
        assert!(!origin_authority_matches_host(
            "https://localhost",
            "localhost:444"
        ));
    }

    #[test]
    fn a_portless_host_matches_the_default_port_and_no_other() {
        assert!(!origin_authority_matches_host(
            "https://name.ts.net:10000",
            "name.ts.net"
        ));
        assert!(origin_authority_matches_host(
            "https://name.ts.net",
            "name.ts.net"
        ));
        assert!(origin_authority_matches_host(
            "https://name.ts.net:443",
            "name.ts.net"
        ));
        assert!(!origin_authority_matches_host(
            "http://name.ts.net",
            "name.ts.net"
        ));
        assert!(!origin_authority_matches_host(
            "http://name.ts.net:443",
            "name.ts.net"
        ));
    }

    #[test]
    fn an_app_frames_own_subresources_are_exempt() {
        for path in [
            "/sdk.js",
            "/sdk-prefs.js",
            "/sdk-iframe.css",
            "/sdk-iframe-audio.js",
            "/fonts/fira-code.css",
        ] {
            assert!(is_public_app_asset(path), "{path}");
            assert!(
                is_public_app_asset(&format!("/api/v1{path}")),
                "/api/v1{path}"
            );
        }
    }

    #[test]
    fn nothing_carrying_workspace_data_is_exempt() {
        for path in [
            "/threads/list",
            "/data",
            "/preferences",
            "/credentials",
            "/events/emit",
            "/sdk.js.map",
            "/fonts",
        ] {
            assert!(!is_public_app_asset(path), "{path}");
        }
    }
}
