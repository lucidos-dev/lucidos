//! Reverse proxy: `/<slug>/*` → the workspace engine on `127.0.0.1:<port>`.
//!
//! A **pure strip-and-forward streaming proxy** (ADR 0014 §3). On the way in it
//! strips the `/<slug>` prefix (so the engine sees the root-relative paths it
//! already understands) and adds an `X-Forwarded-Prefix: /<slug>/` request
//! header. On the way out it forwards the response **untouched** — body streamed
//! both directions, compression intact, SSE flowing live. It does NOT read or
//! rewrite `text/html` (the engine stamps `<base href>` / rewrites app refs from
//! the forwarded prefix instead, ADR 0014 §4), does NOT strip `Accept-Encoding`,
//! and carries no speculative retry. This removes the root cause of the gzip-502
//! / empty-reply / lost-compression issues 0013 hit when the gateway read bodies
//! as text.
//!
//! Hop-by-hop headers (RFC 7230 §6.1) are stripped in both directions so the
//! gateway doesn't forward framing headers that conflict with hyper's own
//! connection management.

use axum::body::Body;
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use reqwest::Client;

/// Headers that are connection-specific and must not be forwarded by a proxy.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fn is_hop_by_hop(name: &HeaderName) -> bool {
    let n = name.as_str();
    HOP_BY_HOP.iter().any(|h| n.eq_ignore_ascii_case(h))
}

/// Strip a leading `/<slug>` from a path-and-query, returning the remainder
/// (always starting with `/`). Returns `None` when the path is exactly
/// `/<slug>` with no trailing slash — the caller redirects that to `/<slug>/`
/// so the engine's stamped `<base href="/<slug>/">` resolves.
pub fn strip_prefix<'a>(path_and_query: &'a str, slug: &str) -> Option<&'a str> {
    let prefix = format!("/{slug}");
    let rest = path_and_query.strip_prefix(&prefix)?;
    if rest.is_empty() {
        // "/<slug>" exactly — needs the trailing-slash redirect.
        None
    } else if rest.starts_with('/') {
        // "/<slug>/..." → "/..." (keep the leading slash).
        Some(rest)
    } else {
        // "/<slug>foo" — a different slug sharing the prefix (e.g. "/devx").
        None
    }
}

/// Proxy `req` to `target_base` (e.g. `http://127.0.0.1:51811`), stripping the
/// `/<slug>` prefix and adding `X-Forwarded-Prefix: /<slug>/`. `target_base` has
/// no trailing slash.
pub async fn proxy(
    client: &Client,
    target_base: &str,
    slug: &str,
    req: axum::extract::Request,
) -> Response {
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_string();

    let Some(rest) = strip_prefix(&path_and_query, slug) else {
        // "/<slug>" with no trailing slash → redirect so the SPA shell loads at
        // a path where the engine's injected <base href="/<slug>/"> resolves.
        return Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(header::LOCATION, format!("/{slug}/"))
            .body(Body::empty())
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    };

    let method = req.method().clone();
    let url = format!("{target_base}{rest}");

    // Forward request headers verbatim except HOST (reqwest sets it), the
    // framing headers (hop-by-hop + content-length — reqwest re-frames the
    // streamed body), and any inbound `x-forwarded-prefix`. The last is
    // LOAD-BEARING for security: the gateway is the trust boundary and sets this
    // header authoritatively below, but reqwest's `.header()` *appends* rather
    // than replaces — so a client-spoofed `X-Forwarded-Prefix` left in here would
    // sit FIRST, and the engine's `forwarded_prefix` reads `headers.get(...)`
    // (the first value). Stripping it makes the gateway's the only value.
    let mut builder = client.request(method.clone(), &url);
    for (name, value) in req.headers() {
        if name == header::HOST
            || name == header::CONTENT_LENGTH
            || name.as_str().eq_ignore_ascii_case("x-forwarded-prefix")
            || is_hop_by_hop(name)
        {
            continue;
        }
        builder = builder.header(name.as_str(), value);
    }
    let forwarded_prefix = format!("/{slug}/");
    builder = builder.header("x-forwarded-prefix", &forwarded_prefix);

    // Stream the request body straight through — never buffer it (a 100 MB
    // `PUT /data/*` upload must not sit in gateway memory).
    builder = builder.body(reqwest::Body::wrap_stream(req.into_body().into_data_stream()));

    let upstream = match builder.send().await {
        Ok(r) => r,
        Err(e) => {
            crate::log!("[Gateway] proxy {} -> {} failed: {}", path_and_query, url, e);
            // Boot-window UX (ADR 0014 §11): an engine cold boot (pgvector init,
            // migrations, embedding warmup, ~20 CC sessions resuming) can take
            // tens of seconds, during which a connect fails. Serve a lightweight
            // auto-retry page for navigations instead of a raw 502 the user must
            // reload by hand.
            return starting_page();
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut resp = Response::builder().status(status);
    for (name, value) in upstream.headers() {
        // Forward everything except hop-by-hop framing — content-length,
        // content-encoding (compression survives end-to-end), content-type all
        // pass through untouched.
        if is_hop_by_hop(name) {
            continue;
        }
        resp = resp.header(name, value);
    }
    // Stream the response body — critical for SSE (`/api/v1/events`) and free
    // for everything else.
    resp.body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// A lightweight "workspace starting…" page that auto-reloads, served while a
/// workspace engine is still booting (ADR 0014 §11). 503 so caches/bots don't
/// treat it as the real page; `Retry-After` + a meta-refresh drive the reload.
/// Used by the proxy on a connect failure (engine cold boot) and by the gateway
/// `fallback` when a document navigation lazy-starts a stopped workspace.
///
/// Carries `X-Lucidos-Boot-Splash: 1` so the PWA service worker
/// (`crates/lucidos-app/public/sw.js::networkFirstShell`) can tell this
/// intentional 503 boot page apart from a transient 502/500. Without the marker
/// the SW treats any non-ok navigation as an error and serves the cached app
/// shell instead — which then 503-storms its API calls against the not-yet-ready
/// engine, so the splash never shows for an installed PWA. The marker tells the SW
/// to SHOW this response; it is never cached (still a 503 + `no-store`).
pub fn starting_page() -> Response {
    let mut resp = Response::builder().status(StatusCode::SERVICE_UNAVAILABLE);
    resp = resp.header(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    resp = resp.header(header::RETRY_AFTER, HeaderValue::from_static("2"));
    resp = resp.header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp = resp.header("x-lucidos-boot-splash", HeaderValue::from_static("1"));
    resp.body(Body::from(splash_page_html("Workspace starting…")))
        .unwrap_or_else(|_| StatusCode::SERVICE_UNAVAILABLE.into_response())
}

/// The brand boot splash as a self-contained HTML page: a full-screen gradient
/// wash with the white Lucidos mark playing its reveal animation, and `label`
/// shown beneath it. The reusable background — every boot-window surface passes
/// its own text. Mirrored from `components/shared/BootSplash.tsx`,
/// `LucidosMark.tsx`, and the `.lucidos-mark-animated` rules in
/// `crates/lucidos-app/src/styles/components.css`; self-contained so it renders
/// before any engine is reachable. The 2s meta-refresh that polls for the booted
/// engine doubles as the animation loop. The `<link rel="icon">` is an inline
/// `data:` URI mirror of `crates/lucidos-app/public/favicon.svg` (gradient tile
/// on a rounded square) — inlined rather than referenced because the splash must
/// render before any engine can serve `/favicon.svg`. Geometry + gradient are the
/// single-source values from the icon generator; keep them in sync if the brand
/// changes there. `label` is interpolated verbatim — pass trusted, static text.
fn splash_page_html(label: &str) -> String {
    // Split the template around the label so the static CSS (full of `{}`) needs
    // no `format!` brace-escaping.
    const HEAD: &str = r##"<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="refresh" content="2">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="theme-color" content="#0a4ea8">
<link rel="icon" type="image/svg+xml" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><defs><radialGradient id='g' gradientUnits='userSpaceOnUse' cx='30' cy='22' r='125'><stop offset='0' stop-color='%232d83e0'/><stop offset='1' stop-color='%230a4ea8'/></radialGradient></defs><rect width='100' height='100' rx='22' fill='url(%23g)'/><g transform='translate(13 13) scale(0.74)' fill='%23fff'><rect x='17' y='17' width='29' height='29' rx='7'/><rect x='17' y='54' width='29' height='29' rx='7'/><rect x='54' y='54' width='29' height='29' rx='7'/><path d='M68.5 12 C71 25 74 28.5 87 31 C74 33.5 71 37 68.5 50 C66 37 63 33.5 50 31 C63 28.5 66 25 68.5 12 Z'/></g></svg>">
<title>Starting…</title>
<style>
html,body{margin:0;height:100%}
body{display:flex;flex-direction:column;min-height:100vh;align-items:center;justify-content:center;
text-align:center;color:#fff;font-family:system-ui,-apple-system,sans-serif;
background:radial-gradient(125% 125% at 30% 22%,#2d83e0 0%,#0a4ea8 100%)}
.mark{width:min(46vmin,15rem);height:min(46vmin,15rem)}
.mark-label{margin-top:1.25rem;font-size:.95rem;letter-spacing:.02em;opacity:.85}
.lmk-tile,.lmk-spark{transform-box:fill-box;transform-origin:center}
.lmk-tile{animation:tile-in .5s cubic-bezier(.34,1.56,.64,1) both}
.lmk-tile-1{animation-delay:.15s}.lmk-tile-2{animation-delay:.28s}.lmk-tile-3{animation-delay:.41s}
.lmk-spark{animation:spark-in .55s cubic-bezier(.34,1.56,.64,1) .6s both}
@keyframes tile-in{from{opacity:0;transform:scale(.3)}60%{opacity:1;transform:scale(1.08)}to{opacity:1;transform:scale(1)}}
@keyframes spark-in{from{opacity:0;transform:scale(0) rotate(-60deg)}70%{opacity:1;transform:scale(1.2) rotate(8deg)}to{opacity:1;transform:scale(1) rotate(0)}}
@media (prefers-reduced-motion:reduce){.lmk-tile,.lmk-spark{animation:none}}
</style></head>
<body>
<svg class="mark" viewBox="0 0 100 100" aria-hidden="true">
<g transform="translate(11 11) scale(0.78)" fill="#ffffff">
<rect class="lmk-tile lmk-tile-1" x="17" y="17" width="29" height="29" rx="7"/>
<rect class="lmk-tile lmk-tile-2" x="17" y="54" width="29" height="29" rx="7"/>
<rect class="lmk-tile lmk-tile-3" x="54" y="54" width="29" height="29" rx="7"/>
<path class="lmk-spark" d="M68.5 12 C71 25 74 28.5 87 31 C74 33.5 71 37 68.5 50 C66 37 63 33.5 50 31 C63 28.5 66 25 68.5 12 Z"/>
</g>
</svg>
<p class="mark-label">"##;
    const TAIL: &str = r##"</p>
</body></html>"##;
    format!("{HEAD}{label}{TAIL}")
}

/// Build the shared reqwest client used for proxying to the workspace engines.
/// Connections are pooled; no global timeout (SSE streams are long-lived), only
/// a connect timeout so an unreachable engine fails fast. Accepts invalid certs
/// because a dev engine serves its own self-signed cert on its port (the gateway
/// proxies to it over https); harmless for the plain-http packaged loopback engine.
pub fn build_client() -> Client {
    Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .pool_max_idle_per_host(16)
        .pool_idle_timeout(std::time::Duration::from_secs(60))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("failed to build gateway proxy reqwest client")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_prefix_handles_root_and_subpaths() {
        assert_eq!(strip_prefix("/dev/", "dev"), Some("/"));
        assert_eq!(
            strip_prefix("/dev/api/v1/events", "dev"),
            Some("/api/v1/events")
        );
        assert_eq!(
            strip_prefix("/dev/assets/x.js?v=1", "dev"),
            Some("/assets/x.js?v=1")
        );
        // Exactly "/<slug>" → None (caller redirects to add trailing slash).
        assert_eq!(strip_prefix("/dev", "dev"), None);
    }

    #[test]
    fn strip_prefix_rejects_foreign_slugs() {
        // A different slug that shares a prefix must not match.
        assert_eq!(strip_prefix("/devx/foo", "dev"), None);
        assert_eq!(strip_prefix("/other/foo", "dev"), None);
        assert_eq!(strip_prefix("/api/v1/health", "dev"), None);
    }

    #[test]
    fn hop_by_hop_predicate_matches_framing_headers() {
        assert!(is_hop_by_hop(&HeaderName::from_static("connection")));
        assert!(is_hop_by_hop(&HeaderName::from_static("transfer-encoding")));
        assert!(is_hop_by_hop(&HeaderName::from_static("upgrade")));
        assert!(!is_hop_by_hop(&HeaderName::from_static("content-type")));
        assert!(!is_hop_by_hop(&HeaderName::from_static("content-encoding")));
    }
}

/// Tests for the streaming forwarder against a mock upstream. The gateway must
/// add `X-Forwarded-Prefix`, NOT strip `Accept-Encoding`, stream the response
/// body, and fall back to the auto-retry page (not a raw error) when the engine
/// is unreachable.
#[cfg(test)]
mod proxy_tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Capturing upstream: records the raw request bytes it receives, then 200s.
    async fn capturing_upstream() -> (u16, Arc<tokio::sync::Mutex<String>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let captured = Arc::new(tokio::sync::Mutex::new(String::new()));
        let c = captured.clone();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                *c.lock().await = String::from_utf8_lossy(&buf[..n]).into_owned();
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                    .await;
                let _ = sock.flush().await;
            }
        });
        (port, captured)
    }

    fn request(method: &str, uri: &str, body: Body) -> axum::extract::Request {
        axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .body(body)
            .unwrap()
    }

    #[tokio::test]
    async fn adds_forwarded_prefix_and_keeps_accept_encoding() {
        let (port, captured) = capturing_upstream().await;
        let target = format!("http://127.0.0.1:{port}");
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/dev/")
            .header("accept-encoding", "gzip, br")
            .header("accept-language", "en-US")
            .body(Body::empty())
            .unwrap();
        let resp = proxy(&build_client(), &target, "dev", req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        assert!(
            got.contains("x-forwarded-prefix: /dev/"),
            "the forwarded prefix must be added; upstream saw:\n{got}"
        );
        assert!(
            got.contains("accept-encoding"),
            "accept-encoding must be forwarded (compression stays intact); upstream saw:\n{got}"
        );
        assert!(
            got.contains("accept-language"),
            "other request headers must pass through; upstream saw:\n{got}"
        );
    }

    #[tokio::test]
    async fn strips_client_spoofed_forwarded_prefix() {
        // The gateway is the trust boundary: a client-supplied X-Forwarded-Prefix
        // must NOT survive to the engine, or it would control the stamped
        // `<base href>`. Only the gateway's own `/dev/` may reach the upstream.
        let (port, captured) = capturing_upstream().await;
        let target = format!("http://127.0.0.1:{port}");
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/dev/")
            .header("x-forwarded-prefix", "/evil/")
            .body(Body::empty())
            .unwrap();
        let resp = proxy(&build_client(), &target, "dev", req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        assert!(
            got.contains("x-forwarded-prefix: /dev/"),
            "the gateway's own prefix must be forwarded; upstream saw:\n{got}"
        );
        assert!(
            !got.contains("/evil/"),
            "a client-spoofed x-forwarded-prefix must be stripped; upstream saw:\n{got}"
        );
    }

    #[tokio::test]
    async fn no_trailing_slash_redirects() {
        let resp = proxy(
            &build_client(),
            "http://127.0.0.1:1",
            "dev",
            request("GET", "/dev", Body::empty()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            resp.headers().get(header::LOCATION).unwrap(),
            "/dev/"
        );
    }

    #[tokio::test]
    async fn unreachable_engine_serves_starting_page() {
        // Bind then drop to get a definitely-closed loopback port (connect →
        // ECONNREFUSED). The boot-window page (503 + auto-refresh) replaces the
        // raw 502 from 0013.
        let port = {
            let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            l.local_addr().unwrap().port()
        };
        let target = format!("http://127.0.0.1:{port}");
        let resp = proxy(&build_client(), &target, "dev", request("GET", "/dev/", Body::empty())).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers().get(header::RETRY_AFTER).unwrap(), "2");
        // The PWA service worker keys on this marker to SHOW the splash instead of
        // falling back to the cached app shell (sw.js::networkFirstShell).
        assert_eq!(resp.headers().get("x-lucidos-boot-splash").unwrap(), "1");
    }
}
