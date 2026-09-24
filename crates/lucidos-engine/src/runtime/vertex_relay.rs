//! The Vertex relay: a loopback hop between Claude Code and Vertex that asks
//! an always-thinking model for its progress notes.
//!
//! Claude Code asks for `thinking.display: "updates"` only on the first-party
//! API. On Vertex, Opus 5.5 and Fable 5.x then send every note as an empty
//! `thinking` block, and the user sees nothing. Claude Code honours
//! `ANTHROPIC_VERTEX_BASE_URL`, so each session points it here. The relay
//! changes the `thinking` value and the beta header, and forwards everything
//! else untouched.
//!
//! A temporary measure: `docs/temporary-measures.md` § "Claude Code's Vertex
//! calls go through the Vertex relay". The design and its alternatives are in
//! `docs/plans/2026-09-23-coding-agent-notes-hidden-on-opus-5-5.md`.

use std::net::Ipv4Addr;
use std::sync::{Arc, OnceLock};

use axum::body::{Body, Bytes};
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde_json::value::RawValue;
use uuid::Uuid;

use crate::llm::anthropic_wire::{
    thinking_mode, ThinkingMode, ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES, DISPLAY_PROGRESS_UPDATES,
};

/// The variable Claude Code reads its Vertex endpoint from.
pub(crate) const ENV_VERTEX_BASE_URL: &str = "ANTHROPIC_VERTEX_BASE_URL";

/// Where the relay's one route lives. Every engine route sits under `/api/v1`,
/// and this one does too, on its own socket.
const ROUTE_PREFIX: &str = "/api/v1/vertex-relay";

/// Signing domain of a relay token, kept apart from the origin token.
const TOKEN_DOMAIN: &str = "vertex-relay";

/// Stands in for "no upstream override" inside a token.
const NO_OVERRIDE: &str = "-";

/// The port this engine's relay listens on, set once at boot.
static RELAY_PORT: OnceLock<u16> = OnceLock::new();

/// Bind the relay on loopback, serve it, and record its port for spawns.
pub async fn start() -> std::io::Result<u16> {
    let port = bind_and_serve(reqwest::Client::builder()).await?;
    if RELAY_PORT.set(port).is_err() {
        crate::log!("[VertexRelay] a relay was already running; keeping the first");
    }
    crate::log!("[VertexRelay] listening on 127.0.0.1:{port}");
    Ok(port)
}

/// Bind `127.0.0.1:0` and serve the relay there. Loopback only: the relay
/// carries the user's Google token, so nothing off this machine may reach it.
async fn bind_and_serve(client: reqwest::ClientBuilder) -> std::io::Result<u16> {
    let client = client
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(std::io::Error::other)?;
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    let app = Router::new()
        .route(&format!("{ROUTE_PREFIX}/:token/*rest"), post(relay))
        .with_state(Arc::new(client));
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            crate::log!("[VertexRelay] server stopped: {e}");
        }
    });
    Ok(port)
}

/// This engine's relay port, or `None` when it runs no relay.
pub(crate) fn port() -> Option<u16> {
    RELAY_PORT.get().copied()
}

/// The `ANTHROPIC_VERTEX_BASE_URL` a Claude Code session for `thread_id` gets.
/// `user_base` is a Vertex base URL the session would otherwise have used; the
/// relay forwards there instead. `None` before the signing secret exists.
pub(crate) fn relay_base_url(
    port: u16,
    thread_id: Uuid,
    user_base: Option<&str>,
) -> Option<String> {
    let token = mint_token(thread_id, user_base.and_then(usable_override))?;
    Some(format!("http://127.0.0.1:{port}{ROUTE_PREFIX}/{token}"))
}

/// Whether `url` is a relay URL this module built.
pub(crate) fn is_relay_url(url: &str) -> bool {
    url.starts_with("http://127.0.0.1:") && url.contains(ROUTE_PREFIX)
}

/// A user's Vertex base URL the relay can forward to. A relay URL is refused:
/// an engine started from inside a session inherits its parent's relay, and
/// chaining through it would tie this session to another engine's lifetime.
fn usable_override(base: &str) -> Option<&str> {
    let base = base.trim().trim_end_matches('/');
    let is_http = base.starts_with("http://") || base.starts_with("https://");
    (is_http && !base.contains(ROUTE_PREFIX)).then_some(base)
}

/// `<thread>.<override as hex, or ->.<mac>`. Every character is URL-safe, so
/// the token sits in a path segment unescaped.
fn mint_token(thread_id: Uuid, upstream_override: Option<&str>) -> Option<String> {
    let upstream = upstream_override
        .map(|base| crate::api::hex::hex_lower(base.as_bytes()))
        .unwrap_or_else(|| NO_OVERRIDE.to_string());
    let payload = format!("{thread_id}.{upstream}");
    let mac = crate::api::actor::sign_in_domain(TOKEN_DOMAIN, &payload)?;
    Some(format!("{payload}.{mac}"))
}

/// What a verified token grants: which thread, and where to forward.
#[derive(Debug, PartialEq, Eq)]
struct Grant {
    thread_id: Uuid,
    upstream_override: Option<String>,
}

fn verify_token(token: &str) -> Option<Grant> {
    let (payload, mac) = token.rsplit_once('.')?;
    if !crate::api::actor::verify_in_domain(TOKEN_DOMAIN, payload, mac) {
        return None;
    }
    let (thread, upstream) = payload.split_once('.')?;
    let upstream_override = match upstream {
        NO_OVERRIDE => None,
        hex => Some(String::from_utf8(crate::api::hex::hex_decode(hex)?).ok()?),
    };
    Some(Grant {
        thread_id: Uuid::parse_str(thread).ok()?,
        upstream_override,
    })
}

/// One Vertex call Claude Code makes, read off its path.
#[derive(Debug, PartialEq, Eq)]
struct VertexCall<'a> {
    location: &'a str,
    model: &'a str,
}

/// Accept only `projects/<p>/locations/<l>/publishers/anthropic/models/<m>:<method>`
/// with a raw-predict method. Anything else is not a model call to relay.
fn parse_vertex_call(path: &str) -> Option<VertexCall<'_>> {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let ["projects", project, "locations", location, "publishers", "anthropic", "models", model_method] =
        segments.as_slice()
    else {
        return None;
    };
    let (model, method) = model_method.split_once(':')?;
    let is_label = |s: &str, extra: &[u8]| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
    };
    let lowercase_location = location
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let ok = is_label(project, b"-_")
        && is_label(location, b"-")
        && lowercase_location
        && is_label(model, b"-_.@")
        && matches!(method, "rawPredict" | "streamRawPredict");
    ok.then_some(VertexCall { location, model })
}

/// The URL a call forwards to. Only a location this module validated, or the
/// session's own override, ever names the host.
fn upstream_url(grant: &Grant, call: &VertexCall<'_>, path: &str, query: Option<&str>) -> String {
    let path = format!("/{}", path.trim_start_matches('/'));
    let base = match &grant.upstream_override {
        Some(base) => base.clone(),
        None => format!(
            "https://{}/v1",
            crate::llm::vertex::vertex_host(call.location)
        ),
    };
    match query {
        Some(q) => format!("{base}{path}?{q}"),
        None => format!("{base}{path}"),
    }
}

/// The body with `thinking.display` set to `"updates"`, or `None` to forward
/// it untouched. Every top-level value but `thinking` keeps its exact bytes.
fn ask_for_progress_notes(body: &[u8], model: &str) -> Option<Vec<u8>> {
    if thinking_mode(model) != Some(ThinkingMode::AlwaysOn) {
        return None;
    }
    let mut fields: std::collections::BTreeMap<String, Box<RawValue>> =
        serde_json::from_slice(body).ok()?;
    let mut thinking: serde_json::Value =
        serde_json::from_str(fields.get("thinking")?.get()).ok()?;
    let settings = thinking.as_object_mut()?;
    let disabled = settings.get("type").and_then(|t| t.as_str()) == Some("disabled");
    let asked = settings.get("display").and_then(|d| d.as_str()) == Some(DISPLAY_PROGRESS_UPDATES);
    if disabled || asked {
        return None;
    }
    settings.insert("display".into(), DISPLAY_PROGRESS_UPDATES.into());
    fields.insert(
        "thinking".into(),
        RawValue::from_string(thinking.to_string()).ok()?,
    );
    serde_json::to_vec(&fields).ok()
}

/// Headers that belong to one hop and never cross the relay.
fn is_hop_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "host"
            | "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
    )
}

/// The caller's headers, minus hop headers. `accept-encoding` goes too, so the
/// upstream answers in plain bytes the relay can stream as they come.
fn forwarded_request_headers(incoming: &HeaderMap, asked_for_notes: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in incoming {
        if !is_hop_header(name) && name != header::ACCEPT_ENCODING {
            headers.append(name.clone(), value.clone());
        }
    }
    if asked_for_notes {
        add_beta(&mut headers);
    }
    headers
}

/// Without the beta, Vertex rejects `display: "updates"` as an unknown value.
fn add_beta(headers: &mut HeaderMap) {
    const BETA: HeaderName = HeaderName::from_static("anthropic-beta");
    let existing: Vec<String> = headers
        .get_all(&BETA)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
        .collect();
    if existing
        .iter()
        .any(|b| b == ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES)
    {
        return;
    }
    let joined = existing
        .into_iter()
        .chain([ANTHROPIC_BETA_THINKING_DISPLAY_UPDATES.to_string()])
        .collect::<Vec<_>>()
        .join(",");
    if let Ok(value) = HeaderValue::from_str(&joined) {
        headers.insert(BETA, value);
    }
}

/// An error in the shape Claude Code reads from the API, so it shows the
/// user the relay's reason rather than a parse failure.
fn relay_error(status: StatusCode, kind: &str, message: String) -> Response {
    let body = serde_json::json!({
        "type": "error",
        "error": { "type": kind, "message": format!("Lucidos Vertex relay: {message}") },
    });
    (status, axum::Json(body)).into_response()
}

async fn relay(
    State(client): State<Arc<reqwest::Client>>,
    Path((token, rest)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let Some(grant) = verify_token(&token) else {
        return relay_error(
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "the relay token does not verify".into(),
        );
    };
    let Some(call) = parse_vertex_call(&rest) else {
        return relay_error(
            StatusCode::NOT_FOUND,
            "not_found_error",
            format!("not a Vertex model call: /{rest}"),
        );
    };
    // Read only now, so a caller without a token cannot make the relay buffer.
    // No size cap: a 1M-token conversation is many megabytes.
    let body = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(body) => body,
        Err(e) => {
            return relay_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                format!("could not read the request body: {e}"),
            )
        }
    };
    let url = upstream_url(&grant, &call, &rest, query.as_deref());
    let rewritten = ask_for_progress_notes(&body, call.model);
    let forwarded_headers = forwarded_request_headers(&headers, rewritten.is_some());
    let forwarded_body = rewritten.map(Bytes::from).unwrap_or(body);
    let upstream = match client
        .post(&url)
        .headers(forwarded_headers)
        .body(forwarded_body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => {
            crate::log!(
                "[VertexRelay] thread {} could not reach {url}: {e}",
                grant.thread_id
            );
            return relay_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                format!("could not reach Vertex: {e}"),
            );
        }
    };
    let mut response = Response::builder().status(upstream.status());
    for (name, value) in upstream.headers() {
        if !is_hop_header(name) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|e| {
            relay_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                format!("could not relay the response: {e}"),
            )
        })
}

#[cfg(test)]
#[path = "vertex_relay_tests.rs"]
mod tests;
