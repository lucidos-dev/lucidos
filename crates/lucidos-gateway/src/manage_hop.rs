//! The manage hop: a device on gateway A manages gateway B's users and
//! devices (ADR 0656).
//!
//! A serves `/~/api/v1/remote-gateways/<name>/manage/*` to its own devices.
//! It forwards each call to B's `/~/api/gateway/v1/manage/*` with its gateway
//! credential. A claim names the calling device, signed like a relay claim
//! but for the fixed prefix [`MANAGE_CLAIM_PREFIX`]. B serves the call only
//! when its pairing record for A carries the manage grant.
//!
//! Both halves live here: [`forward`] is A's, [`receiver`] is B's.

use axum::body::Body;
use axum::extract::{OriginalUri, Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;

use crate::auth::AuthenticatedDevice;
use crate::gateway_protocol::{
    AuthenticatedGateway, GATEWAY_API_PREFIX, HEADER_GATEWAY_CREDENTIAL, MANAGE_PATH,
    MANAGE_PROTOCOL,
};
use crate::relay::{self, ClaimRefusal, RelayClaim};
use crate::remote_gateways::RemoteGateway;
use crate::server::GatewayState;

/// The prefix every manage claim names. `~` is never a workspace slug, so a
/// workspace claim cannot carry it, and [`verify_manage_claim`] takes nothing
/// else.
pub const MANAGE_CLAIM_PREFIX: &str = "/~/manage/";

/// B's refusal when its pairing record for A carries no manage grant.
pub const REASON_NOT_GRANTED: &str = "manage-not-granted";

/// A's refusal when B predates the manage hop.
pub const REASON_TOO_OLD: &str = "remote-gateway-too-old";

/// A's answer when B refuses A's credential or claim.
pub const REASON_REMOTE_REFUSED: &str = "remote-refused";

/// The largest request body A forwards. Every allowed call sends a name, an
/// email or a user id.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// B's manage router, nested at [`MANAGE_PATH`] behind `require_gateway`.
///
/// Exactly the allow-list in ADR 0656. Pairing codes, gateway pairings and
/// the control plane are not here, so they answer 404.
pub fn receiver() -> Router<GatewayState> {
    Router::new()
        .merge(crate::auth_api::device_routes())
        .merge(crate::users_api::routes())
        .route_layer(middleware::from_fn(require_manage))
        .fallback(|| async { StatusCode::NOT_FOUND })
}

/// B's check of a manage claim: signed by the presented credential, fresh,
/// for [`MANAGE_CLAIM_PREFIX`], and naming a device.
pub fn verify_manage_claim(
    headers: &HeaderMap,
    credential: &str,
    now: i64,
) -> Result<RelayClaim, ClaimRefusal> {
    let claim = relay::verify_signed_claim(headers, credential, now, |prefix| {
        prefix == MANAGE_CLAIM_PREFIX
    })?;
    if claim.device.is_none() {
        return Err(ClaimRefusal::NoDevice);
    }
    Ok(claim)
}

/// Refuse a manage call from a pairing without the grant, or without a good
/// claim. The handlers then see no device of B's own, so a call never counts
/// as the caller revoking itself.
async fn require_manage(mut req: Request, next: Next) -> Response {
    let Some(gateway) = req.extensions().get::<AuthenticatedGateway>().cloned() else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !gateway.may_manage {
        return refusal(
            StatusCode::FORBIDDEN,
            REASON_NOT_GRANTED,
            format!(
                "This gateway does not let devices on '{}' manage its users and devices. \
                 On this gateway, open Settings, Access, Paired gateways and turn it on.",
                gateway.label
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
    let claim = match verify_manage_claim(req.headers(), credential, now) {
        Ok(claim) => claim,
        Err(refused) => {
            crate::log!(
                "[Gateway] refused a manage call from '{}': {}",
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
    if req.method() != axum::http::Method::GET {
        crate::log!(
            "[Gateway] {} {} by '{}' via '{}'",
            req.method(),
            req.uri().path(),
            claim.device.as_ref().map_or("", |d| d.label.as_str()),
            gateway.label
        );
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

/// A's half: forward a device's manage call to remote gateway `name`, and
/// pass B's answer back.
pub async fn forward(
    State(state): State<GatewayState>,
    Path((name, _)): Path<(String, String)>,
    OriginalUri(original): OriginalUri,
    req: Request,
) -> Response {
    // The raw path, so an encoded email reaches B still encoded.
    let own_root = format!("/~/api/v1/remote-gateways/{name}{MANAGE_PATH}");
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
            "a manage path cannot hold a dot segment".into(),
        );
    }
    let Some(device) = req.extensions().get::<AuthenticatedDevice>().cloned() else {
        return refusal(
            StatusCode::FORBIDDEN,
            "no-device",
            "only a paired device may manage a remote gateway".into(),
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
    if let Err(response) = ensure_manage_protocol(&state, &remote).await {
        return response;
    }
    let Some(client) = state.peer_connection().client() else {
        return refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the peer client is unavailable".into(),
        );
    };
    let Some((_, claim_headers)) = relay::signed_claim_headers(
        &state,
        &remote.credential,
        MANAGE_CLAIM_PREFIX.to_string(),
        Some(device),
    ) else {
        return refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the manage claim is not a header value".into(),
        );
    };
    let (parts, body) = req.into_parts();
    let Ok(body) = axum::body::to_bytes(body, MAX_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let path = match original.query() {
        Some(query) => format!("{GATEWAY_API_PREFIX}{MANAGE_PATH}{rest}?{query}"),
        None => format!("{GATEWAY_API_PREFIX}{MANAGE_PATH}{rest}"),
    };
    let mut upstream = match client.request(&remote, parts.method, &path) {
        Ok(request) => request.headers(claim_headers),
        Err(reason) => return refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal", reason),
    };
    if let Some(content_type) = parts.headers.get(header::CONTENT_TYPE) {
        upstream = upstream.header(header::CONTENT_TYPE, content_type);
    }
    let unreachable = || {
        refusal(
            StatusCode::BAD_GATEWAY,
            "transport",
            format!("The remote gateway '{name}' could not be reached"),
        )
    };
    let response = match upstream
        .body(body)
        .timeout(crate::peer_connection::PEER_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => {
            let e = e.without_url();
            crate::log!(
                "[Gateway] manage call to remote gateway '{name}' failed: {}",
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
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    let Ok(bytes) = response.bytes().await else {
        return unreachable();
    };
    let mut out = Response::builder().status(status);
    if let Some(content_type) = content_type {
        out = out.header(header::CONTENT_TYPE, content_type);
    }
    out.body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

/// Refuse with [`REASON_TOO_OLD`] unless B serves the manage hop.
///
/// A learns B's protocol at pairing and on each rotation, so a B updated
/// since then still reads as old. A then asks B's health and keeps the newer
/// number, so it asks only until B is new enough.
async fn ensure_manage_protocol(
    state: &GatewayState,
    remote: &RemoteGateway,
) -> Result<(), Response> {
    if remote.gateway_protocol >= MANAGE_PROTOCOL {
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
            crate::log!("[Gateway] could not record the protocol of remote gateway '{name}': {e}");
        }
    }
    if protocol >= MANAGE_PROTOCOL {
        return Ok(());
    }
    Err(refusal(
        StatusCode::CONFLICT,
        REASON_TOO_OLD,
        format!("The remote gateway '{name}' is too old to manage from here. Update it first."),
    ))
}

#[cfg(test)]
#[path = "manage_hop_tests.rs"]
mod tests;
