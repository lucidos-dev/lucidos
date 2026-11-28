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

const ENGINE_PAIRING_REQUESTS_CHANGED_PATH: &str = "/api/v1/pairing-requests/changed";

fn kind_name(kind: PairingKind) -> &'static str {
    match kind {
        PairingKind::Device => "device",
        PairingKind::Gateway => "gateway",
    }
}

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

pub(crate) const DECLINED: &str = "the owner declined this pairing on a paired device";

pub(crate) fn minter(device: Option<&Extension<AuthenticatedDevice>>) -> Minter {
    match device {
        Some(_) => Minter::PairedDevice,
        None => Minter::LocalProcess,
    }
}

pub(crate) fn awaiting_owner() -> Response {
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "paired": false,
            "awaiting_owner": true,
            "message": "waiting for the owner to tap Allow in Settings, Access on any device \
                        already paired here. A code started from a paired device needs no \
                        Allow. Never move or edit the gateway's files to skip this.",
        })),
    )
        .into_response()
}

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
    crate::log_legacy!("[Gateway] {} {verb} a pairing request", device.label);
    signal_engines(&state, None);
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[path = "pairing_requests_tests.rs"]
mod tests;
