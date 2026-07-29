//! HTTP undo endpoint for command checkpoints (ADR 0002, Phase 4). The Undo
//! button on a `CommandCheckpointed` card POSTs here; the handler restores the
//! workspace from the checkpoint's safety ref and emits
//! `CommandCheckpointReverted`. Mirrors the change discard/revert endpoints.

use super::*;

#[derive(Deserialize)]
pub(super) struct CommandCheckpointUndoRequest {
    pub checkpoint_id: String,
}

/// POST /api/v1/command-checkpoint/undo — restore the workspace from a command
/// checkpoint and mark it reverted. 400 with an `[ERROR]`-prefixed body when the
/// id is unknown or the git restore fails; 200 on success (and on a no-op
/// re-undo of an already-reverted checkpoint).
pub(super) async fn undo_command_checkpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CommandCheckpointUndoRequest>,
) -> impl IntoResponse {
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match state
        .engine
        .undo_command_checkpoint(&body.checkpoint_id, actor)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => {
            crate::log!(
                "[CommandCheckpoint] undo failed for {}: {}",
                body.checkpoint_id,
                e
            );
            (StatusCode::BAD_REQUEST, format!("[ERROR] {e}")).into_response()
        }
    }
}

/// Route for the command-checkpoint undo (ADR 0002, Phase 4) — restore the
/// workspace from a ReversibleDanger snapshot.
pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/command-checkpoint/undo",
        post(undo_command_checkpoint),
    )
}
