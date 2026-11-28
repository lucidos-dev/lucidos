use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

use super::error::ApiError;
use super::AppState;
use crate::engine::frontend_preview::FrontendPreviewStatus;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/frontend-preview", get(get_status))
        .route("/frontend-preview/start", post(start))
        .route("/frontend-preview/stop", post(stop))
}

#[derive(Debug, Deserialize)]
struct StartRequest {
    thread_id: Uuid,
}

async fn get_status(State(state): State<AppState>) -> Json<FrontendPreviewStatus> {
    Json(state.engine.frontend_preview_status().await)
}

async fn start(
    State(state): State<AppState>,
    Json(req): Json<StartRequest>,
) -> Result<Json<FrontendPreviewStatus>, ApiError> {
    state
        .engine
        .start_frontend_preview(req.thread_id)
        .await
        .map(Json)
        .map_err(ApiError::bad_request)
}

async fn stop(State(state): State<AppState>) -> Json<FrontendPreviewStatus> {
    Json(state.engine.stop_frontend_preview().await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_returns_the_gateway_path_without_guessing_an_origin() {
        let status = FrontendPreviewStatus {
            running: true,
            thread_id: Some(Uuid::nil()),
            path: Some("/~preview/0123456789abcdef/dev/".into()),
            started_at: None,
            worktree: None,
        };
        let json = serde_json::to_value(status).unwrap();
        assert_eq!(json["path"], "/~preview/0123456789abcdef/dev/");
        assert!(json.get("port").is_none());
        assert!(json.get("url").is_none());
    }
}
