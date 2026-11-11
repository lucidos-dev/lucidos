//! The people the gateway knows, and the calling device's person (ADR 0623).

use axum::{http::HeaderMap, routing::get, Json, Router};

use super::AppState;
use crate::gateway_operators::Reader;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/operators/me", get(me))
        .route("/operators/people", get(people))
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

/// GET /api/v1/operators/people: every person the gateway knows, and `you`, the
/// caller's person or null. `team` is true with two or more people: only then
/// does a chat send take mentions (ADR 0649).
async fn people(headers: HeaderMap) -> Json<serde_json::Value> {
    let snapshot = crate::gateway_operators::current();
    let you = crate::api::actor::user_actor(&headers, None)
        .and_then(|actor| snapshot.operator_of_origin(&actor))
        .map(|who| who.id);
    Json(serde_json::json!({
        "people": snapshot.people(),
        "you": you,
        "team": snapshot.person_count() >= 2,
    }))
}
