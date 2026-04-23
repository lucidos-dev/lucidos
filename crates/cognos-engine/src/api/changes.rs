use super::actor::{user_actor, HEADER_DEVICE_ID};
use super::*;
use crate::engine::http::workspace_client::HEADER_WORKSPACE;
use crate::engine::thread_events::MessageOrigin;
use crate::engine::ApplyResult;
use axum::http::HeaderMap;

/// Resolve the actor for a change-mutation request, enriching with the device
/// label so the popover renders "Chrome on Mac" instead of "Unknown device".
/// Skips the DB lookup when a workspace header is present (workspace beats device
/// in `build_message_origin`, so the looked-up label would be discarded).
async fn resolve_change_actor(state: &AppState, headers: &HeaderMap) -> Option<MessageOrigin> {
    let device_label = if headers.contains_key(HEADER_WORKSPACE) {
        None
    } else {
        match headers.get(HEADER_DEVICE_ID).and_then(|v| v.to_str().ok()) {
            Some(did) => crate::core::DeviceStore::display_name(state.engine.pool(), did).await,
            None => None,
        }
    };
    user_actor(headers, None, device_label)
}

pub(super) async fn broadcast_changes(state: &AppState) {
    let pending = crate::core::changes::list_pending(state.engine.pool())
        .await
        .unwrap_or_default();
    let applied = crate::core::changes::list_recently_applied(state.engine.pool(), 15, None)
        .await
        .unwrap_or_default();
    let restart_groups =
        crate::core::changes::restart_groups_since(state.engine.pool(), state.started_at)
            .await
            .unwrap_or_default();
    state
        .engine
        .event_bus
        .emit_or_log(
            crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::ChangesUpdated {
                    total_pending: pending.len(),
                    pending,
                    applied,
                    restart_required: !restart_groups.is_empty(),
                },
            ),
            "[Changes] ChangesUpdated",
        )
        .await;
}

/// GET /api/changes — list pending + applied changes with pagination for applied
pub(super) async fn list_changes(
    State(state): State<AppState>,
    Query(query): Query<ChangesListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let limit = query.limit.unwrap_or(15).clamp(1, 100);
    let before_ts = query.before.map(super::parse_unix_ts);

    let pool = state.engine.pool();
    let pending = crate::core::changes::list_pending(pool)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    serde_json::json!({ "error": format!("Failed to list pending changes: {e}") }),
                ),
            )
        })?;
    // Fetch limit+1 to detect has_more
    let mut applied = crate::core::changes::list_recently_applied(pool, limit + 1, before_ts)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    serde_json::json!({ "error": format!("Failed to list applied changes: {e}") }),
                ),
            )
        })?;
    let has_more_applied = applied.len() as i64 > limit;
    if has_more_applied {
        applied.truncate(limit as usize);
    }
    let client_update = crate::core::changes::client_update_since(pool, state.started_at).await;
    let restart_groups = crate::core::changes::restart_groups_since(pool, state.started_at)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to load restart groups: {e}") })),
            )
        })?;
    Ok(Json(serde_json::json!({
        "pending": pending,
        "applied": applied,
        "total_pending": pending.len(),
        "restart_required": !restart_groups.is_empty(),
        "restart_groups": restart_groups,
        "client_update_available": client_update,
        "has_more_applied": has_more_applied,
    })))
}

/// GET /api/changes/applied — list recently applied changes with pagination
pub(super) async fn list_applied_changes(
    State(state): State<AppState>,
    Query(query): Query<ChangesListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let limit = query.limit.unwrap_or(15).clamp(1, 100);
    let before_ts = query.before.map(super::parse_unix_ts);

    let mut applied = crate::core::changes::list_recently_applied(
        state.engine.pool(),
        limit + 1,
        before_ts,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to list applied changes: {e}") })),
        )
    })?;
    let has_more = applied.len() as i64 > limit;
    if has_more {
        applied.truncate(limit as usize);
    }
    Ok(Json(
        serde_json::json!({ "applied": applied, "has_more": has_more }),
    ))
}

/// POST /api/changes/:id/revert — revert an applied change
pub(super) async fn revert_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let actor = resolve_change_actor(&state, &headers).await;
    match state.engine.revert_change(id, actor).await {
        Ok(message) => {
            broadcast_changes(&state).await;
            Ok(Json(serde_json::json!({ "message": message })))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )),
    }
}

/// POST /api/changes/:id/apply — apply a single change
pub(super) async fn apply_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ApplyResult>, (StatusCode, Json<serde_json::Value>)> {
    let actor = resolve_change_actor(&state, &headers).await;
    match state.engine.apply_change(id, actor).await {
        Ok(result) => {
            broadcast_changes(&state).await;
            Ok(Json(result))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )),
    }
}

/// POST /api/changes/:id/discard — discard a single change
pub(super) async fn discard_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let actor = resolve_change_actor(&state, &headers).await;
    match state.engine.discard_change(id, actor).await {
        Ok(()) => {
            broadcast_changes(&state).await;
            Ok(Json(serde_json::json!({ "message": "Change discarded." })))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )),
    }
}

/// POST /api/changes/apply-all — apply all pending changes
pub(super) async fn apply_all_changes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let actor = resolve_change_actor(&state, &headers).await;
    let pending = crate::core::changes::list_pending(state.engine.pool())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
        })?;
    if pending.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "No pending changes" })),
        ));
    }
    let mut applied = 0;
    let mut failed = 0;
    let mut errors = Vec::new();
    for change in &pending {
        match state.engine.apply_change(change.id, actor.clone()).await {
            Ok(result) => {
                if result.conflict_thread_id.is_some() {
                    // Stop at first conflict — frontend will start CC
                    broadcast_changes(&state).await;
                    let mut resp = serde_json::to_value(&result)
                        .expect("ApplyResult contains only Serialize-safe primitives");
                    resp["message"] = serde_json::Value::String(format!(
                        "Applied {} change(s), then hit a conflict.",
                        applied
                    ));
                    resp["applied"] = serde_json::Value::Number(applied.into());
                    resp["failed"] = serde_json::Value::Number(failed.into());
                    return Ok(Json(resp));
                }
                applied += 1;
            }
            Err(e) => {
                failed += 1;
                errors.push(format!("{}: {}", change.branch_name, e));
            }
        }
    }
    broadcast_changes(&state).await;
    let restart =
        crate::core::changes::requires_restart_since(state.engine.pool(), state.started_at).await;
    let mut msg = format!("{} applied.", applied);
    if failed > 0 {
        msg.push_str(&format!(" {} failed: {}", failed, errors.join("; ")));
    }
    if restart {
        msg.push_str(" This change requires the engine to restart.");
    }
    Ok(Json(
        serde_json::json!({ "message": msg, "restart_required": restart }),
    ))
}

/// GET /api/changes/:id — get a single change by ID
pub(super) async fn get_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::core::changes::Change>, (StatusCode, Json<serde_json::Value>)> {
    let change = crate::core::changes::get_by_id(state.engine.pool(), id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("DB error: {e}") })),
            )
        })?
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Change not found" })),
        ))?;
    Ok(Json(change))
}

/// GET /api/changes/for-repo/:repo_id — list changes for a specific repo
pub(super) async fn list_changes_for_repo(
    State(state): State<AppState>,
    Path(repo_id): Path<Uuid>,
    Query(query): Query<ChangesListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let repo = crate::core::repositories::RepositoryStore::get(&state.pool, repo_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?
        .ok_or((StatusCode::NOT_FOUND, "Repository not found".into()))?;

    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let before_ts = query.before.map(super::parse_unix_ts);

    let (pending, applied, has_more) =
        crate::core::changes::list_for_repo(&state.pool, &repo.path, limit, before_ts)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;

    Ok(Json(serde_json::json!({
        "pending": pending,
        "applied": applied,
        "has_more": has_more,
    })))
}

/// POST /api/changes/discard-all — discard all pending changes
pub(super) async fn discard_all_changes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let actor = resolve_change_actor(&state, &headers).await;
    let pending = crate::core::changes::list_pending(state.engine.pool())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
        })?;
    if pending.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "No pending changes" })),
        ));
    }
    let mut discarded = 0;
    let mut failed = 0;
    let mut errors = Vec::new();
    for change in &pending {
        match state.engine.discard_change(change.id, actor.clone()).await {
            Ok(()) => discarded += 1,
            Err(e) => {
                log!("[Changes] Failed to discard change {}: {}", change.id, e);
                failed += 1;
                errors.push(format!("{}: {}", change.branch_name, e));
            }
        }
    }
    broadcast_changes(&state).await;
    let message = if failed == 0 {
        format!("{} change(s) discarded.", discarded)
    } else {
        format!("{} change(s) discarded; {} failed.", discarded, failed)
    };
    Ok(Json(serde_json::json!({
        "message": message,
        "discarded": discarded,
        "failed": failed,
        "errors": errors,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the wire format so wire-breaking changes (renamed fields,
    /// changed status casing, removed optionality) trip a test instead of
    /// silently shipping. Per-variant correctness is covered by the
    /// `ApplyResult` constructors in `engine::types`.
    #[test]
    fn applied_with_merge_serializes_to_expected_shape() {
        let change_id = Uuid::nil();
        let thread_id = Uuid::nil();
        let result = ApplyResult::applied_with_merge(
            change_id,
            Some(thread_id),
            false,
            "b".repeat(40),
            "a".repeat(40),
            &["fix: a".to_string(), "fix: b".to_string()],
            3,
        );
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["status"], "applied");
        assert_eq!(json["change_id"], change_id.to_string());
        assert_eq!(json["thread_id"], thread_id.to_string());
        assert_eq!(json["applied_commit"], "a".repeat(40));
        assert_eq!(json["previous_commit"], "b".repeat(40));
        assert_eq!(json["commits_applied"], 2);
        assert_eq!(json["files_changed"], 3);
        assert_eq!(json["restart_required"], false);
        assert!(
            json.get("conflict_thread_id").is_none(),
            "absent Option must not serialize"
        );
        assert!(json.get("review_thread_id").is_none());
    }

    #[test]
    fn noop_omits_sha_fields() {
        let json =
            serde_json::to_value(ApplyResult::noop(Uuid::nil(), None, 0, "nothing to merge"))
                .unwrap();
        assert_eq!(json["status"], "noop");
        assert_eq!(json["thread_id"], serde_json::Value::Null);
        assert!(
            json.get("applied_commit").is_none(),
            "noop must not pretend to have a SHA"
        );
        assert!(json.get("previous_commit").is_none());
    }
}
