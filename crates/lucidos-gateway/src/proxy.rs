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
    builder = builder.body(reqwest::Body::wrap_stream(
        req.into_body().into_data_stream(),
    ));

    let upstream = match builder.send().await {
        Ok(r) => r,
        Err(e) => {
            crate::log!(
                "[Gateway] proxy {} -> {} failed: {}",
                path_and_query,
                url,
                e
            );
            // Boot-window UX (ADR 0014 §11): an engine cold boot (pgvector init,
            // migrations, embedding warmup, ~20 CC sessions resuming) can take
            // tens of seconds, during which a connect fails. Serve a lightweight
            // auto-retry page for navigations instead of a raw 502 the user must
            // reload by hand. This is the path that renders the ENGINE-reported
            // phases: once `bring_up` sets the route (while the engine is still
            // Booting), a lazy-start navigation lands here, not on `fallback`'s
            // no-route branch — so `boot_label` carries the current phase
            // (caller passes `boot_splash_label`; a transient mid-session restart
            // has no phase and gets the neutral default).
            return starting_page(boot_label);
        }
    };

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
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
/// `label` is what the gateway currently knows about this boot
/// ([`crate::server::GatewayState::boot_splash_label`]): the boot phase (see
/// [`crate::boot_phase`]), or a RETRYING boot failure's message when an attempt
/// failed on something that can still clear, such as a Docker daemon that has
/// not finished starting. The proxy's own connect-failure path passes the
/// neutral default. It advances across the 2s meta-refresh reloads as the boot
/// progresses.
///
/// A retrying failure belongs on THIS page rather than [`failed_page`] precisely
/// because of the refresh: the condition is expected to clear, and when it does
/// the next reload lands the user in the workspace with nothing to click.
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
/// window past the gateway's budget (`server::BOOT_ESCAPE_BUDGET`, private to
/// that module, so this is a plain reference rather than a doc link). An
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
    resp = resp.header(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp = resp.header(header::RETRY_AFTER, HeaderValue::from_static(retry_after));
    resp = resp.header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp = resp.header("x-lucidos-boot-splash", HeaderValue::from_static("1"));
    resp.body(Body::from(html))
        .unwrap_or_else(|_| StatusCode::SERVICE_UNAVAILABLE.into_response())
}

/// The brand boot splash as a self-contained HTML page: a full-screen gradient
/// wash with the white Lucidos mark playing its reveal animation, and `label`
/// shown beneath it. The reusable background, where every boot-window surface
/// passes its own text.
///
/// **This is the app's own boot splash, rendered by the gateway.** The page is
/// assembled from [`app_splash_css`] and [`app_mark_svg`], both lifted verbatim
/// out of `crates/lucidos-app/index.html` at compile time, so the two surfaces
/// are one splash rather than two copies: the user crosses from this page to the
/// app document mid-boot, on the same url, and nothing about the mark or the
/// status can move because nothing about them is defined twice. What this page
/// adds is its own canvas plus three documented deviations (a wrapping status
/// line, no breathe under the meta-refresh, its own label), and one difference
/// in timing rather than style: it renders the shared escape link outright,
/// where the app document keeps the same link hidden until its boot gives up.
/// It is also self-contained by necessity: it renders when no engine is
/// reachable, so it can link nothing.
///
/// The `<link rel="icon">` is an inline `data:` URI mirror of
/// `crates/lucidos-app/public/favicon.svg` (gradient tile on a rounded square),
/// inlined for the same reason. Its geometry + gradient are the single-source
/// values from the icon generator; keep them in sync if the brand changes there.
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
    const STYLE_OPEN: &str = r##"</title>
<style>
"##;
    // Everything the shared stylesheet does NOT cover: this document's own canvas,
    // and the four places this page deliberately differs from the app splash.
    const GATEWAY_CSS_AND_BODY: &str = r##"
html,body{margin:0;height:100%}
/* Paint the gradient on the root with a base colour + fixed attachment so it
covers the whole viewport, the iOS standalone-PWA bottom safe-area / overscroll
region included. The fixed `inset:0` .boot-splash element leaves that strip
uncovered, and iOS fills it with the flat BASE COLOUR rather than the gradient
image, so the base is what is actually seen there, butted against the gradient
above it. #145eb9 is the gradient's own colour at the seam (progress 0.70, the
mean across the bottom edge, which runs 0.62 to 0.84 and is aspect-independent);
the 100%-stop #0a4ea8 that used to sit here read as a darker band. Keep this in
step with index.html's FOUC script, which paints <html> for the same reason. */
html{background:#145eb9 radial-gradient(125% 125% at 30% 22%,#2d83e0 0%,#0a4ea8 100%) no-repeat fixed}
/* The deviations from the shared splash stylesheet above, and nothing else.
Every size, color, font and animation comes from there. */
/* 1. The app's status is always one short line (nowrap + ellipsis). These labels
are sentences (a boot phase, or the reason a workspace cannot open), so let them
wrap rather than truncate. A one-line label still fills exactly the 1.4em box the
app reserves, so the mark sits in the same place on both surfaces. */
.boot-splash-status{height:auto;white-space:normal;overflow:visible;text-align:center}
/* 2. This page reloads every couple of seconds (the meta-refresh polls for the
booted engine) and every reload restarts its animations, so a breathe would snap
back to full opacity each time. Overriding animation-NAME alone reuses the shared
timing: the reveal on first paint, then the mark simply stands there. The
workspace document, which does not reload, picks the breathe up when it takes
over. Scoped to no-preference because these rules sit after the shared sheet and
would otherwise put a name back on the animation it silences for reduced motion. */
@media (prefers-reduced-motion:no-preference){
.boot-splash-mark{animation-name:boot-mark-reveal}
.boot-splash-formed .boot-splash-mark{animation-name:none}
}
/* The escape link needs nothing here: `.boot-splash-escape` is in the shared
sheet above, because the app document offers the same link when its own boot
gives up. This page differs only in WHEN it renders one (outright, on the
stalled/failed pages, where there is nothing else left to offer). */
</style></head>
<body>
<div class="boot-splash">
"##;
    // 3. The last deviation, in markup rather than CSS: the app bakes its own
    // status text ("Opening your workspace…"), ours is per-page.
    const MARK_TO_LABEL: &str = r##"
<div class="boot-splash-status boot-splash-status-shown">"##;
    // Keep the mark built for as long as this tab is showing one. This page and
    // the app shell are the SAME url (the meta-refresh reloads this until the
    // engine answers, then the engine serves index.html), so with no handover
    // every reload, and the final swap to the app, would re-play the reveal: a
    // mark that is already standing there drops to opacity 0 and rebuilds. Read
    // the flag to skip our own reveal, and set it for whoever comes next.
    // index.html REMOVES it as it reads it, so it can never suppress a reveal
    // that would not have been a rebuild. sessionStorage is per-tab and
    // same-origin (the gateway serves both documents), which is exactly the scope
    // of "a mark is on screen in this tab right now".
    const HANDOVER: &str = r##"<script>try{var f=sessionStorage.getItem('lucidos-splash-mark-formed')==='1';sessionStorage.setItem('lucidos-splash-mark-formed','1');if(f){document.querySelector('.boot-splash').classList.add('boot-splash-formed')}}catch(e){}</script>"##;
    // The picker link stands down the cold-start auto-open (`?pick`), so a manual
    // tap can't loop back into the unreachable workspace.
    const ESCAPE_LINK: &str =
        r##"<a class="boot-splash-escape" href="/~/?pick">Back to workspaces</a>"##;
    let escape_html = if escape { ESCAPE_LINK } else { "" };
    // Omitted entirely (not `content="0"`) when there is nothing to wait for.
    let refresh_html = match refresh_secs {
        Some(secs) => format!(r#"<meta http-equiv="refresh" content="{secs}">"#),
        None => String::new(),
    };
    // A page with nothing to wait for is not "Starting…".
    let title = if refresh_secs.is_some() {
        "Starting…"
    } else {
        "Cannot open workspace"
    };
    let label = escape_html_text(label);
    let css = app_splash_css();
    let mark = app_mark_svg();
    format!(
        "{HEAD_A}{refresh_html}{HEAD_B}{title}{STYLE_OPEN}{css}{GATEWAY_CSS_AND_BODY}{mark}\
         {MARK_TO_LABEL}{label}</div>\n{escape_html}\n</div>\n{HANDOVER}\n</body></html>"
    )
}

/// The app document, embedded at COMPILE time. The splash must render with no
/// engine reachable, so it cannot link the app's stylesheet; embedding the file
/// lets this page carry the very same one inline. `include_str!` is a build
/// dependency, so cargo rebuilds this crate whenever index.html changes, and
/// `files_require_restart` (engine-side) treats index.html as a bundled asset for
/// the same reason.
const APP_INDEX_HTML: &str = include_str!("../../lucidos-app/index.html");

const CSS_START: &str = "/* lucidos-boot-splash-css-start */";
const CSS_END: &str = "/* lucidos-boot-splash-css-end */";
const MARK_START: &str = "<!-- lucidos-boot-splash-mark-start -->";
const MARK_END: &str = "<!-- lucidos-boot-splash-mark-end -->";

/// The boot-splash stylesheet, verbatim from index.html (see its "SINGLE SOURCE
/// FOR BOTH SPLASH SURFACES" comment). This is why the two splashes cannot
/// drift: there is one stylesheet, and this page overrides exactly four things
/// in it.
///
/// Empty if the markers ever move, which degrades to an unstyled splash rather
/// than taking the page down. `the_app_splash_stylesheet_and_mark_are_extractable`
/// is what actually prevents that, at build-gate time.
fn app_splash_css() -> &'static str {
    slice_between(APP_INDEX_HTML, CSS_START, CSS_END).unwrap_or("")
}

/// The boot-splash mark, verbatim from index.html, so both surfaces draw the
/// same glyph with the same classes and the same reveal. Same empty-on-missing
/// contract as [`app_splash_css`].
fn app_mark_svg() -> &'static str {
    slice_between(APP_INDEX_HTML, MARK_START, MARK_END).unwrap_or("")
}

/// The text between `open` and the first following `close`, excluding both.
fn slice_between<'a>(haystack: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = haystack.find(open)? + open.len();
    let end = haystack[start..].find(close)? + start;
    Some(&haystack[start..end])
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

    /// The anchors [`app_splash_css`] / [`app_mark_svg`] slice index.html on. A
    /// rename there would silently leave this page unstyled or markless, which
    /// is the one failure mode of sharing the file instead of copying it. The
    /// app side pins the same contract in `src/utils/bootSplash.test.ts`.
    #[test]
    fn the_app_splash_stylesheet_and_mark_are_extractable() {
        // One pair of each marker, or the slice silently takes the wrong span.
        for marker in [CSS_START, CSS_END, MARK_START, MARK_END] {
            assert_eq!(APP_INDEX_HTML.matches(marker).count(), 1, "{marker}");
        }
        let css = app_splash_css();
        assert!(css.contains(".boot-splash-mark"), "{css}");
        assert!(css.contains("@keyframes boot-mark-reveal"), "{css}");
        assert!(css.contains("@keyframes boot-mark-breathe"), "{css}");
        let mark = app_mark_svg();
        assert!(mark.contains(r#"<svg class="boot-splash-mark""#), "{mark}");
        assert!(mark.contains("<rect"), "{mark}");
        assert!(mark.contains("</svg>"), "{mark}");
    }

    /// The page renders the app's splash rather than a copy of it: same
    /// stylesheet, same mark, same class names. Nothing about the mark's size or
    /// the status line's placement is defined here, so nothing can drift across
    /// the seam where this page hands over to the app document.
    #[test]
    fn splash_page_is_built_from_the_app_splash_not_a_copy_of_it() {
        let html = splash_page_html("Starting engine", Some(2), true);
        assert!(html.contains(app_splash_css()), "{html}");
        assert!(html.contains(app_mark_svg()), "{html}");
        assert!(html.contains(r#"<div class="boot-splash">"#), "{html}");
        assert!(
            html.contains(r#"<div class="boot-splash-status boot-splash-status-shown">"#),
            "{html}"
        );
        // Everything below is about this page's OWN css (the tail after the
        // shared sheet); the sheet itself is the app's to police.
        let tail = html.split("</style>").next().unwrap_or_default();
        let tail = tail.split("html,body{margin:0").nth(1).unwrap_or_default();
        let tail = strip_css_comments(tail);
        // A rem length here would ride the root font-size, which differs between
        // the two documents (the app root is var(--user-ui-scale)); that is what
        // made the mark grow at the seam.
        assert!(!tail.contains("rem"), "{tail}");
        // No type of its own either: every line on this page inherits the
        // stack/size/spacing from `.boot-splash`. A `font-` declaration here is
        // a second copy of a value the shared sheet already owns, and dropping
        // the old `body{font-family:…}` without inheriting is what once left the
        // escape link rendering in the UA serif.
        assert!(!tail.contains("font-"), "{tail}");
    }

    /// The escape link is one link with one appearance across both surfaces: the
    /// app document offers the same link when its own boot gives up, so the rule
    /// lives in the shared sheet and this page must NOT carry a second copy.
    #[test]
    fn escape_link_is_styled_by_the_shared_sheet_not_a_local_copy() {
        assert!(
            app_splash_css().contains(".boot-splash-escape"),
            "the shared sheet must own the escape link's appearance"
        );
        let html = splash_page_html("This is taking longer than expected.", Some(10), true);
        let tail = html.split("</style>").next().unwrap_or_default();
        let tail = tail.split("html,body{margin:0").nth(1).unwrap_or_default();
        assert!(
            !strip_css_comments(tail).contains(".boot-splash-escape"),
            "{tail}"
        );
    }

    /// The canvas base colour is what iOS paints into the bottom safe-area strip
    /// on BOTH splash surfaces, so the two must agree or the seam moves when this
    /// page hands over to the app document mid-boot. index.html is embedded here
    /// already, so read the value out of it rather than keeping a second copy of
    /// the constant.
    #[test]
    fn splash_canvas_base_colour_matches_the_app_document() {
        let app_base = APP_INDEX_HTML
            .split("<body style=\"background:")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .expect("index.html body must paint the boot canvas");
        assert!(
            app_base.starts_with('#') && app_base.len() == 7,
            "unexpected base colour in index.html: {app_base}"
        );
        let html = splash_page_html("Starting engine", Some(2), false);
        assert!(
            html.contains(&format!("html{{background:{app_base} radial-gradient(")),
            "gateway canvas base drifted from index.html ({app_base}): {html}"
        );
    }

    /// CSS comments removed, so a comment that MENTIONS a unit is not read as a
    /// declaration using it.
    fn strip_css_comments(css: &str) -> String {
        let mut out = String::new();
        let mut rest = css;
        while let Some(open) = rest.find("/*") {
            out.push_str(&rest[..open]);
            rest = match rest[open..].find("*/") {
                Some(close) => &rest[open + close + 2..],
                None => "",
            };
        }
        out.push_str(rest);
        out
    }

    /// Every document in a boot shows the SAME standing mark: this page reloads
    /// itself every couple of seconds and is then replaced by the app document at
    /// the same url, so without a handover each of those would re-play the reveal
    /// and rebuild a mark that is already on screen. The flag is set for the next
    /// document and read to suppress our own reveal; index.html consumes it
    /// (removing it), so it can never suppress a reveal that was not a rebuild.
    #[test]
    fn splash_keeps_the_mark_built_across_every_document_in_a_boot() {
        for html in [
            splash_page_html("Starting engine", Some(2), false),
            splash_page_html("This is taking longer than expected.", Some(10), true),
        ] {
            assert!(
                html.contains("sessionStorage.setItem('lucidos-splash-mark-formed','1')"),
                "{html}"
            );
            assert!(
                html.contains("sessionStorage.getItem('lucidos-splash-mark-formed')"),
                "{html}"
            );
            assert!(
                html.contains("classList.add('boot-splash-formed')"),
                "{html}"
            );
            // Once formed, the mark stands still: the meta-refresh restarts every
            // animation, so a breathe would snap back to full opacity each reload.
            assert!(
                html.contains(".boot-splash-formed .boot-splash-mark{animation-name:none}"),
                "{html}"
            );
        }
    }

    #[test]
    fn starting_splash_renders_the_phase_label_and_has_no_escape_link() {
        let html = splash_page_html("Running migrations…", Some(2), false);
        // The current boot-phase label is shown beneath the mark.
        assert!(html.contains("Running migrations…"));
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

    /// A gateway-observed failure the supervisor is still retrying rides the
    /// SAME starting splash: the reason is visible, and the 2s refresh survives
    /// so the page carries the user into the workspace the moment an attempt
    /// succeeds. Reading as a dead end would be wrong, because it is not one.
    #[test]
    fn retrying_splash_states_the_reason_and_keeps_refreshing() {
        let resp = starting_page(
            &crate::boot_failure::BootFailure::retrying(
                "The Docker daemon is not running yet.",
                2,
                5,
            )
            .message(),
        );
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let html = splash_page_html(
            "The Docker daemon is not running yet. Retrying… (attempt 2 of 5)",
            Some(2),
            false,
        );
        assert!(
            html.contains("The Docker daemon is not running yet."),
            "{html}"
        );
        assert!(html.contains("Retrying… (attempt 2 of 5)"), "{html}");
        // The refresh is the whole point: this state clears by itself.
        assert!(
            html.contains(r#"http-equiv="refresh" content="2""#),
            "{html}"
        );
        assert!(html.contains("<title>Starting…</title>"), "{html}");
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
            resp.headers()
                .get("x-lucidos-boot-splash")
                .map(|v| v.to_str().unwrap()),
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
        assert!(
            html.contains("its database was created by a newer version"),
            "{html}"
        );
        // No meta-refresh AT ALL — not `content="0"`, not a long interval.
        assert!(!html.contains("http-equiv=\"refresh\""), "{html}");
        // ...and the tab must not claim the workspace is still starting.
        assert!(
            html.contains("<title>Cannot open workspace</title>"),
            "{html}"
        );
        assert!(!html.contains("Starting…"), "{html}");
        // The escape to the picker is the only action left.
        assert!(html.contains(r##"href="/~/?pick""##));
        assert!(html.contains("Back to workspaces"));
    }

    /// The failure message crosses a wire (the `boot-failure` control endpoint), so
    /// it is not trusted-static like the phase labels are.
    #[test]
    fn splash_escapes_html_in_the_label() {
        let html = splash_page_html(r#"<script>alert("x")</script> & 'quoted'"#, None, false);
        // The page ships exactly ONE script of its own (the mark handover), so a
        // raw tag out of the label would show up as a second one. Counting beats
        // a bare "contains no <script>": that only held while the page had none.
        assert_eq!(
            html.matches("<script>").count(),
            1,
            "raw tag survived: {html}"
        );
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
            resp.headers()
                .get("x-lucidos-boot-splash")
                .map(|v| v.to_str().unwrap()),
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
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
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
        assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/dev/");
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
            "Running migrations…",
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
            html.contains("Running migrations…"),
            "the connect-failure splash must render the passed boot phase label"
        );
    }
}
