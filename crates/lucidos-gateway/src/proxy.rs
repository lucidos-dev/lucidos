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
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
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

/// Does the gateway own this request header, rather than forwarding it?
///
/// The trust boundary, and one definition for both paths. Five names are the
/// gateway's: `x-forwarded-prefix`, `x-forwarded-host`, `x-lucidos-device-id`
/// and the two engine credentials. A client-supplied one must never reach the
/// engine. The caller re-injects the prefix, the device id and the local token
/// with its own values, and whoever sends the upstream request re-frames `HOST`
/// and `CONTENT_LENGTH`.
///
/// The two credentials are here for the reason the device id is, one step
/// sharper. A wide-bound engine reads them as authorization
/// (`lucidos_engine::api::local_auth`), so a client-chosen value would BE the
/// authorization. Only the hook socket presents the webhook one.
///
/// `keep_handover` marks the upgrade path, where three headers differ.
/// `connection` and `upgrade` are kept: there they ARE the handshake, not
/// framing to discard (ADR 0151). `origin` goes the other way. The HTTP path
/// forwards it for the engine's gate, and the upgrade path judges it here, in
/// [`foreign_handshake_origin`], and consumes it (ADR 0163).
fn gateway_owns_header(name: &HeaderName, keep_handover: bool) -> bool {
    let n = name.as_str();
    if name == header::HOST
        || name == header::CONTENT_LENGTH
        || n.eq_ignore_ascii_case("x-forwarded-prefix")
        || n.eq_ignore_ascii_case("x-forwarded-host")
        || n.eq_ignore_ascii_case(crate::stack::HEADER_DEVICE_ID)
        || n.eq_ignore_ascii_case(crate::auth::HEADER_LOCAL_TOKEN)
        || n.eq_ignore_ascii_case(crate::auth::HEADER_WEBHOOK_TOKEN)
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

/// Whether `origin` and `host` name the same authority, with the scheme's
/// default port filled in so `https://name.ts.net` matches `name.ts.net`.
///
/// A copy of the engine's `api::browser_origin` function of the same name. This
/// crate has no dependency on the engine, and its Cargo header says a small
/// shared surface is duplicated rather than shared. [`is_hop_by_hop`] above is
/// the same trade.
///
/// The hostname-only arm takes an https origin on its default port, and nothing
/// else. `port()` is `None` for exactly that, since the URL parser normalizes a
/// default port away. Two gateways run on one machine, so unguarded a portless
/// `Host` would match a page on the other one's port.
///
/// A `Host` carries no scheme, and every topology sending a portless one reaches
/// us on 443. So `http://name` is a page on port 80, not this origin.
fn origin_authority_matches_host(origin: &str, host: &str) -> bool {
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

/// The page this handshake came from, when it is not one of ours.
///
/// A browser sends no `Sec-Fetch-Site` on a WebSocket handshake: Chromium and
/// Gecko send no fetch metadata there at all. So the engine's same-origin gate
/// has no input, and behind this hop its `Host` is the internal upstream rather
/// than what the client dialled. The question is answered here instead, in the
/// one place that still holds the client's own authority.
///
/// `Origin` carries the answer unforgeably, on the same terms `Sec-Fetch-Site`
/// does for an ordinary request: the `WebSocket` constructor takes no headers,
/// so page script cannot touch it. A handshake with no `Origin` is not from a
/// browser and is left to the bind topology, exactly as the engine's gate
/// leaves it. An `Origin` with no `Host` to compare against is refused rather
/// than guessed at, for the same reason the engine refuses it.
fn foreign_handshake_origin(headers: &HeaderMap) -> Option<&str> {
    let origin = headers.get(header::ORIGIN)?.to_str().ok()?;
    let ours = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|host| origin_authority_matches_host(origin, host));
    (!ours).then_some(origin)
}

/// Is this a WebSocket upgrade request?
///
/// Both halves are required. `Connection` is a comma-separated LIST and a
/// browser sends `keep-alive, Upgrade`, so a whole-value compare misses it.
/// `Upgrade` names the protocol, and only websocket takes the splice path.
fn is_websocket_upgrade(req: &axum::extract::Request) -> bool {
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
    local_token: &str,
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

    if is_websocket_upgrade(&req) {
        return proxy_upgrade(target_base, rest, slug, local_token, req).await;
    }

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
    //   - `x-lucidos-device-id` is re-injected from the AUTHENTICATED device, and
    //     only when there is one. The engine keys push, preferences and actor
    //     attribution on it, so a forged value would let a caller act as any
    //     device. `enforce` stamps the extension; a client cannot.
    let authenticated_device = req
        .extensions()
        .get::<crate::auth::AuthenticatedDevice>()
        .cloned();
    let mut builder = client.request(method.clone(), &url);
    for (name, value) in req.headers() {
        if gateway_owns_header(name, false) {
            continue;
        }
        builder = builder.header(name.as_str(), value);
    }
    let forwarded_prefix = format!("/{slug}/");
    builder = builder.header("x-forwarded-prefix", &forwarded_prefix);
    if let Some(crate::auth::AuthenticatedDevice(id)) = authenticated_device {
        builder = builder.header(crate::stack::HEADER_DEVICE_ID, id);
    }
    // Prove to the engine that this hop is the gateway. An engine on a wide
    // bind requires it; a loopback one ignores it. Sent unconditionally so the
    // two topologies take one code path, and safe to send either way: the
    // engine is a co-located process that could read the same file.
    //
    // This is the gateway vouching for a hop it has ALREADY authorized, not the
    // caller's own credential. `enforce` ran as a router layer, so an unpaired
    // request never reaches here (ADR 0094).
    if let Some(value) = crate::auth::sensitive_credential(local_token) {
        builder = builder.header(crate::auth::HEADER_LOCAL_TOKEN, value);
    }

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

/// How long the upgrade hop waits to reach the engine. Matches the HTTP path's
/// `connect_timeout`, so a down engine fails the same way on both.
const UPGRADE_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Proxy a WebSocket upgrade to the engine, then splice the two byte streams.
///
/// Transparent by construction (ADR 0151): the gateway never parses a frame, so
/// subprotocols, extensions, ping, pong and close all pass through with no code
/// here to get them wrong. It carries the SAME rules as the HTTP path, because
/// both read the trust boundary out of [`gateway_owns_header`].
///
/// `enforce` has already run as a router layer, so an unpaired caller never
/// reaches this. That is the auth boundary, and it is inherited rather than
/// re-implemented.
async fn proxy_upgrade(
    target_base: &str,
    rest: &str,
    slug: &str,
    local_token: &str,
    mut req: axum::extract::Request,
) -> Response {
    // The same-origin question, answered where the client's own authority still
    // is. Refused before the engine is dialled, so a foreign page cannot even
    // open the connection. See `foreign_handshake_origin` for why this hop owns
    // the question rather than the engine's gate.
    if let Some(origin) = foreign_handshake_origin(req.headers()) {
        // Named in full, because the browser hides a refused handshake's
        // response and this log line is the only place the reason appears.
        crate::log!(
            "[Gateway] websocket upgrade to /{}{} refused: it came from {}, and this gateway \
             is reached at {:?}",
            slug,
            rest,
            origin,
            req.headers().get(header::HOST),
        );
        return (
            StatusCode::FORBIDDEN,
            "cross-origin websocket handshakes are not allowed",
        )
            .into_response();
    }
    // Plain http only. `engine_tls` is false in both shipped topologies, and
    // only the retired pre-0096 one turns it on. A TLS leg here would be code
    // nothing reaches, so refuse loudly instead of failing obscurely.
    let Some(authority) = target_base.strip_prefix("http://") else {
        crate::log!(
            "[Gateway] websocket upgrade to {} refused: the upgrade hop carries plain http only",
            target_base
        );
        return (
            StatusCode::BAD_GATEWAY,
            "websocket upgrade needs a plain-http engine hop",
        )
            .into_response();
    };

    // Taken before the request is consumed. Absent when hyper saw no
    // upgradeable connection, which means HTTP/2 or a client that cannot hand
    // over. Nothing to splice either way.
    let Some(client_upgrade) = req.extensions_mut().remove::<hyper::upgrade::OnUpgrade>() else {
        return (StatusCode::BAD_REQUEST, "not an upgradeable connection").into_response();
    };

    let mut upstream = hyper::Request::builder()
        .method(req.method().clone())
        .uri(rest)
        .header(header::HOST, authority);
    for (name, value) in req.headers() {
        if !gateway_owns_header(name, true) {
            upstream = upstream.header(name, value);
        }
    }
    upstream = upstream.header("x-forwarded-prefix", format!("/{slug}/"));
    if let Some(crate::auth::AuthenticatedDevice(id)) =
        req.extensions().get::<crate::auth::AuthenticatedDevice>()
    {
        upstream = upstream.header(crate::stack::HEADER_DEVICE_ID, id);
    }
    // The upgrade is a second way in, so it meets the same door (ADR 0151).
    // Omitting it here would let a wide-bound engine refuse the socket while
    // serving every HTTP request beside it.
    if let Some(value) = crate::auth::sensitive_credential(local_token) {
        upstream = upstream.header(crate::auth::HEADER_LOCAL_TOKEN, value);
    }
    let Ok(upstream) = upstream.body(axum::body::Body::empty()) else {
        return (StatusCode::BAD_REQUEST, "malformed upgrade request").into_response();
    };

    let connect = tokio::time::timeout(
        UPGRADE_CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect(authority),
    );
    let stream = match connect.await {
        Ok(Ok(s)) => s,
        // A cold-booting engine looks exactly like this, so 503 rather than
        // 502: the condition clears by itself and a client may retry.
        //
        // The two failures are logged apart because they mean different
        // things. Refused is nobody listening yet; timed out is a host too
        // busy to accept. Collapsing them loses the refusal's reason, which is
        // the common one.
        Ok(Err(e)) => {
            crate::log!(
                "[Gateway] websocket upgrade to {}{} could not connect: {}",
                target_base,
                rest,
                e
            );
            return (StatusCode::SERVICE_UNAVAILABLE, "engine unreachable").into_response();
        }
        Err(_) => {
            crate::log!(
                "[Gateway] websocket upgrade to {}{} timed out after {:?}",
                target_base,
                rest,
                UPGRADE_CONNECT_TIMEOUT
            );
            return (StatusCode::SERVICE_UNAVAILABLE, "engine unreachable").into_response();
        }
    };

    let (mut sender, conn) =
        match hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await {
            Ok(pair) => pair,
            Err(e) => {
                crate::log!("[Gateway] websocket upgrade handshake failed: {}", e);
                return (StatusCode::BAD_GATEWAY, "upgrade handshake failed").into_response();
            }
        };
    // `with_upgrades` is what lets the connection hand its socket over rather
    // than closing it after the 101.
    tokio::spawn(conn.with_upgrades());

    let engine_response = match sender.send_request(upstream).await {
        Ok(r) => r,
        Err(e) => {
            crate::log!("[Gateway] websocket upgrade request failed: {}", e);
            return (StatusCode::BAD_GATEWAY, "upgrade request failed").into_response();
        }
    };

    // The engine declined the upgrade (404, 401, anything). Forward its answer
    // verbatim, so the client sees the engine's reason and not ours.
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

    // Rebuild the 101 verbatim, framing headers included. On this path they are
    // the handover the client's own hyper needs to see.
    let mut out = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    for (name, value) in engine_response.headers().iter() {
        out = out.header(name, value);
    }
    let upstream_upgrade = hyper::upgrade::on(engine_response);

    tokio::spawn(async move {
        let (client_io, engine_io) = match tokio::try_join!(client_upgrade, upstream_upgrade) {
            Ok(pair) => pair,
            Err(e) => {
                crate::log!("[Gateway] websocket upgrade never completed: {}", e);
                return;
            }
        };
        let mut client_io = hyper_util::rt::TokioIo::new(client_io);
        let mut engine_io = hyper_util::rt::TokioIo::new(engine_io);
        // Either side closing ends the splice, which is the ordinary way a
        // call ends. Only the rest is worth a line.
        if let Err(e) = tokio::io::copy_bidirectional(&mut client_io, &mut engine_io).await {
            if e.kind() != std::io::ErrorKind::ConnectionReset
                && e.kind() != std::io::ErrorKind::BrokenPipe
            {
                crate::log!("[Gateway] websocket relay ended: {}", e);
            }
        }
    });

    out.body(Body::empty())
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

/// The gateway's own credential, as the test modules below spell it.
///
/// Deliberately not a value any test client sends, so "the gateway's own
/// reached the upstream" and "the client's did not" stay separate assertions.
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
        let resp = proxy(
            &build_client(),
            &target,
            "dev",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
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
        let resp = proxy(
            &build_client(),
            &target,
            "dev",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
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
        let resp = proxy(
            &build_client(),
            &target,
            "dev",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
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

    #[tokio::test]
    async fn the_gateway_vouches_for_its_own_hop() {
        // A wide-bound engine requires this, and a loopback one ignores it. It
        // is sent either way so the two topologies take one code path.
        let (port, captured) = capturing_upstream().await;
        let target = format!("http://127.0.0.1:{port}");
        let req = request("GET", "/dev/api/v1/threads/list", Body::empty());
        let resp = proxy(
            &build_client(),
            &target,
            "dev",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
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
        // The escalation this closes. A wide-bound engine reads both headers as
        // authorization, so a value the caller chose would BE the
        // authorization. The gateway owns both names and re-injects only its
        // own local token.
        for spoofed in [
            crate::auth::HEADER_LOCAL_TOKEN,
            crate::auth::HEADER_WEBHOOK_TOKEN,
        ] {
            let (port, captured) = capturing_upstream().await;
            let target = format!("http://127.0.0.1:{port}");
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/dev/api/v1/data/config/apis.json")
                .header(spoofed, "client-forged-secret")
                .body(Body::empty())
                .unwrap();
            let resp = proxy(
                &build_client(),
                &target,
                "dev",
                DEFAULT_LABEL,
                TEST_LOCAL_TOKEN,
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
    async fn the_webhook_credential_is_never_injected_on_the_proxy_path() {
        // Only the hook socket presents it. Injecting it here would hand every
        // proxied request a second credential, and widen what a proxy bug leaks.
        let (port, captured) = capturing_upstream().await;
        let target = format!("http://127.0.0.1:{port}");
        let req = request("GET", "/dev/", Body::empty());
        let resp = proxy(
            &build_client(),
            &target,
            "dev",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        assert!(
            !got.contains(crate::auth::HEADER_WEBHOOK_TOKEN),
            "the webhook scope has no business on this path; upstream saw:\n{got}"
        );
    }

    #[tokio::test]
    async fn forwards_the_authenticated_device_id() {
        // The engine keys push, per-device preferences and actor attribution on
        // this header, so it must carry the device the gateway authenticated.
        let (port, captured) = capturing_upstream().await;
        let target = format!("http://127.0.0.1:{port}");
        let mut req = request("GET", "/dev/", Body::empty());
        req.extensions_mut()
            .insert(crate::auth::AuthenticatedDevice("device-1".into()));
        let resp = proxy(
            &build_client(),
            &target,
            "dev",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        assert!(
            got.contains("x-lucidos-device-id: device-1"),
            "the authenticated device id must reach the engine; upstream saw:\n{got}"
        );
    }

    #[tokio::test]
    async fn strips_client_spoofed_device_id() {
        // A client-chosen device id would let any paired caller act as any other
        // device. The gateway's own value is the only one allowed through.
        let (port, captured) = capturing_upstream().await;
        let target = format!("http://127.0.0.1:{port}");
        let mut req = axum::http::Request::builder()
            .method("GET")
            .uri("/dev/")
            .header("x-lucidos-device-id", "someone-elses-device")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(crate::auth::AuthenticatedDevice("device-1".into()));
        let resp = proxy(
            &build_client(),
            &target,
            "dev",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
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
    }

    #[tokio::test]
    async fn forwards_no_device_id_without_an_authenticated_one() {
        // A local process holds no device row, so it must not be handed one.
        // With no extension the header is stripped and nothing replaces it.
        let (port, captured) = capturing_upstream().await;
        let target = format!("http://127.0.0.1:{port}");
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/dev/")
            .header("x-lucidos-device-id", "unproven-device")
            .body(Body::empty())
            .unwrap();
        let resp = proxy(
            &build_client(),
            &target,
            "dev",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
            req,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let got = captured.lock().await.to_lowercase();
        assert!(
            !got.contains("x-lucidos-device-id"),
            "no authenticated device means no forwarded id; upstream saw:\n{got}"
        );
    }

    #[tokio::test]
    async fn no_trailing_slash_redirects() {
        let resp = proxy(
            &build_client(),
            "http://127.0.0.1:1",
            "dev",
            DEFAULT_LABEL,
            TEST_LOCAL_TOKEN,
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
            TEST_LOCAL_TOKEN,
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

/// The WebSocket upgrade hop (ADR 0151). Two things must hold and neither is
/// visible from the HTTP tests above: the handshake survives the proxy, and the
/// trust boundary is the same one.
#[cfg(test)]
mod upgrade_tests {
    use super::*;
    use crate::boot_phase::DEFAULT_LABEL;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A client's upgrade request, as a browser sends it.
    ///
    /// `Connection` carries a LIST, which is what browsers do and what a
    /// whole-value compare would miss.
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

    /// Read a request or response head, stopping at the blank line.
    async fn read_head(sock: &mut tokio::net::TcpStream) -> String {
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

    /// A stand-in engine: completes the handshake, then echoes every byte.
    ///
    /// Raw TCP rather than a WebSocket library, deliberately. What these tests
    /// must prove is that BYTES cross intact, because the gateway never parses
    /// a frame.
    async fn upgrading_upstream() -> (u16, Arc<tokio::sync::Mutex<String>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
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
        (port, captured)
    }

    /// A gateway serving one route through the real [`proxy`], with an
    /// optionally-authenticated device. `axum::serve` drives connections with
    /// upgrades, which is what puts `OnUpgrade` in the request extensions.
    async fn gateway_serving(target: String, device: Option<&'static str>) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = axum::Router::new().fallback(move |mut req: axum::extract::Request| {
            let target = target.clone();
            async move {
                if let Some(id) = device {
                    req.extensions_mut()
                        .insert(crate::auth::AuthenticatedDevice(id.into()));
                }
                proxy(
                    &build_client(),
                    &target,
                    "dev",
                    DEFAULT_LABEL,
                    TEST_LOCAL_TOKEN,
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

    /// Open an upgrade through a gateway and return its response head plus the
    /// live socket.
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

    /// The whole point: an upgrade crosses the gateway and bytes flow both
    /// ways afterwards. A relay that re-framed messages could pass the
    /// handshake half of this and still corrupt the payload.
    #[tokio::test]
    async fn an_upgrade_crosses_the_gateway_and_bytes_flow_both_ways() {
        let (upstream_port, captured) = upgrading_upstream().await;
        let gw = gateway_serving(format!("http://127.0.0.1:{upstream_port}"), None).await;
        let (head, mut client) = upgrade_through(gw, "").await;

        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        assert!(
            head.to_lowercase().contains("sec-websocket-accept:"),
            "the engine's handshake answer must reach the client: {head}"
        );

        // Bytes, not frames. The gateway is transparent, so whatever is
        // written arrives unchanged.
        client.write_all(b"\x82\x03abc").await.unwrap();
        let mut back = [0u8; 5];
        client.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"\x82\x03abc");

        // The engine saw a real handshake, with the prefix stripped.
        let got = captured.lock().await.clone();
        assert!(got.contains("get /api/v1/ws-echo http/1.1"), "{got}");
        assert!(got.contains("upgrade: websocket"), "{got}");
        assert!(got.contains("sec-websocket-key:"), "{got}");
        assert!(got.contains("x-forwarded-prefix: /dev/"), "{got}");
    }

    /// The framing headers are the one thing the upgrade path must NOT strip.
    /// The HTTP path drops both as hop-by-hop, and an engine that never sees
    /// them refuses the handshake.
    #[tokio::test]
    async fn the_handover_headers_survive_the_upgrade_path() {
        let (upstream_port, captured) = upgrading_upstream().await;
        let gw = gateway_serving(format!("http://127.0.0.1:{upstream_port}"), None).await;
        let (head, _client) = upgrade_through(gw, "").await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let got = captured.lock().await.clone();
        assert!(got.contains("connection:"), "{got}");
        assert!(got.contains("upgrade: websocket"), "{got}");
    }

    /// Same trust boundary as the HTTP path: the authenticated device reaches
    /// the engine, which keys push, preferences and attribution on it.
    #[tokio::test]
    async fn the_upgrade_path_forwards_the_authenticated_device() {
        let (upstream_port, captured) = upgrading_upstream().await;
        let gw = gateway_serving(
            format!("http://127.0.0.1:{upstream_port}"),
            Some("device-1"),
        )
        .await;
        let (head, _client) = upgrade_through(gw, "").await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let got = captured.lock().await.clone();
        assert!(got.contains("x-lucidos-device-id: device-1"), "{got}");
    }

    /// A second path through the gateway is a second place to forget the trust
    /// boundary. A forged device id would let a paired caller act as any other
    /// device. It must die here exactly as it does on the HTTP path.
    #[tokio::test]
    async fn the_upgrade_path_strips_a_spoofed_device_id_and_prefix() {
        let (upstream_port, captured) = upgrading_upstream().await;
        let gw = gateway_serving(
            format!("http://127.0.0.1:{upstream_port}"),
            Some("device-1"),
        )
        .await;
        let (head, _client) = upgrade_through(
            gw,
            "x-lucidos-device-id: someone-elses-device\r\n\
             x-forwarded-prefix: /evil/\r\n\
             x-forwarded-host: evil.example\r\n",
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let got = captured.lock().await.clone();
        assert!(got.contains("x-lucidos-device-id: device-1"), "{got}");
        assert!(!got.contains("someone-elses-device"), "{got}");
        assert!(got.contains("x-forwarded-prefix: /dev/"), "{got}");
        assert!(!got.contains("/evil/"), "{got}");
        assert!(!got.contains("evil.example"), "{got}");
    }

    /// A local process holds no device row, so nothing is handed to it. The
    /// stripped forgery is not replaced by anything.
    #[tokio::test]
    async fn an_upgrade_with_no_authenticated_device_forwards_none() {
        let (upstream_port, captured) = upgrading_upstream().await;
        let gw = gateway_serving(format!("http://127.0.0.1:{upstream_port}"), None).await;
        let (head, _client) = upgrade_through(gw, "x-lucidos-device-id: unproven-device\r\n").await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let got = captured.lock().await.clone();
        assert!(!got.contains("x-lucidos-device-id"), "{got}");
    }

    /// A browser handshake carries an `Origin` and no fetch metadata, because
    /// Chromium and Gecko send none on one. The engine's own gate therefore has
    /// nothing to decide with: behind this hop its `Host` is the internal
    /// upstream, so the fallback would compare our own page against the wrong
    /// authority and refuse it. The question is answered here, and the header
    /// is consumed with it.
    #[tokio::test]
    async fn a_handshake_from_our_own_page_upgrades_and_leaves_its_origin_here() {
        let (upstream_port, captured) = upgrading_upstream().await;
        let gw = gateway_serving(format!("http://127.0.0.1:{upstream_port}"), None).await;
        let (head, _client) = upgrade_through(gw, "Origin: https://gateway.example\r\n").await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let got = captured.lock().await.clone();
        assert!(
            !got.contains("origin:"),
            "the engine would refuse a handshake carrying one: {got}"
        );
    }

    /// The attack this closes. A page on another port of this machine is
    /// same-site, so its cookie rides along and pairing alone lets it through.
    #[tokio::test]
    async fn a_handshake_from_another_page_is_refused_and_the_engine_never_dialled() {
        let (upstream_port, captured) = upgrading_upstream().await;
        let gw = gateway_serving(format!("http://127.0.0.1:{upstream_port}"), None).await;
        // Another host, then the same host on another port. The second is the
        // sharper one: this `Host` carries no port, so a hostname-only compare
        // would have waved it through.
        for origin in ["http://localhost:5252", "https://gateway.example:10000"] {
            let (head, _client) = upgrade_through(gw, &format!("Origin: {origin}\r\n")).await;
            assert!(head.starts_with("HTTP/1.1 403"), "{origin}: {head}");
        }
        assert!(
            captured.lock().await.is_empty(),
            "a refused handshake must not reach the engine at all"
        );
    }

    /// A phone's handshake compares a MagicDNS name against itself, because
    /// `tailscale serve` passes the client's `Host` through. Both shipped serve
    /// shapes are here: the browser omits `:443` from `Host` and writes it in
    /// neither half of the `:10000` one.
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

    /// Two gateways run on one machine, each with its own `tailscale serve`
    /// route. A portless `Host` means the default port, so the page on the
    /// other route is foreign. Matching by hostname alone was the bypass.
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
        // Port 80 is not port 443. A `Host` carries no scheme, so a plain-http
        // page of the same name is another origin, not this one.
        assert!(!origin_authority_matches_host(
            "http://name.ts.net",
            "name.ts.net"
        ));
        assert!(!origin_authority_matches_host(
            "http://name.ts.net:443",
            "name.ts.net"
        ));
    }

    /// An engine that declines answers for itself. The gateway must not turn a
    /// 404 into its own error, or the client cannot tell a missing route from
    /// a broken proxy.
    #[tokio::test]
    async fn a_declined_upgrade_forwards_the_engines_own_answer() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let upstream_port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                read_head(&mut sock).await;
                let _ = sock
                    .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nno socket")
                    .await;
                let _ = sock.flush().await;
            }
        });
        let gw = gateway_serving(format!("http://127.0.0.1:{upstream_port}"), None).await;
        let (head, _client) = upgrade_through(gw, "").await;
        assert!(head.starts_with("HTTP/1.1 404"), "{head}");
    }

    /// A cold-booting engine looks like a refused connection. 503 says the
    /// condition clears by itself, where 502 would read as a dead end.
    #[tokio::test]
    async fn an_unreachable_engine_answers_503_rather_than_a_boot_splash() {
        let dead = {
            let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            l.local_addr().unwrap().port()
        };
        let gw = gateway_serving(format!("http://127.0.0.1:{dead}"), None).await;
        let (head, _client) = upgrade_through(gw, "").await;
        assert!(head.starts_with("HTTP/1.1 503"), "{head}");
        // An HTML boot splash is meaningless to a socket, so it is not served.
        assert!(!head.to_lowercase().contains("text/html"), "{head}");
    }

    /// `Connection` is a comma-separated list, and a browser sends
    /// `keep-alive, Upgrade`. A whole-value compare would send every browser
    /// upgrade down the HTTP path, where `reqwest` silently drops it.
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
        // Some other protocol is not ours to splice.
        assert!(!is_websocket_upgrade(&req("Upgrade", "h2c")));
        // A plain request never takes the path.
        let plain = axum::http::Request::builder()
            .uri("/dev/")
            .body(Body::empty())
            .unwrap();
        assert!(!is_websocket_upgrade(&plain));
    }

    /// The trust boundary is one predicate for both paths, and only three
    /// headers differ. Pinned here so a future edit cannot widen the upgrade
    /// path's exception past them.
    #[test]
    fn the_two_paths_differ_only_in_the_handover_and_the_origin() {
        for owned in [
            header::HOST.as_str(),
            header::CONTENT_LENGTH.as_str(),
            "x-forwarded-prefix",
            "x-forwarded-host",
            crate::stack::HEADER_DEVICE_ID,
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
        // The other way round. An ordinary request's `Origin` is the engine
        // gate's to read, and stripping it there would blind the gate on the
        // one path where it works.
        let origin = HeaderName::from_static("origin");
        assert!(!gateway_owns_header(&origin, false));
        assert!(gateway_owns_header(&origin, true));
        // `sec-fetch-site` crosses both paths on purpose. WebKit sends it on a
        // handshake, page script cannot forge it, and it tells same-site from
        // cross-site where a host compare cannot.
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
}
