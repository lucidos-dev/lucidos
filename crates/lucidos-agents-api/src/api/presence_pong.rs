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
    StatusCode::OK
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/presence-pong", post(presence_pong))
}
