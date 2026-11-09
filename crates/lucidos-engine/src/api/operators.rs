//! The calling device's person, as the gateway knows them (ADR 0623).

use axum::{http::HeaderMap, routing::get, Json, Router};

use super::AppState;
use crate::gateway_operators::Reader;

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/operators/me", get(me))
}

/// GET /api/v1/operators/me: the devices whose reads count as the caller's.
/// `devices` is null when the caller reads as the whole team: a gateway with
/// fewer than two people, or a device no person holds. A page reads its
/// Review group through it.
async fn me(headers: HeaderMap) -> Json<serde_json::Value> {
    let reader = Reader::of(
        crate::api::actor::user_actor(&headers, None).as_ref(),
        &crate::gateway_operators::current(),
    );
    Json(serde_json::json!({ "devices": reader.devices() }))
}
