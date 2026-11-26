//! The gateway hops: a device on gateway A acts on gateway B through A.
//!
//! A serves `/~/api/v1/remote-gateways/<name><path>/*` to its own devices. It
//! forwards each call to B's `/~/api/gateway/v1<path>/*` with its gateway
//! credential. A claim names the calling device, signed like a relay claim
//! but for the hop's fixed claim prefix. B serves the call only while its
//! pairing record for A carries the hop's grant.
//!
//! Two hops share this code. [`MANAGE`] reaches B's users and devices (ADR
//! 0656). [`CONTROL`] creates, starts, stops and restores B's workspaces (ADR
//! 0811). [`forward`] is A's half, [`receiver`] is B's.

use std::time::Duration;

use axum::body::Body;
use axum::extract::{OriginalUri, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;

use crate::auth::AuthenticatedDevice;
use crate::gateway_protocol::{
    AuthenticatedGateway, CONTROL_PATH, CONTROL_PROTOCOL, GATEWAY_API_PREFIX,
    HEADER_GATEWAY_CREDENTIAL, MANAGE_PATH, MANAGE_PROTOCOL,
};
use crate::relay::{self, ClaimRefusal, RelayClaim};
use crate::remote_gateways::RemoteGateway;
use crate::server::GatewayState;

/// One hop: where it lives, what B checks, and what A passes through.
pub struct Hop {
    /// Relative to [`GATEWAY_API_PREFIX`] on B, and to the remote gateway's
    /// own root on A.
    pub path: &'static str,
    /// The prefix every claim for this hop names. `~` is never a workspace
    /// slug, so a workspace claim cannot carry it. Each hop's prefix differs,
    /// so no claim verifies on another hop.
    pub claim_prefix: &'static str,
    /// The first `GATEWAY_PROTOCOL` whose B serves this hop.
    pub min_protocol: u32,
    /// B's refusal kind when the pairing lacks the grant.
    pub not_granted_kind: &'static str,
    /// What the grant lets A's devices do, completing "does not let devices
    /// on A ...".
    grant_scope: &'static str,
    /// Completes "is too old for ...".
    capability: &'static str,
    granted: fn(&AuthenticatedGateway) -> bool,
    max_body_bytes: usize,
    timeout: Duration,
    /// Request headers A copies to B, beside `Content-Type`.
    request_headers: &'static [HeaderName],
    /// Response headers A copies back, beside `Content-Type` and `Location`.
    response_headers: &'static [HeaderName],
    routes: fn() -> Router<GatewayState>,
}

/// B's users and devices (ADR 0656).
pub static MANAGE: Hop = Hop {
    path: MANAGE_PATH,
    claim_prefix: "/~/manage/",
    min_protocol: MANAGE_PROTOCOL,
    not_granted_kind: "manage-not-granted",
    grant_scope: "manage its users and devices",
    capability: "managing its users and devices from here",
    granted: |gateway| gateway.may_manage,
    // Every allowed call sends a name, an email or a user id.
    max_body_bytes: 64 * 1024,
    timeout: crate::peer_connection::PEER_TIMEOUT,
    request_headers: &[],
    response_headers: &[],
    routes: manage_routes,
};

/// B's workspaces: create, start, stop and restore (ADR 0811).
pub static CONTROL: Hop = Hop {
    path: CONTROL_PATH,
    claim_prefix: "/~/control/",
    min_protocol: CONTROL_PROTOCOL,
    not_granted_kind: "control-not-granted",
    grant_scope: "create, start, stop or restore its workspaces",
    capability: "creating or restoring workspaces from here",
    granted: |gateway| gateway.may_control,
    // One restore chunk, and room for its headers' worth of slack.
    max_body_bytes: MAX_CHUNK_BYTES + 64 * 1024,
    // A chunk crosses the link from A to B within one request.
    timeout: Duration::from_secs(300),
    request_headers: &crate::restore_upload::TUS_REQUEST_HEADERS,
    response_headers: &crate::restore_upload::TUS_RESPONSE_HEADERS,
    routes: crate::control::hop_routes,
};

/// The largest restore chunk the client sends: `FIRST_CHUNK_BYTES` in
/// `resumableUpload.ts`. A forwards exactly one chunk per request (ADR 0811).
pub const MAX_CHUNK_BYTES: usize = 32 * 1024 * 1024;

/// A's refusal when B predates the hop.
pub const REASON_TOO_OLD: &str = "remote-gateway-too-old";

/// A's answer when B refuses A's credential or claim.
pub const REASON_REMOTE_REFUSED: &str = "remote-refused";

/// A's answer when B answers with something other than JSON or plain text.
pub const REASON_UNEXPECTED_ANSWER: &str = "remote-unexpected-answer";

fn manage_routes() -> Router<GatewayState> {
    Router::new()
        .merge(crate::auth_api::device_routes())
        .merge(crate::users_api::routes())
}

/// B's router for `hop`, nested at its path behind `require_gateway`.
///
/// Exactly the hop's allow-list (ADRs 0656, 0811). Everything else answers
/// 404.
pub fn receiver(hop: &'static Hop) -> Router<GatewayState> {
    (hop.routes)()
        .route_layer(middleware::from_fn_with_state(hop, require_grant))
        .fallback(|| async { StatusCode::NOT_FOUND })
}

impl Hop {
    /// B's check of a claim: signed by the presented credential, fresh, for
    /// this hop's prefix, and naming a device.
    pub fn verify_claim(
        &self,
        headers: &HeaderMap,
        credential: &str,
        now: i64,
    ) -> Result<RelayClaim, ClaimRefusal> {
        let claim = relay::verify_signed_claim(headers, credential, now, |prefix| {
            prefix == self.claim_prefix
        })?;
        if claim.device.is_none() {
            return Err(ClaimRefusal::NoDevice);
        }
        Ok(claim)
    }

    fn name(&self) -> &'static str {
        self.path.trim_start_matches('/')
    }
}

/// B's line naming who changed something through a hop, or `None` for a call
/// that only reads. A restore chunk is a `PATCH`, and its commit names it.
pub fn attribution(
    hop: &Hop,
    method: &Method,
    path: &str,
    device: &str,
    gateway: &str,
) -> Option<String> {
    if matches!(*method, Method::GET | Method::HEAD | Method::PATCH) {
        return None;
    }
    Some(format!(
        "[Gateway] {} {method} {path} by '{device}' via '{gateway}'",
        hop.name()
    ))
}

/// Refuse a call from a pairing without the hop's grant, or without a good
/// claim. The handlers then see no device of B's own, so a call never counts
/// as the caller revoking itself.
async fn require_grant(State(hop): State<&'static Hop>, mut req: Request, next: Next) -> Response {
    let Some(gateway) = req.extensions().get::<AuthenticatedGateway>().cloned() else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !(hop.granted)(&gateway) {
        return refusal(
            StatusCode::FORBIDDEN,
            hop.not_granted_kind,
            format!(
                "This gateway does not let devices on '{}' {}. \
                 On this gateway, open Settings, Access, Paired gateways and turn it on.",
                gateway.label, hop.grant_scope
            ),
        );
    }
    let credential = req
        .headers()
        .get(HEADER_GATEWAY_CREDENTIAL)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    let claim = match hop.verify_claim(req.headers(), credential, now) {
        Ok(claim) => claim,
        Err(refused) => {
            crate::log_legacy!(
                "[Gateway] refused a {} call from '{}': {}",
                hop.name(),
                gateway.label,
                refused.message()
            );
            return refusal(
                StatusCode::UNAUTHORIZED,
                "bad-claim",
                refused.message().to_string(),
            );
        }
    };
    let device = claim.device.as_ref().map_or("", |d| d.label.as_str());
    if let Some(line) = attribution(hop, req.method(), req.uri().path(), device, &gateway.label) {
        crate::log_legacy!("{line}");
    }
    req.extensions_mut().remove::<AuthenticatedDevice>();
    next.run(req).await
}

/// A typed refusal: the sentence for a person, and a kind for the UI.
fn refusal(status: StatusCode, reason_kind: &str, reason: String) -> Response {
    (
        status,
        axum::Json(serde_json::json!({
            "error": reason,
            "reason_kind": reason_kind,
        })),
    )
        .into_response()
}

/// A's route for the manage hop.
pub async fn forward_manage(
    state: State<GatewayState>,
    path: Path<(String, String)>,
    original: OriginalUri,
    req: Request,
) -> Response {
    forward(&MANAGE, state, path, original, req).await
}

/// A's route for the control hop.
pub async fn forward_control(
    state: State<GatewayState>,
    path: Path<(String, String)>,
    original: OriginalUri,
    req: Request,
) -> Response {
    forward(&CONTROL, state, path, original, req).await
}

/// A's half: forward a device's call to remote gateway `name`, and pass B's
/// answer back.
async fn forward(
    hop: &'static Hop,
    State(state): State<GatewayState>,
    Path((name, _)): Path<(String, String)>,
    OriginalUri(original): OriginalUri,
    req: Request,
) -> Response {
    // The raw path, so an encoded email reaches B still encoded.
    let own_root = format!("/~/api/v1/remote-gateways/{name}{}", hop.path);
    let Some(rest) = original
        .path()
        .strip_prefix(&own_root)
        .filter(|rest| rest.starts_with('/'))
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if crate::auth_api::carries_a_dot_segment(original.path()) {
        return refusal(
            StatusCode::BAD_REQUEST,
            "bad-path",
            format!("a {} path cannot hold a dot segment", hop.name()),
        );
    }
    let Some(device) = req.extensions().get::<AuthenticatedDevice>().cloned() else {
        return refusal(
            StatusCode::FORBIDDEN,
            "no-device",
            "only a paired device may act on a remote gateway".into(),
        );
    };
    let Some(remote) = state
        .remote_gateways()
        .list()
        .into_iter()
        .find(|g| g.name == name)
    else {
        return refusal(
            StatusCode::NOT_FOUND,
            "no-remote-gateway",
            format!("no remote gateway called '{name}'"),
        );
    };
    if let Err(response) = ensure_protocol(hop, &state, &remote).await {
        return response;
    }
    // Sign only once the body is in, so a slow upload cannot age the claim.
    let (parts, body) = req.into_parts();
    let Ok(body) = axum::body::to_bytes(body, hop.max_body_bytes).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Some((_, claim_headers)) = relay::signed_claim_headers(
        &state,
        &remote.credential,
        hop.claim_prefix.to_string(),
        Some(device),
    ) else {
        return refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the hop claim is not a header value".into(),
        );
    };
    let b_root = format!("{GATEWAY_API_PREFIX}{}", hop.path);
    let path = match original.query() {
        Some(query) => format!("{b_root}{rest}?{query}"),
        None => format!("{b_root}{rest}"),
    };
    let mut upstream = match state
        .peer_connection()
        .request(&remote, parts.method, &path)
    {
        Ok(request) => request.headers(claim_headers),
        Err(reason) => return refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal", reason),
    };
    for name in std::iter::once(&header::CONTENT_TYPE).chain(hop.request_headers) {
        if let Some(value) = parts.headers.get(name) {
            upstream = upstream.header(name, value);
        }
    }
    let unreachable = || {
        refusal(
            StatusCode::BAD_GATEWAY,
            "transport",
            format!("The remote gateway '{name}' could not be reached"),
        )
    };
    let response = match upstream.body(body).timeout(hop.timeout).send().await {
        Ok(response) => response,
        Err(e) => {
            let e = e.without_url();
            crate::log_legacy!(
                "[Gateway] {} call to remote gateway '{name}' failed: {}",
                hop.name(),
                crate::external_address::error_chain(&e)
            );
            return unreachable();
        }
    };
    let status = response.status();
    // B refusing A itself is no answer about this device, which a 401 would
    // tell the browser.
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return refusal(
            StatusCode::BAD_GATEWAY,
            REASON_REMOTE_REFUSED,
            format!("The remote gateway '{name}' no longer accepts this gateway"),
        );
    }
    let headers = response.headers().clone();
    let content_type = headers.get(header::CONTENT_TYPE);
    if !content_type.is_none_or(is_inert) {
        crate::log_legacy!(
            "[Gateway] refused a {} answer from remote gateway '{name}': {:?}",
            hop.name(),
            content_type
        );
        // A proxy's own page for a body cap. Keep the 413, so the client
        // halves its chunk (ADR 0810).
        if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
            return refusal(
                StatusCode::PAYLOAD_TOO_LARGE,
                REASON_UNEXPECTED_ANSWER,
                format!("Something in front of the remote gateway '{name}' refused a request this large"),
            );
        }
        return refusal(
            StatusCode::BAD_GATEWAY,
            REASON_UNEXPECTED_ANSWER,
            format!("The remote gateway '{name}' answered with something other than data"),
        );
    }
    let Ok(bytes) = response.bytes().await else {
        return unreachable();
    };
    let mut out = Response::builder()
        .status(status)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    for name in std::iter::once(&header::CONTENT_TYPE).chain(hop.response_headers) {
        if let Some(value) = headers.get(name) {
            out = out.header(name, value);
        }
    }
    if let Some(location) = headers
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix(&b_root))
        .filter(|rest| rest.starts_with('/'))
        .and_then(|rest| HeaderValue::from_str(&format!("{own_root}{rest}")).ok())
    {
        out = out.header(header::LOCATION, location);
    }
    out.body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

/// Can a browser on A take this answer without running anything of B's? JSON
/// and plain text, both sent with `nosniff`.
fn is_inert(content_type: &HeaderValue) -> bool {
    let Ok(value) = content_type.to_str() else {
        return false;
    };
    let media = value.split(';').next().unwrap_or_default().trim();
    media.eq_ignore_ascii_case("application/json") || media.eq_ignore_ascii_case("text/plain")
}

/// Refuse with [`REASON_TOO_OLD`] unless B serves `hop`.
///
/// A learns B's protocol at pairing and on each rotation, so a B updated
/// since then still reads as old. A then asks B's health and keeps the newer
/// number, so it asks only until B is new enough.
async fn ensure_protocol(
    hop: &Hop,
    state: &GatewayState,
    remote: &RemoteGateway,
) -> Result<(), Response> {
    if remote.gateway_protocol >= hop.min_protocol {
        return Ok(());
    }
    let name = &remote.name;
    let health = state
        .peer_connection()
        .health_with_headers(&remote.address, &remote.headers)
        .await
        .map_err(|reason| refusal(StatusCode::BAD_GATEWAY, "transport", reason))?;
    let protocol = health
        .get("gateway_protocol")
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| u32::try_from(p).ok())
        .unwrap_or(0);
    if protocol > remote.gateway_protocol {
        if let Err(e) = state.remote_gateways().set_protocol(name, protocol) {
            crate::log_legacy!(
                "[Gateway] could not record the protocol of remote gateway '{name}': {e}"
            );
        }
    }
    if protocol >= hop.min_protocol {
        return Ok(());
    }
    Err(refusal(
        StatusCode::CONFLICT,
        REASON_TOO_OLD,
        format!(
            "The remote gateway '{name}' is too old for {}. Update it first.",
            hop.capability
        ),
    ))
}

#[cfg(test)]
#[path = "manage_hop_tests.rs"]
mod manage_tests;

#[cfg(test)]
#[path = "control_hop_tests.rs"]
mod control_tests;
