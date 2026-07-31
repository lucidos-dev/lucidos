//! Base-path stamping for the workspace gateway (ADR 0014 §4).
//!
//! Behind the gateway the engine is reached under a path prefix (`/<slug>/`).
//! The gateway is a pure streaming proxy — it strips `/<slug>` and forwards an
//! `X-Forwarded-Prefix: /<slug>/` request header, never touching response
//! bodies. The ONE adjustment a workspace-prefixed page needs therefore lives
//! here, in the engine that serves the page:
//!
//!  * The SPA shell (`index.html`) gets `<base href="/<slug>/">` stamped in, so
//!    its relative asset refs (`./assets/…`, Vite `base: './'`) resolve back
//!    through the gateway to this workspace's engine.
//!  * App-UI pages keep their absolute engine refs (`<script src="/api/v1/…">`,
//!    the documented SDK contract) but those get **re-scoped** with the prefix
//!    (see [`super::app_ui::rescope_app_html`]) — base-href can't fix absolute
//!    URLs, which ignore it.
//!
//! Hit directly (no gateway, `LUCIDOS_NO_GATEWAY`), there is no
//! `X-Forwarded-Prefix`, so the prefix is `/` and both adjustments are no-ops —
//! one engine serves both access modes correctly, per request.

use axum::http::HeaderMap;

/// The forwarded path prefix this request arrived under: `/<slug>/` behind the
/// gateway (from `X-Forwarded-Prefix`), else `/` (direct hit). Always begins and
/// ends with `/`. A malformed header value falls back to `/`.
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

/// Insert `<base href="…">` as the first child of `<head>` so every relative ref
/// in the document resolves against it. A `prefix` of `/` is a no-op (direct
/// access needs no base). Falls back to prepending when there is no `<head>`.
pub fn inject_base_href(html: &str, prefix: &str) -> String {
    if prefix == "/" {
        return html.to_string();
    }
    insert_into_head(html, &format!("<base href=\"{prefix}\">"))
}

/// The workspace gateway's external port, from `LUCIDOS_GATEWAY_PORT` — set by
/// the gateway when it spawns this engine (ADR 0014, `lucidos-gateway` `stack.rs`).
/// `None` for a legacy no-gateway engine (`LUCIDOS_NO_GATEWAY`, e2e direct-engine),
/// which has no gateway picker to address.
pub fn gateway_port() -> Option<String> {
    std::env::var("LUCIDOS_GATEWAY_PORT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Insert `<meta name="lucidos-gateway-port" content="…">` as the first child of
/// `<head>`. This lets a page served on the engine's OWN port (direct access,
/// `<base href="/">`) build an absolute URL to the gateway picker — whose origin
/// (port) differs from the engine's, so the relative `/~/` picker route can't
/// reach it. Behind the gateway the relative route already works, so the page
/// ignores this; it is harmless to stamp in both cases. `None` is a no-op (no
/// gateway). Falls back to prepending when there is no `<head>`.
pub fn inject_gateway_port(html: &str, port: Option<&str>) -> String {
    match port {
        None => html.to_string(),
        Some(port) => insert_into_head(
            html,
            &format!("<meta name=\"lucidos-gateway-port\" content=\"{port}\">"),
        ),
    }
}

/// This workspace's gateway slug, from `LUCIDOS_WORKSPACE_ID`, which the gateway
/// sets when it spawns this engine (`lucidos-gateway` `stack.rs`). `None` for an
/// engine no gateway launched, which has no slug to be addressed by.
pub fn workspace_id() -> Option<String> {
    std::env::var("LUCIDOS_WORKSPACE_ID")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Insert `<meta name="lucidos-workspace-id" content="…">` as the first child of
/// `<head>`. Paired with [`inject_gateway_port`], this is what lets a page served
/// on the engine's OWN port address ITSELF on the gateway
/// (`https://<host>:<port>/<slug>/`) instead of only reaching the picker.
///
/// The reason that matters: a direct-port document has no `<base>` element at all
/// (see [`inject_base_href`], a no-op for `/`), so the slug is otherwise nowhere
/// in the document, and a navigation to `/<slug>/` is exactly what makes the
/// gateway lazy-start a STOPPED workspace (ADR 0014 §11). So a page whose engine
/// has died can still offer a working way in, from cached HTML, with no engine
/// and no application bundle to help it.
///
/// The value is HTML-escaped. The gateway sets it from a registry slug and
/// nothing else writes it, but it reaches us through an env var and lands in an
/// attribute, so it is escaped here rather than trusted at the boundary.
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

/// Minimal HTML attribute escaping for a double-quoted value.
fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Insert `tag` as the first child of `<head>`; prepend it when there is no
/// `<head>`. Shared by the base-href, gateway-port and workspace-id stampers.
fn insert_into_head(html: &str, tag: &str) -> String {
    match find_head_open_end(html) {
        Some(pos) => {
            let mut out = String::with_capacity(html.len() + tag.len());
            out.push_str(&html[..pos]);
            out.push_str(tag);
            out.push_str(&html[pos..]);
            out
        }
        None => format!("{tag}{html}"),
    }
}

/// Byte offset just past the opening `<head …>` tag, case-insensitively.
fn find_head_open_end(html: &str) -> Option<usize> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<head")?;
    let close = lower[start..].find('>')? + start;
    Some(close + 1)
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

    /// The value lands in a double-quoted attribute. A slug never contains these
    /// characters, but the value arrives through an env var, so a hostile or
    /// merely malformed one must not break out of the attribute.
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

    /// A direct-port document is the whole point of the pair: it gets NO base
    /// element, so these two metas are the only way it can address itself on the
    /// gateway and lazy-start a stopped workspace.
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
        // serve_shell stamps base href, then gateway port — both land as the
        // first child of <head>, gateway port outermost (stamped last).
        let html = "<html><head><title>x</title></head></html>";
        let out = inject_gateway_port(&inject_base_href(html, "/dev/"), Some("5251"));
        assert!(out.contains(
            "<head><meta name=\"lucidos-gateway-port\" content=\"5251\"><base href=\"/dev/\"><title>x</title>"
        ));
    }
}
