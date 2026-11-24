//! HTTP endpoint for the PresenceCheck pong inbox.

use axum::{extract::State, http::StatusCode, routing::post, Json, Router};

use crate::api::AppState;
pub use crate::engine::presence_tracker::{
    decide_push_allowed, PresencePong, PresencePongRequest, PresenceTracker,
};

pub async fn presence_pong(
    State(state): State<AppState>,
    Json(req): Json<PresencePongRequest>,
) -> StatusCode {
    state.engine.presence_tracker.record(req);
    // Always 200 — late pongs are normal, no value in surfacing the race
    // back to the page. See spec §3 "Failure handling".
    StatusCode::OK
}

/// Route for the PresenceCheck pong inbox (system-knowhow/notifications/3-the-presencecheck-protocol.md §3).
pub(super) fn router() -> Router<AppState> {
    Router::new().route("/presence-pong", post(presence_pong))
}
