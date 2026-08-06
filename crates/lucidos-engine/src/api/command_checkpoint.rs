//! HTTP surface for command checkpoints (ADR 0002, Phase 4 and the 2026-08-06
//! addendum). Two endpoints, both driven by the `CommandCheckpointed` card:
//! **undo**, which restores the workspace from the checkpoint's pre image and
//! removes what the command created, and **diff**, which shows what that
//! command changed so the Undo is not a blind button.

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

#[derive(Deserialize)]
pub(super) struct CommandCheckpointDiffQuery {
    pub checkpoint_id: String,
}

/// What one checkpointed command changed, in the same shape every other diff
/// surface returns so the frontend renders it with the same components.
/// `reclaimed` is what distinguishes an empty diff that means "the snapshots
/// behind this card are gone" from one that means "nothing changed", which the
/// modal explains rather than rendering as no files.
#[derive(Serialize)]
pub(super) struct CommandCheckpointDiff {
    #[serde(flatten)]
    pub diff: super::diff::RepoDiff,
    pub reclaimed: bool,
}

/// GET /api/v1/command-checkpoint/diff?checkpoint_id=... : the diff between a
/// checkpoint's pre and post images.
///
/// The id is parsed as a UUID and looked up before it reaches a ref name. It
/// arrives from a query string, and the two checkpoint namespaces are addressed
/// by interpolating it. 400 on a malformed id, 404 on one with no
/// `CommandCheckpointed` row.
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
    // An unavailable pair (reclaimed by the retention sweep, or written before
    // the post image existed) is a normal outcome for an old card, not an
    // error, so it renders as an explanation. Probed directly rather than via
    // `diff_checkpoint_effects`, whose classification work would be computed
    // and thrown away here.
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

/// Routes for the command-checkpoint card (ADR 0002, Phase 4): restore the
/// workspace from a ReversibleDanger snapshot, and show what that command did.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/command-checkpoint/undo", post(undo_command_checkpoint))
        .route("/command-checkpoint/diff", get(get_command_checkpoint_diff))
}
