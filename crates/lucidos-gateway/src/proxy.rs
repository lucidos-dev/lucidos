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
    boot_label: &str,
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
    // streamed body), and any inbound `x-forwarded-prefix` / `x-forwarded-host`.
    // Stripping those two forwarding headers is the trust boundary: a
    // client-forged value must never reach the engine or a configured upstream.
    //   - `x-forwarded-prefix` is LOAD-BEARING and re-injected below with the
    //     gateway's own value. reqwest's `.header()` *appends* rather than
    //     replaces, so a client-spoofed one left here would sit FIRST and the
    //     engine's `forwarded_prefix` reads `headers.get(...)` (the first value).
    //   - `x-forwarded-host` is NOT re-injected — the engine's credentialed-proxy
    //     guard uses `Sec-Fetch-Site`, not a reconstructed host — but it is still
    //     stripped so a forged value can't pass through to an upstream that trusts
    //     it for URL generation / host-based authz.
    let mut builder = client.request(method.clone(), &url);
    for (name, value) in req.headers() {
        if name == header::HOST
            || name == header::CONTENT_LENGTH
            || name.as_str().eq_ignore_ascii_case("x-forwarded-prefix")
            || name.as_str().eq_ignore_ascii_case("x-forwarded-host")
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
            // reload by hand. This is the path that renders the ENGINE-reported
            // phases: once `bring_up` sets the route (while the engine is still
            // Booting), a lazy-start navigation lands here, not on `fallback`'s
            // no-route branch — so `boot_label` carries the current phase
            // (caller passes `boot_phase_label`; a transient mid-session restart
            // has no phase and gets the neutral default).
            return starting_page(boot_label);
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
/// `label` is the current boot-phase label (see [`crate::boot_phase`]) — the
/// gateway passes the phase for the slug, the proxy passes the neutral default.
/// It advances across the 2s meta-refresh reloads as the boot progresses.
///
/// Carries `X-Lucidos-Boot-Splash: 1` so the PWA service worker
/// (`crates/lucidos-app/public/sw.js::networkFirstShell`) can tell this
/// intentional 503 boot page apart from a transient 502/500. Without the marker
/// the SW treats any non-ok navigation as an error and serves the cached app
/// shell instead — which then 503-storms its API calls against the not-yet-ready
/// engine, so the splash never shows for an installed PWA. The marker tells the SW
/// to SHOW this response; it is never cached (still a 503 + `no-store`).
pub fn starting_page(label: &str) -> Response {
    // 2s meta-refresh, no escape link — the happy-path boot window.
    boot_splash_response(splash_page_html(label, Some(2), false), "2")
}

/// The boot-splash TERMINAL-FAILURE page: the engine reported a boot failure it
/// cannot retry its way out of (see `lucidos-engine/src/boot_failure.rs`), most
/// commonly a database written by a NEWER Lucidos than the installed one — what an
/// app downgrade produces. `message` is the engine's own user-facing sentence.
///
/// Differs from [`stalled_page`] in the two ways that matter: it states the actual
/// cause instead of "taking longer than expected", and it carries **no
/// meta-refresh** at all. Reloading cannot fix a boot that is definitionally
/// unachievable, and the gateway has already stopped respawning the engine, so a
/// refresh loop would only re-render the same page forever. The escape link to the
/// picker is the one action left. Same 503 + boot-splash marker as its siblings, so
/// the PWA service worker shows it rather than serving the cached app shell.
///
/// This page is also the ONLY place the "install a newer version" remedy can reach
/// the user: the packaged in-app update toast is started from the workspace app's
/// startup hook (`useStartup.ts` → `startAppUpdateChecks`), which never runs while
/// the workspace is stuck on this splash.
pub fn failed_page(message: &str) -> Response {
    // `Retry-After` still advertises a sane poll interval for well-behaved clients
    // even though the page itself does not self-refresh.
    boot_splash_response(splash_page_html(message, None, true), "60")
}

/// The boot-splash ESCAPE page, served once a workspace has been stuck in its boot
/// window past the gateway's budget ([`crate::server::BOOT_ESCAPE_BUDGET`]). An
/// alive-but-unreachable engine (misconfigured bind, network partition) is never
/// marked `Unhealthy`, so without this the splash would meta-refresh forever with
/// no way out. It keeps a SLOWER (10s) refresh so a late-but-real recovery still
/// lands on the workspace, AND shows a manual "Back to workspaces" link to the
/// picker (`/~/?pick`, which stands down the cold-start auto-open so there is no
/// picker↔workspace loop). Same 503 + boot-splash marker as [`starting_page`].
pub fn stalled_page() -> Response {
    boot_splash_response(
        splash_page_html("This is taking longer than expected.", Some(10), true),
        "10",
    )
}

/// Shared 503 boot-splash response shell: `text/html`, `Retry-After`, `no-store`,
/// and the `X-Lucidos-Boot-Splash: 1` marker so the PWA service worker
/// (`networkFirstShell`) SHOWS the page rather than treating the non-ok
/// navigation as an error and serving the cached app shell.
fn boot_splash_response(html: String, retry_after: &'static str) -> Response {
    let mut resp = Response::builder().status(StatusCode::SERVICE_UNAVAILABLE);
    resp = resp.header(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    resp = resp.header(header::RETRY_AFTER, HeaderValue::from_static(retry_after));
    resp = resp.header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp = resp.header("x-lucidos-boot-splash", HeaderValue::from_static("1"));
    resp.body(Body::from(html))
        .unwrap_or_else(|_| StatusCode::SERVICE_UNAVAILABLE.into_response())
}

/// The brand boot splash as a self-contained HTML page: a full-screen gradient
/// wash with the white Lucidos mark playing its reveal animation, and `label`
/// shown beneath it. The reusable background — every boot-window surface passes
/// its own text. The mark is the brand glyph from
/// `crates/lucidos-app/src/components/shared/LucidosMark.tsx`; its per-tile reveal
/// keyframes (`tile-in`/`spark-in`) are self-contained inline below so the page
/// renders before any engine — or the app stylesheet — is reachable. The label
/// styling matches the frontend boot splash (`crates/lucidos-app/index.html`
/// `.boot-splash-status`) so the text does not jump across the cold-boot→workspace
/// seam — see the `.mark-label` rule below. The 2s meta-refresh that polls for the booted
/// engine doubles as the animation loop. The `<link rel="icon">` is an inline
/// `data:` URI mirror of `crates/lucidos-app/public/favicon.svg` (gradient tile
/// on a rounded square) — inlined rather than referenced because the splash must
/// render before any engine can serve `/favicon.svg`. Geometry + gradient are the
/// single-source values from the icon generator; keep them in sync if the brand
/// changes there.
///
/// `label` is HTML-ESCAPED before interpolation. The phase labels are static
/// strings, but [`failed_page`]'s message arrives from the engine over the
/// `boot-failure` control endpoint — loopback-only and id-validated, yet no longer
/// trusted-static once it crosses a wire. Escaping here rather than at that one
/// call site keeps every present and future caller safe by construction.
///
/// `refresh_secs` sets the meta-refresh interval (2s on the happy-path
/// [`starting_page`], slower on [`stalled_page`]); `None` omits the refresh tag
/// entirely, for [`failed_page`], where reloading can never change the outcome.
/// When `escape` is set a manual "Back to workspaces" link to the picker is shown
/// below the label.
fn splash_page_html(label: &str, refresh_secs: Option<u32>, escape: bool) -> String {
    // Split the template around the refresh tag, the label, and the escape link so
    // the static CSS (full of `{}`) needs no `format!` brace-escaping.
    const HEAD_A: &str = r##"<!doctype html><html><head><meta charset="utf-8">
"##;
    const HEAD_B: &str = r##"
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="theme-color" content="#0a4ea8">
<link rel="icon" type="image/svg+xml" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><defs><radialGradient id='g' gradientUnits='userSpaceOnUse' cx='30' cy='22' r='125'><stop offset='0' stop-color='%232d83e0'/><stop offset='1' stop-color='%230a4ea8'/></radialGradient></defs><rect width='100' height='100' rx='22' fill='url(%23g)'/><g transform='translate(13 13) scale(0.74)' fill='%23fff'><rect x='17' y='17' width='29' height='29' rx='7'/><rect x='17' y='54' width='29' height='29' rx='7'/><rect x='54' y='54' width='29' height='29' rx='7'/><path d='M68.5 12 C71 25 74 28.5 87 31 C74 33.5 71 37 68.5 50 C66 37 63 33.5 50 31 C63 28.5 66 25 68.5 12 Z'/></g></svg>">
<title>"##;
    // Split again around the tab title: the failure page must not sit in the tab
    // claiming "Starting…" while the page itself says the workspace cannot open.
    const HEAD_C: &str = r##"</title>
<style>
html,body{margin:0;height:100%}
/* Paint the gradient on the root with a solid fallback + fixed attachment so it
covers the whole viewport — including the iOS standalone-PWA bottom safe-area /
overscroll region, which a body-only background (sized to 100vh) leaves uncovered
(it fell back to a different color — the lighter strip at the bottom). The solid
#0a4ea8 fallback matches the gradient's dark end and the theme-color. */
html{background:#0a4ea8 radial-gradient(125% 125% at 30% 22%,#2d83e0 0%,#0a4ea8 100%) no-repeat fixed}
body{display:flex;flex-direction:column;min-height:100vh;align-items:center;justify-content:center;
text-align:center;color:#fff;
/* Fixed all-system monospace stack — inlined because the splash renders before
any engine can serve the app stylesheet, and deliberately NOT the workspace
font preference (a web font would swap-jank). Same stack the frontend boot
splash status uses (crates/lucidos-app/index.html `.boot-splash-status`) so the
status renders the same across the cold-boot→workspace seam. Keep in sync. */
font-family:ui-monospace,SFMono-Regular,'SF Mono',Menlo,'Fira Code','JetBrains Mono',Monaco,Consolas,monospace}
.mark{width:min(46vmin,15rem);height:min(46vmin,15rem)}
/* font-size, letter-spacing and color match the frontend boot-splash status
(crates/lucidos-app/index.html `.boot-splash-status`) so the label does not
change size/spacing/color across the cold-boot→workspace seam. The margin
mirrors that splash's 1.5rem mark↔status gap (and zeros the default <p> bottom
margin so vertical centering matches). Keep all four values in sync. */
.mark-label{margin:1.5rem 0 0;font-size:.9375rem;letter-spacing:.01em;opacity:.82}
/* The stalled-boot escape link (shown only past the boot budget). Matches the
label's size/spacing, underlined so it reads as a tap target. */
.mark-escape{display:inline-block;margin:1rem 0 0;font-size:.9375rem;letter-spacing:.01em;color:#fff;opacity:.92;text-decoration:underline;text-underline-offset:.2em}
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
    // The picker link stands down the cold-start auto-open (`?pick`), so a manual
    // tap can't loop back into the unreachable workspace.
    const ESCAPE_LINK: &str =
        r##"<a class="mark-escape" href="/~/?pick">Back to workspaces</a>"##;
    let escape_html = if escape { ESCAPE_LINK } else { "" };
    // Omitted entirely (not `content="0"`) when there is nothing to wait for.
    let refresh_html = match refresh_secs {
        Some(secs) => format!(r#"<meta http-equiv="refresh" content="{secs}">"#),
        None => String::new(),
    };
    // A page with nothing to wait for is not "Starting…".
    let title = if refresh_secs.is_some() { "Starting…" } else { "Cannot open workspace" };
    let label = escape_html_text(label);
    format!("{HEAD_A}{refresh_html}{HEAD_B}{title}{HEAD_C}{label}</p>\n{escape_html}\n</body></html>")
}

/// Escape text for interpolation into HTML element content or a quoted attribute.
/// `&` first, or the later replacements' own ampersands would be double-escaped.
fn escape_html_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
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
    fn starting_splash_renders_the_phase_label_and_has_no_escape_link() {
        let html = splash_page_html("Downloading memory model — first run, this can take a minute…", Some(2), false);
        // The current boot-phase label is shown beneath the mark.
        assert!(html.contains("Downloading memory model — first run, this can take a minute…"));
        // The 2s auto-refresh that advances the label / drives the happy-path
        // transition is preserved.
        assert!(html.contains(r#"http-equiv="refresh" content="2""#));
        // The happy-path splash has NO escape link (no anchor, no picker href) and
        // none of the removed auto-redirect machinery.
        assert!(!html.contains("<a "));
        assert!(!html.contains("/~/?pick"));
        assert!(!html.contains("Back to workspaces"));
        assert!(!html.contains("boot-escape"));
        assert!(!html.contains("show-escape"));
        assert!(!html.contains("lucidos-boot-since"));
    }

    #[test]
    fn stalled_splash_has_manual_escape_link_and_slower_refresh() {
        let html = splash_page_html("This is taking longer than expected.", Some(10), true);
        assert!(html.contains("This is taking longer than expected."));
        // A slower (10s) refresh so a late-but-real recovery still lands on the
        // workspace, rather than the happy-path 2s.
        assert!(html.contains(r#"http-equiv="refresh" content="10""#));
        // A MANUAL "Back to workspaces" link to the picker. `?pick` stands down the
        // cold-start auto-open, so tapping it cannot loop back into the workspace.
        assert!(html.contains(r##"href="/~/?pick""##));
        assert!(html.contains("Back to workspaces"));
        // It is a manual link, NOT the removed auto-redirect / countdown.
        assert!(!html.contains("show-escape"));
        assert!(!html.contains("lucidos-boot-since"));
    }

    #[test]
    fn stalled_page_is_503_with_boot_splash_marker() {
        let resp = stalled_page();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            resp.headers().get("x-lucidos-boot-splash").map(|v| v.to_str().unwrap()),
            Some("1")
        );
    }

    /// A terminal failure must not self-refresh: the gateway has stopped
    /// respawning, so a reload loop would re-render the same page forever.
    #[test]
    fn failure_splash_states_the_cause_with_no_refresh_but_an_escape_link() {
        let html = splash_page_html(
            "Lucidos 0.15.0 cannot open this workspace: its database was created by a \
             newer version of Lucidos.",
            None,
            true,
        );
        assert!(html.contains("its database was created by a newer version"), "{html}");
        // No meta-refresh AT ALL — not `content="0"`, not a long interval.
        assert!(!html.contains("http-equiv=\"refresh\""), "{html}");
        // ...and the tab must not claim the workspace is still starting.
        assert!(html.contains("<title>Cannot open workspace</title>"), "{html}");
        assert!(!html.contains("Starting…"), "{html}");
        // The escape to the picker is the only action left.
        assert!(html.contains(r##"href="/~/?pick""##));
        assert!(html.contains("Back to workspaces"));
    }

    /// The failure message crosses a wire (the `boot-failure` control endpoint), so
    /// it is not trusted-static like the phase labels are.
    #[test]
    fn splash_escapes_html_in_the_label() {
        let html = splash_page_html(
            r#"<script>alert("x")</script> & 'quoted'"#,
            None,
            false,
        );
        assert!(!html.contains("<script>"), "raw tag survived: {html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(html.contains("&amp;"), "{html}");
        assert!(html.contains("&quot;"), "{html}");
        assert!(html.contains("&#39;"), "{html}");
    }

    #[test]
    fn failed_page_is_503_with_boot_splash_marker() {
        let resp = failed_page("Lucidos 0.15.0 cannot open this workspace.");
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            resp.headers().get("x-lucidos-boot-splash").map(|v| v.to_str().unwrap()),
            Some("1")
        );
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
    use crate::boot_phase::DEFAULT_LABEL;
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
        let resp = proxy(&build_client(), &target, "dev", DEFAULT_LABEL, req).await;
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
        let resp = proxy(&build_client(), &target, "dev", DEFAULT_LABEL, req).await;
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
    async fn strips_client_spoofed_forwarded_host() {
        // Trust-boundary hygiene: the gateway no longer injects x-forwarded-host
        // (the engine's credentialed-proxy guard uses Sec-Fetch-Site, not a
        // reconstructed host), but a client-forged inbound value must still be
        // stripped so it can't reach an upstream that trusts it. Unlike
        // x-forwarded-prefix, nothing is put back in its place.
        let (port, captured) = capturing_upstream().await;
        let target = format!("http://127.0.0.1:{port}");
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/dev/")
            .header("host", "gateway.example:5251")
            .header("x-forwarded-host", "evil.example")
            .body(Body::empty())
            .unwrap();
        let resp = proxy(&build_client(), &target, "dev", DEFAULT_LABEL, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        assert!(
            !got.contains("x-forwarded-host"),
            "a client-spoofed x-forwarded-host must be stripped and not re-injected; upstream saw:\n{got}"
        );
        assert!(
            !got.contains("evil.example"),
            "the forged forwarded-host value must not reach the upstream; upstream saw:\n{got}"
        );
    }

    #[tokio::test]
    async fn no_trailing_slash_redirects() {
        let resp = proxy(
            &build_client(),
            "http://127.0.0.1:1",
            "dev",
            DEFAULT_LABEL,
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
    async fn unreachable_engine_serves_starting_page_with_boot_label() {
        // Bind then drop to get a definitely-closed loopback port (connect →
        // ECONNREFUSED). The boot-window page (503 + auto-refresh) replaces the
        // raw 502 from 0013.
        let port = {
            let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            l.local_addr().unwrap().port()
        };
        let target = format!("http://127.0.0.1:{port}");
        // The route is set while the engine is still Booting, so a cold-open
        // navigation reaches the proxy (not fallback's no-route branch) during
        // the engine-reported phases — the connect-failure splash MUST render the
        // passed phase label, or those phases would never reach the user.
        let resp = proxy(
            &build_client(),
            &target,
            "dev",
            "Downloading memory model — first run, this can take a minute…",
            request("GET", "/dev/", Body::empty()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers().get(header::RETRY_AFTER).unwrap(), "2");
        // The PWA service worker keys on this marker to SHOW the splash instead of
        // falling back to the cached app shell (sw.js::networkFirstShell).
        assert_eq!(resp.headers().get("x-lucidos-boot-splash").unwrap(), "1");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(
            html.contains("Downloading memory model — first run, this can take a minute…"),
            "the connect-failure splash must render the passed boot phase label"
        );
    }
}
