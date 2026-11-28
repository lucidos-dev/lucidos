use crate::engine::chat::process::footprint_sections::FootprintSectionsExt;
use axum::{extract::State, routing::get, Json, Router};

use super::error::ApiError;
use super::AppState;
use crate::engine::prompt_footprint::WorkspacePromptFootprint;

async fn get_workspace_prompt_footprint(
    State(state): State<AppState>,
) -> Result<Json<WorkspacePromptFootprint>, ApiError> {
    state
        .engine
        .workspace_prompt_footprint()
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(format!("Failed to measure the prompt footprint: {e}")))
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/workspace-prompt-footprint",
        get(get_workspace_prompt_footprint),
    )
}
