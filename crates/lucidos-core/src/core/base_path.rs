use axum::http::HeaderMap;

pub fn forwarded_prefix(headers: &HeaderMap) -> String {
    let raw = headers
        .get("x-forwarded-prefix")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match raw {
        None => "/".to_string(),
        Some(p) => {
            let mut s = String::with_capacity(p.len() + 2);
            if !p.starts_with('/') {
                s.push('/');
            }
            s.push_str(p);
            if !s.ends_with('/') {
                s.push('/');
            }
            s
        }
    }
}

/// INVARIANT: Whether this request reached the engine THROUGH the workspace gateway's
/// proxy, rather than arriving at the engine's own port. Reads the same
/// `X-Forwarded-Prefix` header as [`forwarded_prefix`], but as a yes/no about
/// provenance rather than a value (that function's `/` default is deliberately
/// indistinguishable from a direct hit, which is right for stamping a base href
/// and wrong for deciding who is calling).
///
/// **This is a trust boundary, and it holds because the gateway is one.**
/// `lucidos-gateway`'s proxy STRIPS any client-supplied `x-forwarded-prefix` and
/// injects its own, so the header is present on every proxied request and
/// forgeable on none. A caller that carries it is therefore provably on the far
/// side of the gateway (in practice: a browser, including an app document),
/// and one that does not came straight to this engine's port, which
/// under the gateway is bound to loopback.
///
/// Used by the engine-internal routes that must not be reachable from a page.
/// A peer-address check cannot serve that purpose here: the gateway proxies
/// browser traffic to the engine over loopback, so a hostile page's request and
/// the gateway's own control call arrive from the same `127.0.0.1`.
pub fn arrived_through_gateway_proxy(headers: &HeaderMap) -> bool {
    headers.contains_key("x-forwarded-prefix")
}

pub fn inject_base_href(html: &str, prefix: &str) -> String {
    if prefix == "/" {
        return html.to_string();
    }
    insert_into_head(html, &format!("<base href=\"{}\">", escape_attr(prefix)))
}

pub fn gateway_port() -> Option<String> {
    std::env::var("LUCIDOS_GATEWAY_PORT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn inject_gateway_port(html: &str, port: Option<&str>) -> String {
    match port {
        None => html.to_string(),
        Some(port) => insert_into_head(
            html,
            &format!(
                "<meta name=\"lucidos-gateway-port\" content=\"{}\">",
                escape_attr(port)
            ),
        ),
    }
}

pub fn workspace_id() -> Option<String> {
    std::env::var("LUCIDOS_WORKSPACE_ID")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn api_port() -> Option<String> {
    std::env::var("LUCIDOS_API_PORT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn inject_workspace_id(html: &str, id: Option<&str>) -> String {
    match id {
        None => html.to_string(),
        Some(id) => insert_into_head(
            html,
            &format!(
                "<meta name=\"lucidos-workspace-id\" content=\"{}\">",
                escape_attr(id)
            ),
        ),
    }
}

pub fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn insert_into_head(html: &str, tag: &str) -> String {
    let at = head_insert_offset(html);
    let mut out = String::with_capacity(html.len() + tag.len());
    out.push_str(&html[..at]);
    out.push_str(tag);
    out.push_str(&html[at..]);
    out
}

/// INVARIANT: Byte offset the first head child belongs at, case-insensitively: just past
/// `<head …>`, else just past `<html …>`, else just past a leading doctype,
/// else the very start.
///
/// The fallbacks are for an app document the engine did not author. A tag ahead
/// of a doctype would drop the page into quirks mode. The parser opens a head
/// for a tag that arrives before the document's own.
fn head_insert_offset(html: &str) -> usize {
    static OPENERS: std::sync::LazyLock<[regex::Regex; 3]> = std::sync::LazyLock::new(|| {
        ["<head\\b[^>]*>", "<html\\b[^>]*>", "<!doctype\\b[^>]*>"]
            .map(|p| regex::Regex::new(&format!("(?i){p}")).expect("head opener regex"))
    });
    OPENERS
        .iter()
        .find_map(|re| re.find(html))
        .map_or(0, |m| m.end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with(prefix: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-prefix", HeaderValue::from_str(prefix).unwrap());
        h
    }

    #[test]
    fn forwarded_prefix_defaults_to_root() {
        assert_eq!(forwarded_prefix(&HeaderMap::new()), "/");
        assert_eq!(forwarded_prefix(&headers_with("")), "/");
    }

    #[test]
    fn forwarded_prefix_normalizes_slashes() {
        assert_eq!(forwarded_prefix(&headers_with("/dev/")), "/dev/");
        assert_eq!(forwarded_prefix(&headers_with("/dev")), "/dev/");
        assert_eq!(forwarded_prefix(&headers_with("dev")), "/dev/");
    }

    #[test]
    fn a_proxied_request_is_told_apart_from_a_direct_one() {
        assert!(arrived_through_gateway_proxy(&headers_with("/dev/")));
        assert!(!arrived_through_gateway_proxy(&HeaderMap::new()));
    }

    #[test]
    fn an_empty_forwarded_prefix_still_marks_the_request_as_proxied() {
        assert_eq!(forwarded_prefix(&headers_with("")), "/");
        assert!(arrived_through_gateway_proxy(&headers_with("")));
    }

    #[test]
    fn inject_base_href_after_head() {
        let html = "<html><head><meta charset=\"utf-8\"></head><body>x</body></html>";
        let out = inject_base_href(html, "/dev/");
        assert!(out.contains("<head><base href=\"/dev/\"><meta charset=\"utf-8\">"));
    }

    #[test]
    fn inject_base_href_root_is_noop() {
        let html = "<html><head></head></html>";
        assert_eq!(inject_base_href(html, "/"), html);
    }

    #[test]
    fn inject_base_href_prepends_without_head() {
        let html = "<p>x</p>";
        assert_eq!(
            inject_base_href(html, "/dev/"),
            "<base href=\"/dev/\"><p>x</p>"
        );
    }

    #[test]
    fn a_document_without_a_head_falls_back_outwards_and_never_past_the_doctype() {
        assert_eq!(
            insert_into_head(
                r#"<!DOCTYPE html><html lang="en"><body>x</body></html>"#,
                "<i>"
            ),
            r#"<!DOCTYPE html><html lang="en"><i><body>x</body></html>"#
        );
        assert_eq!(
            insert_into_head("<!DOCTYPE html><body><header>h</header></body>", "<i>"),
            "<!DOCTYPE html><i><body><header>h</header></body>"
        );
    }

    #[test]
    fn inject_gateway_port_after_head() {
        let html = "<html><head><meta charset=\"utf-8\"></head><body>x</body></html>";
        let out = inject_gateway_port(html, Some("5251"));
        assert!(out.contains(
            "<head><meta name=\"lucidos-gateway-port\" content=\"5251\"><meta charset=\"utf-8\">"
        ));
    }

    #[test]
    fn inject_gateway_port_none_is_noop() {
        let html = "<html><head></head></html>";
        assert_eq!(inject_gateway_port(html, None), html);
    }

    #[test]
    fn inject_gateway_port_prepends_without_head() {
        let html = "<p>x</p>";
        assert_eq!(
            inject_gateway_port(html, Some("5252")),
            "<meta name=\"lucidos-gateway-port\" content=\"5252\"><p>x</p>"
        );
    }

    #[test]
    fn inject_gateway_port_escapes_the_value() {
        let out = inject_gateway_port(
            "<html><head></head></html>",
            Some("5251\"><script>alert(1)</script>"),
        );
        assert!(!out.contains("<script>"), "{out}");
        assert!(out.contains("&quot;&gt;&lt;script&gt;"), "{out}");
    }

    #[test]
    fn inject_workspace_id_after_head() {
        let html = "<html><head><meta charset=\"utf-8\"></head><body>x</body></html>";
        let out = inject_workspace_id(html, Some("myws"));
        assert!(out.contains(
            "<head><meta name=\"lucidos-workspace-id\" content=\"myws\"><meta charset=\"utf-8\">"
        ));
    }

    #[test]
    fn inject_workspace_id_none_is_noop() {
        let html = "<html><head></head></html>";
        assert_eq!(inject_workspace_id(html, None), html);
    }

    #[test]
    fn inject_workspace_id_escapes_the_value() {
        let out = inject_workspace_id(
            "<html><head></head></html>",
            Some("a\"><script>alert(1)</script>&x"),
        );
        assert!(
            out.contains("content=\"a&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;&amp;x\">"),
            "{out}"
        );
        assert!(!out.contains("<script>"), "{out}");
    }

    #[test]
    fn inject_base_href_escapes_the_prefix() {
        let out = inject_base_href(
            "<html><head></head></html>",
            "/a\"><script>alert(1)</script><b c=\"",
        );
        assert!(!out.contains("<script>"), "{out}");
        assert!(out.contains("&quot;&gt;&lt;script&gt;"), "{out}");
    }

    #[test]
    fn direct_access_still_carries_both_metas_without_a_base() {
        let html = "<html><head><title>x</title></head></html>";
        let out = inject_workspace_id(
            &inject_gateway_port(&inject_base_href(html, "/"), Some("5251")),
            Some("myws"),
        );
        assert!(!out.contains("<base"), "{out}");
        assert!(out.contains("content=\"5251\""), "{out}");
        assert!(out.contains("content=\"myws\""), "{out}");
    }

    #[test]
    fn base_href_and_gateway_port_compose() {
        let html = "<html><head><title>x</title></head></html>";
        let out = inject_gateway_port(&inject_base_href(html, "/dev/"), Some("5251"));
        assert!(out.contains(
            "<head><meta name=\"lucidos-gateway-port\" content=\"5251\"><base href=\"/dev/\"><title>x</title>"
        ));
    }
}
