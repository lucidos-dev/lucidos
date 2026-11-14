//! Pairings waiting for the owner, and the routes a paired device answers.
//!
//! The local token lets a process mint a pairing, and every subprocess Lucidos
//! starts can read that token. So once any device is paired, a local process's
//! pairing redeems only after a paired device taps Allow (plan invariant 9).
//! The state lives in [`crate::pairing::PendingPairings`]. This module is the
//! HTTP half: who counts as the minter, the waiting and declined answers, and
//! the list and answer routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Serialize;

use crate::auth::AuthenticatedDevice;
use crate::error::ApiError;
use crate::pairing::{Minter, PairingAnswer, PairingKind};
use crate::server::GatewayState;

/// The engine route that notifies the owner's devices and refreshes their
/// Settings, Access page.
const ENGINE_PAIRING_REQUESTS_CHANGED_PATH: &str = "/api/v1/pairing-requests/changed";

/// The wire name of a pairing kind, shared by the list and the engine signal.
fn kind_name(kind: PairingKind) -> &'static str {
    match kind {
        PairingKind::Device => "device",
        PairingKind::Gateway => "gateway",
    }
}

/// Tell every routed engine the waiting requests changed. `raised` names a
/// pairing that just started waiting, so the engines notify the owner's
/// devices. Without it the engines only refresh the list on screen.
pub(crate) fn signal_engines(state: &GatewayState, raised: Option<(PairingKind, &str)>) {
    let body = raised.map(|(kind, label)| {
        serde_json::json!({ "raised": { "kind": kind_name(kind), "label": label } })
    });
    crate::engine_signal::signal_engines(
        state,
        ENGINE_PAIRING_REQUESTS_CHANGED_PATH,
        Some(body.unwrap_or_else(|| serde_json::json!({}))),
        "pairing requests",
    );
}

/// What a redeemer hears after a paired device tapped Don't allow.
pub(crate) const DECLINED: &str = "the owner declined this pairing on a paired device";

/// Who minted a pairing: a paired device only when the gateway resolved one.
/// Anything else, the local token included, is a local process.
pub(crate) fn minter(device: Option<&Extension<AuthenticatedDevice>>) -> Minter {
    match device {
        Some(_) => Minter::PairedDevice,
        None => Minter::LocalProcess,
    }
}

/// The 202 a waiting redeem answers. The redeemer presents the same secret
/// again until it pairs or hears [`DECLINED`].
pub(crate) fn awaiting_owner() -> Response {
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "paired": false,
            "awaiting_owner": true,
            "message": "waiting for the owner to tap Allow in Settings, Access on a device \
                        already paired",
        })),
    )
        .into_response()
}

/// The routes, merged into `auth_api::router` behind its app-document gate.
pub fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/pairing-requests", get(list))
        .route("/pairing-requests/:id/allow", post(allow))
        .route("/pairing-requests/:id/decline", post(decline))
}

#[derive(Serialize)]
struct RequestRow {
    id: String,
    kind: &'static str,
    label: String,
    expires_in_secs: u64,
}

/// The waiting requests. A read, so a local process may list them too.
async fn list(State(state): State<GatewayState>) -> Json<Vec<RequestRow>> {
    Json(
        state
            .pairing_requests()
            .into_iter()
            .map(|(kind, request)| RequestRow {
                id: request.id,
                kind: kind_name(kind),
                label: request.label,
                expires_in_secs: request.expires_in_secs,
            })
            .collect(),
    )
}

async fn allow(
    state: State<GatewayState>,
    device: Option<Extension<AuthenticatedDevice>>,
    id: Path<String>,
) -> Result<StatusCode, ApiError> {
    answer(state, device, id, PairingAnswer::Allow)
}

async fn decline(
    state: State<GatewayState>,
    device: Option<Extension<AuthenticatedDevice>>,
    id: Path<String>,
) -> Result<StatusCode, ApiError> {
    answer(state, device, id, PairingAnswer::Decline)
}

/// Only a paired device answers. A local process minted the pairing, so its
/// own answer would prove nothing.
fn answer(
    State(state): State<GatewayState>,
    device: Option<Extension<AuthenticatedDevice>>,
    Path(id): Path<String>,
    answer: PairingAnswer,
) -> Result<StatusCode, ApiError> {
    let Some(Extension(device)) = device else {
        return Err(ApiError::forbidden(
            "only a paired device can answer a pairing request",
        ));
    };
    if !state
        .answer_pairing_request(&id, answer)
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::not_found("no pairing request with that id"));
    }
    let verb = match answer {
        PairingAnswer::Allow => "allowed",
        PairingAnswer::Decline => "declined",
    };
    crate::log!("[Gateway] {} {verb} a pairing request", device.label);
    signal_engines(&state, None);
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[path = "pairing_requests_tests.rs"]
mod tests;
