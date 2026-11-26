use std::borrow::Cow;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::engine_hop::{self, Credential, EngineAddress};
use lucidos_gateway_stamp::StampKey;

pub(crate) const HEADER_CALLER_KIND: &str = "x-lucidos-caller-kind";

fn caller_kind(headers: &HeaderMap, extensions: &axum::http::Extensions) -> &'static str {
    if extensions
        .get::<crate::auth::AuthenticatedDevice>()
        .is_some()
        || headers.contains_key(header::ORIGIN)
        || headers
            .keys()
            .any(|name| name.as_str().starts_with("sec-fetch-"))
    {
        "browser"
    } else if extensions
        .get::<crate::auth::AuthenticatedLocalProcess>()
        .is_some()
        || extensions
            .get::<crate::auth::AgentRestoreCaller>()
            .is_some()
        || extensions
            .get::<crate::workspace_calls::AttestedWorkspaceCall>()
            .is_some()
        || extensions
            .get::<crate::diagnostic_transport::AttestedDiagnosticDelivery>()
            .is_some()
    {
        "local-process"
    } else {
        "external"
    }
}

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

pub(crate) fn is_hop_by_hop(name: &HeaderName) -> bool {
    let n = name.as_str();
    HOP_BY_HOP.iter().any(|h| n.eq_ignore_ascii_case(h))
}

pub(crate) fn gateway_owns_header(name: &HeaderName, keep_handover: bool) -> bool {
    let n = name.as_str();
    if crate::diagnostic_transport::owns_header(n)
        || name == header::HOST
        || name == header::CONTENT_LENGTH
        || n.eq_ignore_ascii_case("x-forwarded-prefix")
        || n.eq_ignore_ascii_case("x-forwarded-host")
        || n.eq_ignore_ascii_case(HEADER_CALLER_KIND)
        || n.eq_ignore_ascii_case(crate::stack::HEADER_DEVICE_ID)
        || n.eq_ignore_ascii_case(crate::stack::HEADER_DEVICE_LABEL)
        || n.eq_ignore_ascii_case(lucidos_gateway_stamp::HEADER_GATEWAY_STAMP)
        || n.eq_ignore_ascii_case(lucidos_gateway_stamp::HEADER_DEVICE_SESSION)
        || n.eq_ignore_ascii_case(lucidos_gateway_stamp::HEADER_CALLER_WORKSPACE)
        || n.eq_ignore_ascii_case(lucidos_gateway_stamp::HEADER_CALLER_THREAD)
        || n.eq_ignore_ascii_case(lucidos_gateway_stamp::HEADER_CALLER_VERDICT)
        || n.eq_ignore_ascii_case(crate::workspace_calls::HEADER_RELAYED_CALL_SIGNATURE)
        || n.eq_ignore_ascii_case(crate::auth::HEADER_LOCAL_TOKEN)
        || n.eq_ignore_ascii_case(crate::auth::HEADER_HOOK_SOCKET_TOKEN)
        || n.eq_ignore_ascii_case(lucidos_engine_socket::CREDENTIAL_HEADER)
        || n.eq_ignore_ascii_case(crate::gateway_protocol::HEADER_GATEWAY_CREDENTIAL)
        || n.eq_ignore_ascii_case(crate::relay::HEADER_RELAY_CLAIM)
        || n.eq_ignore_ascii_case(crate::relay::HEADER_RELAY_SIGNATURE)
        || n.eq_ignore_ascii_case(crate::relay::HEADER_RELAY_LOCAL_BROWSER)
        || n.eq_ignore_ascii_case(crate::relay::HEADER_RELAY_CONTRACT)
        || crate::relay::RELAY_USER_HEADERS
            .iter()
            .any(|h| n.eq_ignore_ascii_case(h))
        || n.eq_ignore_ascii_case(crate::oauth_callback::HEADER_LOCAL_BROWSER)
    {
        return true;
    }
    if keep_handover {
        if n.eq_ignore_ascii_case("connection") || n.eq_ignore_ascii_case("upgrade") {
            return false;
        }
        if name == header::ORIGIN {
            return true;
        }
    }
    is_hop_by_hop(name)
}

pub(crate) fn origin_authority_matches_host(origin: &str, host: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(origin) else {
        return false;
    };
    let Some(origin_host) = url.host_str() else {
        return false;
    };
    let origin_authority = match url.port_or_known_default() {
        Some(port) => format!("{origin_host}:{port}"),
        None => origin_host.to_string(),
    };
    let host = host.trim();
    if origin_authority.eq_ignore_ascii_case(host) {
        return true;
    }
    url.scheme() == "https" && url.port().is_none() && origin_host.eq_ignore_ascii_case(host)
}

fn foreign_origin(headers: &HeaderMap) -> Option<&str> {
    let origin = headers.get(header::ORIGIN)?.to_str().ok()?;
    let ours = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|host| origin_authority_matches_host(origin, host));
    (!ours).then_some(origin)
}

pub(crate) fn vouches_for_origin(headers: &HeaderMap) -> bool {
    headers.contains_key(header::ORIGIN)
        && !headers.contains_key("sec-fetch-site")
        && foreign_origin(headers).is_none()
}

pub(crate) fn is_websocket_upgrade(req: &axum::extract::Request) -> bool {
    let hands_over = req
        .headers()
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        });
    let websocket = req
        .headers()
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    hands_over && websocket
}

pub fn strip_prefix<'a>(path_and_query: &'a str, slug: &str) -> Option<&'a str> {
    let prefix = format!("/{slug}");
    let rest = path_and_query.strip_prefix(&prefix)?;
    if rest.is_empty() {
        None
    } else if rest.starts_with('/') {
        Some(rest)
    } else {
        None
    }
}

fn strip_frame_capability(path_and_query: &str) -> Cow<'_, str> {
    let (path, query) = match path_and_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_and_query, None),
    };
    let Some((_, stripped)) = lucidos_frame_capability::split(path) else {
        return Cow::Borrowed(path_and_query);
    };
    match query {
        Some(query) => Cow::Owned(format!("{stripped}?{query}")),
        None => Cow::Borrowed(stripped),
    }
}

pub async fn proxy(
    engine: &EngineAddress,
    slug: &str,
    prefix: &str,
    boot_label: &str,
    local_token: &str,
    stamp_key: Option<&StampKey>,
    req: axum::extract::Request,
) -> Response {
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_string();

    let Some(rest) = strip_prefix(&path_and_query, slug) else {
        return Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(header::LOCATION, prefix)
            .body(Body::empty())
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    };
    let rest = strip_frame_capability(rest);
    let rest = rest.as_ref();

    if is_websocket_upgrade(&req) {
        return proxy_upgrade(engine, rest, slug, prefix, local_token, stamp_key, req).await;
    }

    let (parts, body) = req.into_parts();
    let origin_vouched = vouches_for_origin(&parts.headers);
    let mut upstream = Request::builder().method(parts.method).uri(rest);
    upstream = upstream.header(
        HEADER_CALLER_KIND,
        caller_kind(&parts.headers, &parts.extensions),
    );
    for (name, value) in &parts.headers {
        let consumed = origin_vouched && name == header::ORIGIN;
        if !consumed && !gateway_owns_header(name, false) {
            upstream = upstream.header(name, value);
        }
    }
    upstream = upstream.header("x-forwarded-prefix", prefix);
    if parts
        .extensions
        .get::<crate::oauth_callback::LocalBrowser>()
        .is_some()
    {
        upstream = upstream.header(crate::oauth_callback::HEADER_LOCAL_BROWSER, "1");
    }
    if let Some(device) = parts.extensions.get::<crate::auth::AuthenticatedDevice>() {
        for (name, value) in device.forwarded_headers(slug, stamp_key) {
            upstream = upstream.header(name, value);
        }
    }
    if let Some(call) = parts
        .extensions
        .get::<crate::workspace_calls::AttestedWorkspaceCall>()
    {
        for (name, value) in call.forwarded_headers(slug, stamp_key) {
            upstream = upstream.header(name, value);
        }
    }
    if let Some(delivery) = parts
        .extensions
        .get::<crate::diagnostic_transport::AttestedDiagnosticDelivery>()
    {
        for (name, value) in delivery.forwarded_headers(slug, stamp_key) {
            upstream = upstream.header(name, value);
        }
    }
    let Ok(upstream) = upstream.body(body) else {
        return (StatusCode::BAD_REQUEST, "malformed request").into_response();
    };

    let upstream = match engine_hop::send(engine, upstream, Credential::Local(local_token)).await {
        Ok(r) => r,
        Err(e) if e.is_foreign() => {
            crate::log_legacy!(
                "[Gateway] proxy {} -> {} refused: {}",
                request_path_for_log(&path_and_query),
                engine,
                e
            );
            return (
                StatusCode::BAD_GATEWAY,
                "the engine socket is not this user's",
            )
                .into_response();
        }
        Err(e) => {
            crate::log_legacy!(
                "[Gateway] proxy {} -> {} failed: {}",
                request_path_for_log(&path_and_query),
                engine,
                e
            );
            if crate::auth_api::is_nested_frame(&parts.headers) {
                return frame_starting_page();
            }
            return starting_page(boot_label);
        }
    };

    let (parts, body) = upstream.into_parts();
    let mut resp = Response::builder().status(parts.status);
    for (name, value) in &parts.headers {
        if is_hop_by_hop(name) {
            continue;
        }
        resp = resp.header(name, value);
    }
    resp.body(Body::new(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub(crate) fn request_path_for_log(target: &str) -> &str {
    target.split(['?', '#']).next().unwrap_or_default()
}

async fn proxy_upgrade(
    engine: &EngineAddress,
    rest: &str,
    slug: &str,
    prefix: &str,
    local_token: &str,
    stamp_key: Option<&StampKey>,
    mut req: axum::extract::Request,
) -> Response {
    if let Some(origin) = foreign_origin(req.headers()) {
        crate::log_legacy!(
            "[Gateway] websocket upgrade to /{}{} refused: it came from {}, and this gateway \
             is reached at {:?}",
            slug,
            request_path_for_log(rest),
            origin,
            req.headers().get(header::HOST),
        );
        return (
            StatusCode::FORBIDDEN,
            "cross-origin websocket handshakes are not allowed",
        )
            .into_response();
    }

    let Some(client_upgrade) = req.extensions_mut().remove::<hyper::upgrade::OnUpgrade>() else {
        return (StatusCode::BAD_REQUEST, "not an upgradeable connection").into_response();
    };

    let mut upstream = Request::builder().method(req.method().clone()).uri(rest);
    upstream = upstream.header(
        HEADER_CALLER_KIND,
        caller_kind(req.headers(), req.extensions()),
    );
    for (name, value) in req.headers() {
        if !gateway_owns_header(name, true) {
            upstream = upstream.header(name, value);
        }
    }
    upstream = upstream.header("x-forwarded-prefix", prefix);
    if req
        .extensions()
        .get::<crate::oauth_callback::LocalBrowser>()
        .is_some()
    {
        upstream = upstream.header(crate::oauth_callback::HEADER_LOCAL_BROWSER, "1");
    }
    if let Some(device) = req.extensions().get::<crate::auth::AuthenticatedDevice>() {
        for (name, value) in device.forwarded_headers(slug, stamp_key) {
            upstream = upstream.header(name, value);
        }
    }
    let Ok(upstream) = upstream.body(Body::empty()) else {
        return (StatusCode::BAD_REQUEST, "malformed upgrade request").into_response();
    };

    let engine_response =
        match engine_hop::send_upgrade(engine, upstream, Credential::Local(local_token)).await {
            Ok(r) => r,
            Err(e) if e.is_unreachable() => {
                crate::log_legacy!(
                    "[Gateway] websocket upgrade to {}{} could not connect: {}",
                    engine,
                    request_path_for_log(rest),
                    e
                );
                return (StatusCode::SERVICE_UNAVAILABLE, "engine unreachable").into_response();
            }
            Err(e) => {
                crate::log_legacy!(
                    "[Gateway] websocket upgrade to {}{} failed: {}",
                    engine,
                    request_path_for_log(rest),
                    e
                );
                return (StatusCode::BAD_GATEWAY, "upgrade request failed").into_response();
            }
        };

    if engine_response.status() != StatusCode::SWITCHING_PROTOCOLS {
        let (parts, body) = engine_response.into_parts();
        let mut out = Response::builder().status(parts.status);
        for (name, value) in parts.headers.iter() {
            if !is_hop_by_hop(name) {
                out = out.header(name, value);
            }
        }
        return out
            .body(Body::new(body))
            .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response());
    }

    let mut out = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    for (name, value) in engine_response.headers().iter() {
        out = out.header(name, value);
    }
    let upstream_upgrade = hyper::upgrade::on(engine_response);

    tokio::spawn(async move {
        let (client_io, engine_io) = match tokio::try_join!(client_upgrade, upstream_upgrade) {
            Ok(pair) => pair,
            Err(e) => {
                crate::log_legacy!("[Gateway] websocket upgrade never completed: {}", e);
                return;
            }
        };
        let mut client_io = hyper_util::rt::TokioIo::new(client_io);
        let mut engine_io = hyper_util::rt::TokioIo::new(engine_io);
        if let Err(e) = tokio::io::copy_bidirectional(&mut client_io, &mut engine_io).await {
            if e.kind() != std::io::ErrorKind::ConnectionReset
                && e.kind() != std::io::ErrorKind::BrokenPipe
            {
                crate::log_legacy!("[Gateway] websocket relay ended: {}", e);
            }
        }
    });

    out.body(Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub fn starting_page(label: &str) -> Response {
    boot_splash_response(
        splash_page_html(label, Some(STARTING_POLL_SECS), false),
        STARTING_POLL_SECS,
    )
}

pub fn client_upgrade_page(script: &str) -> Response {
    let html = splash_page_html("Updating remote workspace access…", None, true)
        .replace("</head>", &format!("<script>{script}</script></head>"));
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        html,
    )
        .into_response()
}

pub fn failed_page(message: &str) -> Response {
    boot_splash_response(
        splash_page_html(message, None, true),
        FAILED_RETRY_AFTER_SECS,
    )
}

pub fn stalled_page() -> Response {
    boot_splash_response(
        splash_page_html(
            "This is taking longer than expected.",
            Some(STALLED_POLL_SECS),
            true,
        ),
        STALLED_POLL_SECS,
    )
}

const STARTING_POLL_SECS: u32 = 2;
const STALLED_POLL_SECS: u32 = 10;
const FAILED_RETRY_AFTER_SECS: u32 = 60;

pub fn frame_starting_page() -> Response {
    frame_page(STARTING_POLL_SECS)
}

pub fn frame_stalled_page() -> Response {
    frame_page(STALLED_POLL_SECS)
}

pub fn frame_failed_page() -> Response {
    frame_page(FAILED_RETRY_AFTER_SECS)
}

fn frame_page(refresh_secs: u32) -> Response {
    let html = format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="refresh" content="{refresh_secs}"><meta name="color-scheme" content="light dark"></head><body></body></html>"#
    );
    waiting_response(refresh_secs)
        .body(Body::from(html))
        .unwrap_or_else(|_| StatusCode::SERVICE_UNAVAILABLE.into_response())
}

fn boot_splash_response(html: String, retry_after: u32) -> Response {
    waiting_response(retry_after)
        .header("x-lucidos-boot-splash", HeaderValue::from_static("1"))
        .body(Body::from(html))
        .unwrap_or_else(|_| StatusCode::SERVICE_UNAVAILABLE.into_response())
}

fn waiting_response(retry_after: u32) -> axum::http::response::Builder {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )
        .header(header::RETRY_AFTER, HeaderValue::from(retry_after))
        .header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))
}

fn splash_page_html(label: &str, refresh_secs: Option<u32>, escape: bool) -> String {
    const HEAD_A: &str = r##"<!doctype html><html><head><meta charset="utf-8">
"##;
    const HEAD_B: &str = r##"
<link rel="icon" type="image/svg+xml" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><defs><radialGradient id='g' gradientUnits='userSpaceOnUse' cx='30' cy='22' r='125'><stop offset='0' stop-color='%232d83e0'/><stop offset='1' stop-color='%230a4ea8'/></radialGradient></defs><rect width='100' height='100' rx='22' fill='url(%23g)'/><g transform='translate(13 13) scale(0.74)' fill='%23fff'><rect x='17' y='17' width='29' height='29' rx='7'/><rect x='17' y='54' width='29' height='29' rx='7'/><rect x='54' y='54' width='29' height='29' rx='7'/><path d='M68.5 12 C71 25 74 28.5 87 31 C74 33.5 71 37 68.5 50 C66 37 63 33.5 50 31 C63 28.5 66 25 68.5 12 Z'/></g></svg>">
<title>"##;
    const STYLE_OPEN: &str = r##"</title>
<style>
"##;
    const GATEWAY_CSS_AND_BODY: &str = r##"
html,body{margin:0;height:100%}
/* Paint the gradient on the root with a base colour + fixed attachment so it
covers the whole viewport, the iOS standalone-PWA bottom safe-area / overscroll
region included. The fixed, viewport-sized .boot-splash element leaves that strip
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
wrap rather than truncate. The status hangs below the mark, out of flow, so a
wrapped label grows downward and the mark stays where the app document puts it. */
.boot-splash-status{height:auto;white-space:normal;overflow:visible}
/* 2. This page can still be replaced by a reload (the no-script refresh, a new
kind of page, the swap to the app) and every reload restarts its animations, so
a breathe would snap back to full opacity. Overriding animation-NAME alone reuses the shared
timing: the reveal on first paint, then the mark simply stands there. The
workspace document, which does not reload, picks the breathe up when it takes
over. Scoped away from reduced motion because these rules sit after the shared
sheet and would otherwise put a name back on the animation it silences there.
`:where` keeps the scope weightless, so the cascade is what it was unscoped. */
:where(:root:not([data-motion="reduce"])) .boot-splash-mark{animation-name:boot-mark-reveal}
:where(:root:not([data-motion="reduce"])) .boot-splash-formed .boot-splash-mark{animation-name:none}
/* The escape link needs nothing here: `.boot-splash-escape` is in the shared
sheet above, because the app document offers the same link when its own boot
gives up. This page differs only in WHEN it renders one (outright, on the
stalled/failed pages, where there is nothing else left to offer). */
</style>
<script>try{var s=location.pathname.split('/')[1],k='lucidos-motion',v=null;try{v=localStorage.getItem(s&&s!=='~'?'ws:'+s+':'+k:k)}catch(e){}var r=v==='reduce'||(v!=='full'&&matchMedia('(prefers-reduced-motion: reduce)').matches);document.documentElement.setAttribute('data-motion',r?'reduce':'full')}catch(e){}</script>
</head>
<body>
"##;
    const MARK_TO_LABEL: &str = r##"
<div class="boot-splash-foot">
<div class="boot-splash-status boot-splash-status-shown">"##;
    const HANDOVER: &str = r##"<script>try{var f=sessionStorage.getItem('lucidos-splash-mark-formed')==='1';sessionStorage.setItem('lucidos-splash-mark-formed','1');if(f){document.querySelector('.boot-splash').classList.add('boot-splash-formed')}}catch(e){}</script>"##;
    const ESCAPE_LINK: &str =
        r##"<a class="boot-splash-escape" href="/~/?pick">Back to workspaces</a>"##;
    let escape_html = if escape { ESCAPE_LINK } else { "" };
    let (refresh_html, poll_attrs) = match refresh_secs {
        Some(secs) => (
            format!(r#"<noscript><meta http-equiv="refresh" content="{secs}"></noscript>"#),
            format!(
                r#" data-poll-secs="{secs}" data-ready-label="{}""#,
                escape_html_text(app_status_text())
            ),
        ),
        None => (String::new(), String::new()),
    };
    let title = if refresh_secs.is_some() {
        "Starting…"
    } else {
        "Cannot open workspace"
    };
    let label = escape_html_text(label);
    let css = app_splash_css();
    let mark = app_mark_svg();
    let device_meta = app_device_meta();
    format!(
        "{HEAD_A}{refresh_html}\n{device_meta}{HEAD_B}{title}{STYLE_OPEN}{css}{GATEWAY_CSS_AND_BODY}\
         <div class=\"boot-splash\"{poll_attrs}>\n{mark}\
         {MARK_TO_LABEL}{label}</div>\n{escape_html}\n</div>\n</div>\n{HANDOVER}\n{SPLASH_POLLER}\n</body></html>"
    )
}

const SPLASH_POLLER: &str = r##"<script>(function(){var splash=document.querySelector('.boot-splash'),secs=Number(splash&&splash.getAttribute('data-poll-secs'));if(!secs)return;var every=secs*1000;if(!window.fetch||!window.DOMParser){setTimeout(function(){location.reload()},every);return}var status=splash.querySelector('.boot-splash-status'),ready=splash.getAttribute('data-ready-label'),swap;function reload(){location.reload()}function still(){return document.documentElement.getAttribute('data-motion')==='reduce'}function kind(doc){var s=doc.querySelector('.boot-splash');return s?s.getAttribute('data-poll-secs')+(s.querySelector('.boot-splash-escape')?'+escape':''):''}var mine=kind(document);function show(text,then){if(!status||text===status.textContent){if(then)then();return}clearTimeout(swap);status.classList.add('boot-splash-status-swap');swap=setTimeout(function(){status.textContent=text;status.classList.remove('boot-splash-status-swap');if(then)setTimeout(then,still()?0:300)},still()?0:150)}function poll(){fetch(location.href,{headers:{Accept:'text/html'},cache:'no-store'}).then(function(r){if(!r.headers.get('x-lucidos-boot-splash'))return r.ok&&ready?show(ready,reload):reload();return r.text().then(function(html){var doc=new DOMParser().parseFromString(html,'text/html');if(kind(doc)!==mine)return reload();var next=doc.querySelector('.boot-splash-status');if(next)show(next.textContent);setTimeout(poll,every)})}).catch(function(){setTimeout(poll,every)})}setTimeout(poll,every)})()</script>"##;

const APP_INDEX_HTML: &str = include_str!("../../lucidos-app/index.html");

const CSS_START: &str = "/* lucidos-boot-splash-css-start */";
const CSS_END: &str = "/* lucidos-boot-splash-css-end */";
const MARK_START: &str = "<!-- lucidos-boot-splash-mark-start -->";
const MARK_END: &str = "<!-- lucidos-boot-splash-mark-end -->";
const STATUS_OPEN: &str =
    r#"<div class="boot-splash-status boot-splash-status-shown" role="status" aria-live="polite">"#;

fn app_splash_css() -> &'static str {
    slice_between(APP_INDEX_HTML, CSS_START, CSS_END).unwrap_or("")
}

fn app_mark_svg() -> &'static str {
    slice_between(APP_INDEX_HTML, MARK_START, MARK_END).unwrap_or("")
}

fn app_status_text() -> &'static str {
    slice_between(APP_INDEX_HTML, STATUS_OPEN, "</div>").unwrap_or("")
}

fn app_device_meta() -> String {
    APP_INDEX_HTML
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with(r#"<meta name="viewport""#)
                || line.starts_with(r#"<meta name="apple-mobile-web-app-"#)
                || line.starts_with(r#"<meta name="theme-color""#)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn slice_between<'a>(haystack: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = haystack.find(open)? + open.len();
    let end = haystack[start..].find(close)? + start;
    Some(&haystack[start..end])
}

fn escape_html_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
const TEST_LOCAL_TOKEN: &str = "gateway-own-token";

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
        assert_eq!(strip_prefix("/dev", "dev"), None);
    }

    #[test]
    fn strip_prefix_rejects_foreign_slugs() {
        assert_eq!(strip_prefix("/devx/foo", "dev"), None);
        assert_eq!(strip_prefix("/other/foo", "dev"), None);
        assert_eq!(strip_prefix("/api/v1/health", "dev"), None);
    }

    #[test]
    fn the_app_splash_stylesheet_and_mark_are_extractable() {
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

    #[test]
    fn a_waiting_page_hands_over_on_the_app_documents_status() {
        assert_eq!(APP_INDEX_HTML.matches(STATUS_OPEN).count(), 1);
        let text = app_status_text();
        assert!(!text.trim().is_empty(), "the app's baked status must lift");
        assert!(!text.contains('<') && !text.contains('\n'), "{text}");
        let stamp = format!(r#"data-ready-label="{}""#, escape_html_text(text));
        assert!(splash_page_html("Starting engine…", Some(2), false).contains(&stamp));
        assert!(splash_page_html("Taking longer", Some(10), true).contains(&stamp));
        assert!(!splash_page_html("Cannot open", None, true).contains(r#"data-ready-label=""#));
    }

    #[test]
    fn splash_page_is_built_from_the_app_splash_not_a_copy_of_it() {
        let html = splash_page_html("Starting engine", Some(2), true);
        assert!(html.contains(app_splash_css()), "{html}");
        assert!(html.contains(app_mark_svg()), "{html}");
        assert!(html.contains(r#"<div class="boot-splash""#), "{html}");
        assert!(app_splash_css().contains(".boot-splash-foot"));
        let foot = html
            .split(r#"<div class="boot-splash-foot">"#)
            .nth(1)
            .expect("the page must carry the shared foot");
        assert!(
            foot.starts_with("\n<div class=\"boot-splash-status boot-splash-status-shown\">"),
            "{html}"
        );
        assert!(foot.contains("Back to workspaces"), "{html}");
        let tail = html.split("</style>").next().unwrap_or_default();
        let tail = tail.split("html,body{margin:0").nth(1).unwrap_or_default();
        let tail = strip_css_comments(tail);
        assert!(!tail.contains("rem"), "{tail}");
        assert!(!tail.contains("font-"), "{tail}");
    }

    #[test]
    fn splash_page_lays_out_in_the_app_documents_viewport() {
        let app_tag = |prefix: &str| {
            APP_INDEX_HTML
                .lines()
                .map(str::trim)
                .find(|line| line.starts_with(prefix))
                .unwrap_or_else(|| panic!("index.html must declare {prefix}"))
        };
        let viewport = app_tag(r#"<meta name="viewport""#);
        assert!(viewport.contains("viewport-fit=cover"), "{viewport}");
        let html = splash_page_html("Starting engine…", Some(2), false);
        assert!(html.contains(viewport), "{html}");
        assert_eq!(
            html.matches(r#"<meta name="viewport""#).count(),
            1,
            "{html}"
        );
        let status_bar = app_tag(r#"<meta name="apple-mobile-web-app-status-bar-style""#);
        assert!(html.contains(status_bar), "{html}");
        let theme_color = app_tag(r#"<meta name="theme-color""#);
        assert!(html.contains(theme_color), "{html}");
        assert_eq!(
            html.matches(r#"<meta name="theme-color""#).count(),
            1,
            "{html}"
        );
    }

    #[test]
    fn splash_page_carries_no_ios_status_bar_strip() {
        assert!(!APP_INDEX_HTML.contains("status-bar-fill"));
        let html = splash_page_html("Starting engine…", Some(2), false);
        assert!(!html.contains("status-bar-fill"), "{html}");
        assert!(!html.contains("data-ios-status-bar"), "{html}");
    }

    #[test]
    fn splash_page_resolves_motion_before_the_splash_paints() {
        let html = splash_page_html("Starting engine", Some(2), false);
        let contract = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/lucidos-sdk/src/appearance.ts"
        ))
        .expect("packages/lucidos-sdk/src/appearance.ts must exist");
        assert!(
            contract.contains("export const MOTION_STORAGE_KEY = 'lucidos-motion';"),
            "the page reads the mirror under the contract's key"
        );
        assert!(html.contains("'ws:'+s+':'+k"), "{html}");
        assert!(html.contains("k='lucidos-motion'"), "{html}");
        let set = html
            .find("setAttribute('data-motion'")
            .expect("the page must set data-motion");
        let splash = html.find(r#"<div class="boot-splash""#).unwrap();
        assert!(
            set < splash,
            "data-motion must be set before the splash markup"
        );
        assert!(html.contains(
            r#":where(:root:not([data-motion="reduce"])) .boot-splash-mark{animation-name:boot-mark-reveal}"#
        ));
        assert!(!html.contains("prefers-reduced-motion:no-preference"));
    }

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
            assert!(
                html.contains(".boot-splash-formed .boot-splash-mark{animation-name:none}"),
                "{html}"
            );
        }
    }

    #[test]
    fn starting_splash_renders_the_phase_label_and_has_no_escape_link() {
        let html = splash_page_html("Running migrations…", Some(2), false);
        assert!(html.contains("Running migrations…"));
        assert!(html.contains(r#"data-poll-secs="2""#));
        assert!(!html.contains("<a "));
        assert!(!html.contains("/~/?pick"));
        assert!(!html.contains("Back to workspaces"));
        assert!(!html.contains("boot-escape"));
        assert!(!html.contains("show-escape"));
        assert!(!html.contains("lucidos-boot-since"));
    }

    #[test]
    fn a_new_phase_crossfades_in_place_rather_than_reloading() {
        let html = splash_page_html("Recovering sessions…", Some(2), false);
        assert!(
            html.contains(r#"<noscript><meta http-equiv="refresh" content="2"></noscript>"#),
            "{html}"
        );
        assert_eq!(html.matches("http-equiv=\"refresh\"").count(), 1, "{html}");
        assert!(html.contains(r#"data-poll-secs="2""#), "{html}");
        assert!(html.contains(SPLASH_POLLER), "{html}");
        assert!(SPLASH_POLLER.contains("boot-splash-status-swap"));
        assert!(app_splash_css().contains(".boot-splash-status.boot-splash-status-swap"));
    }

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
        assert!(html.contains(r#"data-poll-secs="2""#), "{html}");
        assert!(html.contains("<title>Starting…</title>"), "{html}");
    }

    #[test]
    fn stalled_splash_has_manual_escape_link_and_slower_refresh() {
        let html = splash_page_html("This is taking longer than expected.", Some(10), true);
        assert!(html.contains("This is taking longer than expected."));
        assert!(html.contains(r#"data-poll-secs="10""#));
        assert!(html.contains(r#"<noscript><meta http-equiv="refresh" content="10"></noscript>"#));
        assert!(html.contains(r##"href="/~/?pick""##));
        assert!(html.contains("Back to workspaces"));
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
        assert!(!html.contains("http-equiv=\"refresh\""), "{html}");
        assert!(!html.contains(r#"data-poll-secs=""#), "{html}");
        assert!(
            html.contains("<title>Cannot open workspace</title>"),
            "{html}"
        );
        assert!(!html.contains("Starting…"), "{html}");
        assert!(html.contains(r##"href="/~/?pick""##));
        assert!(html.contains("Back to workspaces"));
    }

    #[test]
    fn splash_escapes_html_in_the_label() {
        let html = splash_page_html(r#"<script>alert("x")</script> & 'quoted'"#, None, false);
        assert_eq!(
            html.matches("<script>").count(),
            3,
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

#[cfg(test)]
mod proxy_tests {
    use super::*;
    use crate::boot_phase::DEFAULT_LABEL;
    use crate::engine_hop::test_engine;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const TEST_DEVICE_LABEL: &str = "Café iPhone";
    pub(super) const TEST_DEVICE_LABEL_HEADER: &str = "x-lucidos-device-label: caf%c3%a9%20iphone";

    pub(super) fn test_device(id: &str) -> crate::auth::AuthenticatedDevice {
        crate::auth::AuthenticatedDevice {
            id: id.into(),
            label: TEST_DEVICE_LABEL.into(),
        }
    }

    async fn capturing_upstream() -> (
        tempfile::TempDir,
        EngineAddress,
        Arc<tokio::sync::Mutex<String>>,
    ) {
        let dir = test_engine::scratch();
        let (listener, addr) = test_engine::bind(&dir);
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
        (dir, addr, captured)
    }

    fn request(method: &str, uri: &str, body: Body) -> axum::extract::Request {
        axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .body(body)
            .unwrap()
    }

    #[tokio::test]
    async fn restore_provenance_survives_consumed_origin_and_replaces_forgery() {
        for local in [false, true] {
            let (_dir, target, captured) = capturing_upstream().await;
            let mut req = axum::http::Request::builder()
                .method("POST")
                .uri("/dev/api/v1/chat/stream")
                .header("host", "gateway.example:5252")
                .header("origin", "http://gateway.example:5252")
                .header(HEADER_CALLER_KIND, "local-process")
                .body(Body::empty())
                .unwrap();
            if local {
                req.extensions_mut()
                    .insert(crate::auth::AuthenticatedLocalProcess);
            }
            let response = proxy(
                &target,
                "dev",
                "/dev/",
                DEFAULT_LABEL,
                TEST_LOCAL_TOKEN,
                None,
                req,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let got = captured.lock().await.to_lowercase();
            assert!(!got.contains("\r\norigin:"));
            assert!(got.contains("x-lucidos-caller-kind: browser\r\n"));
            assert!(!got.contains("x-lucidos-caller-kind: local-process"));
        }
    }

    #[test]
    fn restore_provenance_is_owned_on_both_transports_and_requires_authentication() {
        let name = HeaderName::from_static(HEADER_CALLER_KIND);
        assert!(gateway_owns_header(&name, false));
        assert!(gateway_owns_header(&name, true));
        let mut extensions = axum::http::Extensions::new();
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_CALLER_KIND, "local-process".parse().unwrap());
        assert_eq!(caller_kind(&headers, &extensions), "external");
        extensions.insert(crate::auth::AuthenticatedLocalProcess);
        assert_eq!(caller_kind(&headers, &extensions), "local-process");
        let mut agent = axum::http::Extensions::new();
        agent.insert(crate::auth::AgentRestoreCaller);
        assert_eq!(caller_kind(&headers, &agent), "local-process");
        extensions.insert(test_device("paired"));
        assert_eq!(caller_kind(&headers, &extensions), "browser");
    }

    #[tokio::test]
    async fn adds_forwarded_prefix_and_keeps_accept_encoding() {
        let (_dir, target, captured) = capturing_upstream().await;
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/dev/")
            .header("accept-encoding", "gzip, br")
            .header("accept-language", "en-US")
            .body(Body::empty())
            .unwrap();
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            req,
        )
        .await;
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
    async fn the_frame_capability_segment_never_reaches_the_engine() {
        let (_dir, target, captured) = capturing_upstream().await;
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            request(
                "GET",
                "/dev/~cap/6a0b~site-publisher~00/app/x/s.css?v=2",
                Body::empty(),
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.clone();
        assert!(
            got.starts_with("GET /app/x/s.css?v=2 "),
            "the capability must be stripped and the query kept; upstream saw:\n{got}"
        );
        assert!(
            !got.contains("~cap"),
            "no trace of the pass may reach the engine; upstream saw:\n{got}"
        );
    }

    #[test]
    fn stripping_a_capability_leaves_an_ordinary_path_alone() {
        for path in [
            "/api/v1/sdk.js",
            "/data/artifacts/x.png?t=1",
            "/~cap",
            "/~cap/tok",
            "/",
        ] {
            assert_eq!(strip_frame_capability(path), path, "{path}");
        }
    }

    #[test]
    fn the_capability_is_cut_out_of_the_path_and_never_out_of_the_query() {
        assert_eq!(
            strip_frame_capability("/~cap/tok/data/x.png?to=/a/b"),
            "/data/x.png?to=/a/b"
        );
        assert_eq!(
            strip_frame_capability("/~cap/tok?x=/y/data/z"),
            "/~cap/tok?x=/y/data/z"
        );
    }

    #[tokio::test]
    async fn strips_client_spoofed_forwarded_prefix() {
        let (_dir, target, captured) = capturing_upstream().await;
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/dev/")
            .header("x-forwarded-prefix", "/evil/")
            .body(Body::empty())
            .unwrap();
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            req,
        )
        .await;
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
        let (_dir, target, captured) = capturing_upstream().await;
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/dev/")
            .header("host", "gateway.example:5251")
            .header("x-forwarded-host", "evil.example")
            .body(Body::empty())
            .unwrap();
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            req,
        )
        .await;
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

    async fn upstream_saw_post(host: &str, origin: &str, sec_fetch_site: Option<&str>) -> String {
        let (_dir, target, captured) = capturing_upstream().await;
        let mut builder = axum::http::Request::builder()
            .method("POST")
            .uri("/dev/api/v1/notification/read")
            .header("host", host)
            .header("origin", origin);
        if let Some(site) = sec_fetch_site {
            builder = builder.header("sec-fetch-site", site);
        }
        let req = builder.body(Body::empty()).unwrap();
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let seen = captured.lock().await;
        seen.to_lowercase()
    }

    #[tokio::test]
    async fn our_own_page_on_plain_http_reaches_the_engine_with_no_origin() {
        let got = upstream_saw_post("100.70.0.1:5252", "http://100.70.0.1:5252", None).await;
        assert!(
            !got.contains("origin:"),
            "a vouched Origin must be consumed; upstream saw:\n{got}"
        );
    }

    #[tokio::test]
    async fn a_foreign_origin_still_reaches_the_engine_to_be_refused() {
        for origin in [
            "http://100.70.0.1:5251",
            "https://evil.example",
            "http://100.70.0.1",
            "null",
        ] {
            let got = upstream_saw_post("100.70.0.1:5252", origin, None).await;
            assert!(
                got.contains(&format!("origin: {origin}")),
                "a foreign Origin must be forwarded verbatim; upstream saw:\n{got}"
            );
        }
    }

    #[tokio::test]
    async fn with_fetch_metadata_the_engine_gets_both_headers() {
        let got =
            upstream_saw_post("name.ts.net", "https://name.ts.net", Some("same-origin")).await;
        assert!(got.contains("origin: https://name.ts.net"), "{got}");
        assert!(got.contains("sec-fetch-site: same-origin"), "{got}");
    }

    #[test]
    fn an_origin_with_no_host_is_not_vouched_for() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "http://100.70.0.1:5252".parse().unwrap());
        assert!(!vouches_for_origin(&headers));
        headers.insert(header::HOST, "100.70.0.1:5252".parse().unwrap());
        assert!(vouches_for_origin(&headers));
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(!vouches_for_origin(&headers));
    }

    #[tokio::test]
    async fn the_gateway_vouches_for_its_own_hop() {
        let (_dir, target, captured) = capturing_upstream().await;
        let req = request("GET", "/dev/api/v1/threads/list", Body::empty());
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        assert!(
            got.contains(&format!(
                "{}: {TEST_LOCAL_TOKEN}",
                crate::auth::HEADER_LOCAL_TOKEN
            )),
            "the gateway's own credential must reach the engine; upstream saw:\n{got}"
        );
    }

    #[tokio::test]
    async fn a_client_cannot_supply_either_engine_credential() {
        for spoofed in [
            crate::auth::HEADER_LOCAL_TOKEN,
            crate::auth::HEADER_HOOK_SOCKET_TOKEN,
        ] {
            let (_dir, target, captured) = capturing_upstream().await;
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/dev/api/v1/data/config/apis.json")
                .header(spoofed, "client-forged-secret")
                .body(Body::empty())
                .unwrap();
            let resp = proxy(
                &target,
                "dev",
                "/dev/",
                DEFAULT_LABEL,
                TEST_LOCAL_TOKEN,
                None,
                req,
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK);
            let got = captured.lock().await.to_lowercase();
            assert!(
                !got.contains("client-forged-secret"),
                "a client-supplied {spoofed} must be stripped; upstream saw:\n{got}"
            );
        }
    }

    #[tokio::test]
    async fn the_hook_socket_token_is_never_injected_on_the_proxy_path() {
        let (_dir, target, captured) = capturing_upstream().await;
        let req = request("GET", "/dev/", Body::empty());
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        assert!(
            !got.contains(crate::auth::HEADER_HOOK_SOCKET_TOKEN),
            "the hook socket token has no business on this path; upstream saw:\n{got}"
        );
    }

    #[tokio::test]
    async fn forwards_the_authenticated_device_id() {
        let (_dir, target, captured) = capturing_upstream().await;
        let mut req = request("GET", "/dev/", Body::empty());
        req.extensions_mut().insert(test_device("device-1"));
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        assert!(
            got.contains("x-lucidos-device-id: device-1"),
            "the authenticated device id must reach the engine; upstream saw:\n{got}"
        );
        assert!(
            got.contains(TEST_DEVICE_LABEL_HEADER),
            "the pairing label must reach the engine, percent-encoded; upstream saw:\n{got}"
        );
    }

    #[tokio::test]
    async fn strips_client_spoofed_device_id() {
        let (_dir, target, captured) = capturing_upstream().await;
        let mut req = axum::http::Request::builder()
            .method("GET")
            .uri("/dev/")
            .header("x-lucidos-device-id", "someone-elses-device")
            .header("x-lucidos-device-label", "someone-elses-label")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(test_device("device-1"));
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        assert!(
            got.contains("x-lucidos-device-id: device-1"),
            "the gateway's own device id must be forwarded; upstream saw:\n{got}"
        );
        assert!(
            !got.contains("someone-elses-device"),
            "a client-spoofed device id must be stripped; upstream saw:\n{got}"
        );
        assert!(
            !got.contains("someone-elses-label") && got.contains(TEST_DEVICE_LABEL_HEADER),
            "a client-spoofed label must be replaced by the gateway's; upstream saw:\n{got}"
        );
    }

    #[tokio::test]
    async fn forwards_no_device_id_without_an_authenticated_one() {
        let (_dir, target, captured) = capturing_upstream().await;
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/dev/")
            .header("x-lucidos-device-id", "unproven-device")
            .header("x-lucidos-device-label", "unproven-label")
            .body(Body::empty())
            .unwrap();
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        assert!(
            !got.contains("x-lucidos-device-id") && !got.contains("x-lucidos-device-label"),
            "no authenticated device means no forwarded id or label; upstream saw:\n{got}"
        );
    }

    fn captured_stamp(got: &str) -> Option<String> {
        got.lines().find_map(|line| {
            line.strip_prefix(&format!(
                "{}: ",
                lucidos_gateway_stamp::HEADER_GATEWAY_STAMP
            ))
            .map(|v| v.trim().to_string())
        })
    }

    #[tokio::test]
    async fn signs_the_authenticated_device_for_the_engine() {
        let (_dir, target, captured) = capturing_upstream().await;
        let key = StampKey::generate().unwrap();
        let mut req = request("GET", "/dev/", Body::empty());
        req.extensions_mut().insert(test_device("device-1"));
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            Some(&key),
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        let stamp = captured_stamp(&got).expect("the forwarded device carries a stamp");
        let claim = |device_id| lucidos_gateway_stamp::Claim::Device {
            workspace_id: "dev",
            device_id,
        };
        assert!(key.verify_now(&stamp, claim("device-1")), "{stamp}");
        assert!(!key.verify_now(&stamp, claim("device-2")), "{stamp}");
    }

    #[tokio::test]
    async fn strips_a_client_stamp_and_signs_nothing_without_a_key() {
        for device in [None, Some("device-1")] {
            let (_dir, target, captured) = capturing_upstream().await;
            let mut req = axum::http::Request::builder()
                .method("GET")
                .uri("/dev/")
                .header(lucidos_gateway_stamp::HEADER_GATEWAY_STAMP, "v1.1.forged")
                .body(Body::empty())
                .unwrap();
            if let Some(id) = device {
                req.extensions_mut().insert(test_device(id));
            }
            let resp = proxy(
                &target,
                "dev",
                "/dev/",
                DEFAULT_LABEL,
                TEST_LOCAL_TOKEN,
                None,
                req,
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK);
            let got = captured.lock().await.to_lowercase();
            assert_eq!(
                captured_stamp(&got),
                None,
                "{device:?}: upstream saw:\n{got}"
            );
        }
    }

    #[tokio::test]
    async fn strips_a_client_workspace_call_claim() {
        let (_dir, target, captured) = capturing_upstream().await;
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/dev/api/v1/chat/stream")
            .header(lucidos_gateway_stamp::HEADER_CALLER_WORKSPACE, "work2")
            .header(lucidos_gateway_stamp::HEADER_CALLER_THREAD, "t1")
            .header(lucidos_gateway_stamp::HEADER_CALLER_VERDICT, "allowed")
            .header(lucidos_gateway_stamp::HEADER_DIAGNOSTIC_ORIGIN, "source")
            .header(lucidos_gateway_stamp::HEADER_DIAGNOSTIC_DELIVERY, "once")
            .header(
                lucidos_gateway_stamp::HEADER_DIAGNOSTIC_DIGEST,
                "a".repeat(64),
            )
            .header("x-lucidos-diagnostic-relay-signature", "forged")
            .body(Body::empty())
            .unwrap();
        let key = StampKey::generate().unwrap();
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            Some(&key),
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        for name in [
            lucidos_gateway_stamp::HEADER_DIAGNOSTIC_ORIGIN,
            lucidos_gateway_stamp::HEADER_DIAGNOSTIC_DELIVERY,
            lucidos_gateway_stamp::HEADER_DIAGNOSTIC_DIGEST,
            "x-lucidos-diagnostic-relay-signature",
            lucidos_gateway_stamp::HEADER_CALLER_WORKSPACE,
            lucidos_gateway_stamp::HEADER_CALLER_THREAD,
            lucidos_gateway_stamp::HEADER_CALLER_VERDICT,
        ] {
            assert!(!got.contains(name), "{name}: upstream saw:\n{got}");
        }
        assert_eq!(captured_stamp(&got), None, "upstream saw:\n{got}");
    }

    #[tokio::test]
    async fn no_trailing_slash_redirects() {
        let resp = proxy(
            &test_engine::unreachable(&test_engine::scratch()),
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            request("GET", "/dev", Body::empty()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/dev/");
    }

    #[tokio::test]
    async fn unreachable_engine_serves_starting_page_with_boot_label() {
        let dir = test_engine::scratch();
        let target = test_engine::unreachable(&dir);
        let resp = proxy(
            &target,
            "dev",
            "/dev/",
            "Running migrations…",
            TEST_LOCAL_TOKEN,
            None,
            request("GET", "/dev/", Body::empty()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers().get(header::RETRY_AFTER).unwrap(), "2");
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

    #[tokio::test]
    async fn an_unreachable_engine_gives_a_frame_the_quiet_page_not_the_splash() {
        let dir = test_engine::scratch();
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/dev/app/clock/")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", "iframe")
            .body(Body::empty())
            .unwrap();
        let resp = proxy(
            &test_engine::unreachable(&dir),
            "dev",
            "/dev/",
            "Starting engine…",
            TEST_LOCAL_TOKEN,
            None,
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(resp.headers().get("x-lucidos-boot-splash").is_none());
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(!html.contains("boot-splash"), "{html}");
        assert!(!html.contains("Starting engine"), "{html}");
        assert!(
            html.contains(&format!(
                r#"<meta http-equiv="refresh" content="{STARTING_POLL_SECS}">"#
            )),
            "the frame must reload itself without a fetch: {html}"
        );
    }

    #[tokio::test]
    async fn a_response_streams_through_chunk_by_chunk() {
        use futures::StreamExt as _;
        let (release, held) = tokio::sync::oneshot::channel::<()>();
        let held = Arc::new(std::sync::Mutex::new(Some(held)));
        let app = axum::Router::new().route(
            "/api/v1/events",
            axum::routing::get(move || {
                let held = held.lock().unwrap().take().expect("one request");
                async move {
                    let first = futures::stream::once(async {
                        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"data: one\n\n"))
                    });
                    let second = futures::stream::once(async move {
                        let _ = held.await;
                        Ok(axum::body::Bytes::from_static(b"data: two\n\n"))
                    });
                    Body::from_stream(first.chain(second))
                }
            }),
        );
        let dir = test_engine::scratch();
        let engine = test_engine::serve(&dir, app);
        let resp = proxy(
            &engine,
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            request("GET", "/dev/api/v1/events", Body::empty()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let mut body = resp.into_body().into_data_stream();
        let first = body.next().await.expect("a first chunk").expect("readable");
        assert_eq!(&first[..], b"data: one\n\n");
        release.send(()).expect("the engine is still streaming");
        let second = body
            .next()
            .await
            .expect("a second chunk")
            .expect("readable");
        assert_eq!(&second[..], b"data: two\n\n");
    }

    #[tokio::test]
    async fn a_strangers_engine_socket_receives_nothing_and_answers_502() {
        let dir = test_engine::scratch();
        let (target, recorded) = test_engine::record_one(&dir);
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/dev/api/v1/chat")
            .header(header::COOKIE, "lucidos_device=paired-secret")
            .body(Body::from("private message"))
            .unwrap();
        let resp = proxy(
            &target.held_by_a_stranger(),
            "dev",
            "/dev/",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            None,
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        assert!(resp.headers().get("x-lucidos-boot-splash").is_none());
        assert!(
            recorded.await.expect("recorder").is_empty(),
            "a stranger's socket must read zero bytes"
        );
    }
}

#[cfg(test)]
mod upgrade_tests {
    use super::proxy_tests::{test_device, TEST_DEVICE_LABEL_HEADER};
    use super::*;
    use crate::boot_phase::DEFAULT_LABEL;
    use crate::engine_hop::test_engine;
    use std::sync::Arc;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn upgrade_request_bytes(path: &str, extra: &str) -> String {
        format!(
            "GET {path} HTTP/1.1\r\n\
             Host: gateway.example\r\n\
             Connection: keep-alive, Upgrade\r\n\
             Upgrade: websocket\r\n\
             Sec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             {extra}\r\n"
        )
    }

    async fn read_head(sock: &mut (impl AsyncRead + Unpin)) -> String {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while sock.read_exact(&mut byte).await.is_ok() {
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&head).into_owned()
    }

    async fn upgrading_upstream() -> (
        tempfile::TempDir,
        EngineAddress,
        Arc<tokio::sync::Mutex<String>>,
    ) {
        let dir = test_engine::scratch();
        let (listener, addr) = test_engine::bind(&dir);
        let captured = Arc::new(tokio::sync::Mutex::new(String::new()));
        let c = captured.clone();
        tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            *c.lock().await = read_head(&mut sock).await.to_lowercase();
            let _ = sock
                .write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\n\
                      Upgrade: websocket\r\n\
                      Connection: Upgrade\r\n\
                      Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
                )
                .await;
            let mut buf = vec![0u8; 1024];
            while let Ok(n) = sock.read(&mut buf).await {
                if n == 0 || sock.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        });
        (dir, addr, captured)
    }

    async fn gateway_serving(target: EngineAddress, device: Option<&'static str>) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = axum::Router::new().fallback(move |mut req: axum::extract::Request| {
            let target = target.clone();
            async move {
                if let Some(id) = device {
                    req.extensions_mut().insert(test_device(id));
                }
                proxy(
                    &target,
                    "dev",
                    "/dev/",
                    DEFAULT_LABEL,
                    TEST_LOCAL_TOKEN,
                    None,
                    req,
                )
                .await
            }
        });
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        port
    }

    async fn upgrade_through(port: u16, extra: &str) -> (String, tokio::net::TcpStream) {
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        client
            .write_all(upgrade_request_bytes("/dev/api/v1/ws-echo", extra).as_bytes())
            .await
            .unwrap();
        let head = read_head(&mut client).await;
        (head, client)
    }

    #[tokio::test]
    async fn an_upgrade_crosses_the_gateway_and_bytes_flow_both_ways() {
        let (_dir, upstream, captured) = upgrading_upstream().await;
        let gw = gateway_serving(upstream, None).await;
        let (head, mut client) = upgrade_through(gw, "").await;

        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        assert!(
            head.to_lowercase().contains("sec-websocket-accept:"),
            "the engine's handshake answer must reach the client: {head}"
        );

        client.write_all(b"\x82\x03abc").await.unwrap();
        let mut back = [0u8; 5];
        client.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"\x82\x03abc");

        let got = captured.lock().await.clone();
        assert!(got.contains("get /api/v1/ws-echo http/1.1"), "{got}");
        assert!(got.contains("upgrade: websocket"), "{got}");
        assert!(got.contains("sec-websocket-key:"), "{got}");
        assert!(got.contains("x-forwarded-prefix: /dev/"), "{got}");
    }

    #[tokio::test]
    async fn the_handover_headers_survive_the_upgrade_path() {
        let (_dir, upstream, captured) = upgrading_upstream().await;
        let gw = gateway_serving(upstream, None).await;
        let (head, _client) = upgrade_through(gw, "").await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let got = captured.lock().await.clone();
        assert!(got.contains("connection:"), "{got}");
        assert!(got.contains("upgrade: websocket"), "{got}");
    }

    #[tokio::test]
    async fn the_upgrade_path_forwards_the_authenticated_device() {
        let (_dir, upstream, captured) = upgrading_upstream().await;
        let gw = gateway_serving(upstream, Some("device-1")).await;
        let (head, _client) = upgrade_through(gw, "").await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let got = captured.lock().await.clone();
        assert!(got.contains("x-lucidos-device-id: device-1"), "{got}");
        assert!(got.contains(TEST_DEVICE_LABEL_HEADER), "{got}");
    }

    #[tokio::test]
    async fn the_upgrade_path_strips_a_spoofed_device_id_and_prefix() {
        let (_dir, upstream, captured) = upgrading_upstream().await;
        let gw = gateway_serving(upstream, Some("device-1")).await;
        let (head, _client) = upgrade_through(
            gw,
            "x-lucidos-device-id: someone-elses-device\r\n\
             x-lucidos-device-label: someone-elses-label\r\n\
             x-forwarded-prefix: /evil/\r\n\
             x-forwarded-host: evil.example\r\n",
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let got = captured.lock().await.clone();
        assert!(got.contains("x-lucidos-device-id: device-1"), "{got}");
        assert!(!got.contains("someone-elses-device"), "{got}");
        assert!(!got.contains("someone-elses-label"), "{got}");
        assert!(got.contains(TEST_DEVICE_LABEL_HEADER), "{got}");
        assert!(got.contains("x-forwarded-prefix: /dev/"), "{got}");
        assert!(!got.contains("/evil/"), "{got}");
        assert!(!got.contains("evil.example"), "{got}");
    }

    #[tokio::test]
    async fn an_upgrade_with_no_authenticated_device_forwards_none() {
        let (_dir, upstream, captured) = upgrading_upstream().await;
        let gw = gateway_serving(upstream, None).await;
        let (head, _client) = upgrade_through(
            gw,
            "x-lucidos-device-id: unproven-device\r\n\
             x-lucidos-device-label: unproven-label\r\n",
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let got = captured.lock().await.clone();
        assert!(!got.contains("x-lucidos-device-id"), "{got}");
        assert!(!got.contains("x-lucidos-device-label"), "{got}");
    }

    #[tokio::test]
    async fn a_handshake_from_our_own_page_upgrades_and_leaves_its_origin_here() {
        let (_dir, upstream, captured) = upgrading_upstream().await;
        let gw = gateway_serving(upstream, None).await;
        let (head, _client) = upgrade_through(gw, "Origin: https://gateway.example\r\n").await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let got = captured.lock().await.clone();
        assert!(
            !got.contains("origin:"),
            "the engine would refuse a handshake carrying one: {got}"
        );
    }

    #[tokio::test]
    async fn a_handshake_from_another_page_is_refused_and_the_engine_never_dialled() {
        let (_dir, upstream, captured) = upgrading_upstream().await;
        let gw = gateway_serving(upstream, None).await;
        for origin in ["http://localhost:5252", "https://gateway.example:10000"] {
            let (head, _client) = upgrade_through(gw, &format!("Origin: {origin}\r\n")).await;
            assert!(head.starts_with("HTTP/1.1 403"), "{origin}: {head}");
        }
        assert!(
            captured.lock().await.is_empty(),
            "a refused handshake must not reach the engine at all"
        );
    }

    #[test]
    fn a_default_port_is_filled_in_before_comparing() {
        assert!(origin_authority_matches_host(
            "https://name.ts.net",
            "name.ts.net"
        ));
        assert!(origin_authority_matches_host(
            "https://name.ts.net:10000",
            "name.ts.net:10000"
        ));
        assert!(origin_authority_matches_host(
            "http://LOCALHOST:5251",
            "localhost:5251"
        ));
        assert!(!origin_authority_matches_host(
            "http://localhost:5252",
            "localhost:5251"
        ));
        assert!(!origin_authority_matches_host("not a url", "localhost"));
    }

    #[test]
    fn a_portless_host_matches_the_default_port_and_no_other() {
        assert!(!origin_authority_matches_host(
            "https://name.ts.net:10000",
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

    #[tokio::test]
    async fn an_upgrade_to_a_strangers_socket_sends_nothing_and_answers_502() {
        let dir = test_engine::scratch();
        let (upstream, recorded) = test_engine::record_one(&dir);
        let gw = gateway_serving(upstream.held_by_a_stranger(), Some("device-1")).await;
        let (head, _client) = upgrade_through(gw, "").await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        assert!(
            recorded.await.expect("recorder").is_empty(),
            "a stranger's socket must read zero bytes"
        );
    }

    #[tokio::test]
    async fn a_declined_upgrade_forwards_the_engines_own_answer() {
        let dir = test_engine::scratch();
        let (listener, upstream) = test_engine::bind(&dir);
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                read_head(&mut sock).await;
                let _ = sock
                    .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nno socket")
                    .await;
                let _ = sock.flush().await;
            }
        });
        let gw = gateway_serving(upstream, None).await;
        let (head, _client) = upgrade_through(gw, "").await;
        assert!(head.starts_with("HTTP/1.1 404"), "{head}");
    }

    #[tokio::test]
    async fn an_unreachable_engine_answers_503_rather_than_a_boot_splash() {
        let dir = test_engine::scratch();
        let gw = gateway_serving(test_engine::unreachable(&dir), None).await;
        let (head, _client) = upgrade_through(gw, "").await;
        assert!(head.starts_with("HTTP/1.1 503"), "{head}");
        assert!(!head.to_lowercase().contains("text/html"), "{head}");
    }

    #[test]
    fn a_connection_list_still_names_the_upgrade() {
        let req = |conn: &str, upgrade: &str| {
            axum::http::Request::builder()
                .uri("/dev/api/v1/ws-echo")
                .header(header::CONNECTION, conn)
                .header(header::UPGRADE, upgrade)
                .body(Body::empty())
                .unwrap()
        };
        assert!(is_websocket_upgrade(&req(
            "keep-alive, Upgrade",
            "websocket"
        )));
        assert!(is_websocket_upgrade(&req("Upgrade", "WebSocket")));
        assert!(!is_websocket_upgrade(&req("keep-alive", "websocket")));
        assert!(!is_websocket_upgrade(&req("Upgrade", "h2c")));
        let plain = axum::http::Request::builder()
            .uri("/dev/")
            .body(Body::empty())
            .unwrap();
        assert!(!is_websocket_upgrade(&plain));
    }

    #[test]
    fn the_two_paths_differ_only_in_the_handover_and_the_origin() {
        for owned in [
            header::HOST.as_str(),
            header::CONTENT_LENGTH.as_str(),
            "x-forwarded-prefix",
            "x-forwarded-host",
            crate::stack::HEADER_DEVICE_ID,
            lucidos_gateway_stamp::HEADER_GATEWAY_STAMP,
            lucidos_gateway_stamp::HEADER_DEVICE_SESSION,
            "transfer-encoding",
            "te",
        ] {
            let name = HeaderName::from_static(owned);
            assert!(gateway_owns_header(&name, false), "{owned}");
            assert!(gateway_owns_header(&name, true), "{owned}");
        }
        for handover in ["connection", "upgrade"] {
            let name = HeaderName::from_static(handover);
            assert!(gateway_owns_header(&name, false), "{handover}");
            assert!(!gateway_owns_header(&name, true), "{handover}");
        }
        let origin = HeaderName::from_static("origin");
        assert!(!gateway_owns_header(&origin, false));
        assert!(gateway_owns_header(&origin, true));
        for forwarded in [
            "accept-encoding",
            "sec-websocket-key",
            "cookie",
            "sec-fetch-site",
        ] {
            let name = HeaderName::from_static(forwarded);
            assert!(!gateway_owns_header(&name, false), "{forwarded}");
            assert!(!gateway_owns_header(&name, true), "{forwarded}");
        }
    }
    #[test]
    fn request_logs_omit_credentials_in_queries_and_fragments() {
        for target in [
            "/demo/?pair=01234567",
            "/demo/#pair=01234567",
            "/demo/?x=1&pair=secret",
        ] {
            assert_eq!(request_path_for_log(target), "/demo/");
        }
        assert_eq!(
            request_path_for_log("/demo/api/v1/health"),
            "/demo/api/v1/health"
        );
    }
}
