use super::*;
use crate::api::thread_reach::refuse_without_authority;
use crate::engine::command_permission::{command_checkpoint_origin, CommandCheckpointOrigin};
use crate::engine::permission::Action;

#[derive(Deserialize)]
pub(super) struct CommandCheckpointUndoRequest {
    pub checkpoint_id: String,
}

pub(super) async fn undo_command_checkpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CommandCheckpointUndoRequest>,
) -> impl IntoResponse {
    let origin =
        match authorize_checkpoint_undo(&state.engine.event_bus, &headers, &body.checkpoint_id)
            .await
        {
            Ok(origin) => origin,
            Err(refusal) => return refusal.into_response(),
        };
    let actor = super::actor::user_actor(&headers, None);
    match state
        .engine
        .undo_command_checkpoint(&body.checkpoint_id, origin, actor)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => {
            crate::log_legacy!(
                "[CommandCheckpoint] undo failed for {}: {}",
                body.checkpoint_id,
                e
            );
            (StatusCode::BAD_REQUEST, format!("[ERROR] {e}")).into_response()
        }
    }
}

pub(in crate::api) async fn authorize_checkpoint_undo(
    bus: &crate::engine::event_bus::EventBus,
    headers: &HeaderMap,
    checkpoint_id: &str,
) -> Result<CommandCheckpointOrigin, (StatusCode, String)> {
    let origin = command_checkpoint_origin(bus.pool(), checkpoint_id)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("[ERROR] {e}")))?
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("[ERROR] no command checkpoint with id {checkpoint_id}"),
            )
        })?;
    refuse_without_authority(bus, headers, Some(origin.thread_id), Action::Revert)
        .await
        .map_err(|e| (e.status_code(), format!("[ERROR] {e}")))?;
    Ok(origin)
}

#[derive(Deserialize)]
pub(super) struct CommandCheckpointDiffQuery {
    pub checkpoint_id: String,
}

#[derive(Serialize)]
pub(super) struct CommandCheckpointDiff {
    #[serde(flatten)]
    pub diff: super::diff::RepoDiff,
    pub reclaimed: bool,
}

pub(super) async fn get_command_checkpoint_diff(
    State(state): State<AppState>,
    Query(query): Query<CommandCheckpointDiffQuery>,
) -> Result<Json<CommandCheckpointDiff>, ApiError> {
    let checkpoint_id = Uuid::parse_str(&query.checkpoint_id)
        .map_err(|_| ApiError::bad_request("Invalid checkpoint id"))?
        .to_string();

    let known: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM events \
         WHERE event_type = 'CommandCheckpointed' \
           AND payload->>'checkpoint_id' = $1 \
         LIMIT 1",
    )
    .bind(&checkpoint_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::db)?;
    if known.is_none() {
        return Err(ApiError::not_found("Command checkpoint not found"));
    }

    let workspace = state.engine.workspace_path();
    if !crate::engine::git_ops::checkpoint_pair_available(workspace, &checkpoint_id).await {
        return Ok(Json(CommandCheckpointDiff {
            diff: super::diff::RepoDiff { files: Vec::new() },
            reclaimed: true,
        }));
    }

    let pre = crate::engine::git_ops::command_checkpoint_ref(&checkpoint_id);
    let post = crate::engine::git_ops::command_post_image_ref(&checkpoint_id);
    let output = crate::engine::git_ops::git_cmd(&["diff", &pre, &post, "--no-color"], workspace)
        .await
        .map_err(ApiError::internal)?;
    if !output.status.success() {
        return Err(ApiError::internal(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(Json(CommandCheckpointDiff {
        diff: super::diff::RepoDiff {
            files: super::diff::parse_diff_output(&String::from_utf8_lossy(&output.stdout)),
        },
        reclaimed: false,
    }))
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/command-checkpoint/undo", post(undo_command_checkpoint))
        .route("/command-checkpoint/diff", get(get_command_checkpoint_diff))
}
