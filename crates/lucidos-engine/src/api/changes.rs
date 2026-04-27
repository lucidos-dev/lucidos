use super::actor::user_actor_resolved;
use super::*;
use crate::engine::ApplyResult;
use axum::http::HeaderMap;

pub(super) async fn broadcast_changes(state: &AppState) {
    let proj = state.engine.changes();
    let mut pending = proj.list_pending().await;
    let mut applied = proj.list_recently_applied(15, None).await;
    let restart_groups = proj.restart_groups_since(state.started_at).await;

    let pool = state.engine.pool();
    let (r1, r2) = tokio::join!(
        crate::core::changes::enrich_thread_titles(pool, &mut pending),
        crate::core::changes::enrich_thread_titles(pool, &mut applied),
    );
    if let Err(e) = r1 {
        crate::log!("[Changes] broadcast: enrich pending titles: {}", e);
    }
    if let Err(e) = r2 {
        crate::log!("[Changes] broadcast: enrich applied titles: {}", e);
    }

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
    let proj = state.engine.changes();
    let mut pending = proj.list_pending().await;
    // Fetch limit+1 to detect has_more
    let mut applied = proj.list_recently_applied(limit + 1, before_ts).await;
    let has_more_applied = applied.len() as i64 > limit;
    if has_more_applied {
        applied.truncate(limit as usize);
    }
    let client_update = proj.client_update_since(state.started_at).await;
    let mut restart_groups = proj.restart_groups_since(state.started_at).await;

    let (r1, r2, r3) = tokio::join!(
        crate::core::changes::enrich_thread_titles(pool, &mut pending),
        crate::core::changes::enrich_thread_titles(pool, &mut applied),
        crate::core::changes::enrich_restart_group_titles(pool, &mut restart_groups),
    );
    r1.map_err(internal_err)?;
    r2.map_err(internal_err)?;
    r3.map_err(internal_err)?;

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

fn internal_err(e: sqlx::Error) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": format!("DB error: {e}") })),
    )
}

/// GET /api/changes/applied — list recently applied changes with pagination
pub(super) async fn list_applied_changes(
    State(state): State<AppState>,
    Query(query): Query<ChangesListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let limit = query.limit.unwrap_or(15).clamp(1, 100);
    let before_ts = query.before.map(super::parse_unix_ts);

    let mut applied = state
        .engine
        .changes()
        .list_recently_applied(limit + 1, before_ts)
        .await;
    let has_more = applied.len() as i64 > limit;
    if has_more {
        applied.truncate(limit as usize);
    }
    crate::core::changes::enrich_thread_titles(state.engine.pool(), &mut applied)
        .await
        .map_err(internal_err)?;
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
    let actor = user_actor_resolved(&headers, &state.pool, None).await;
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
    let actor = user_actor_resolved(&headers, &state.pool, None).await;
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
    let actor = user_actor_resolved(&headers, &state.pool, None).await;
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
    let actor = user_actor_resolved(&headers, &state.pool, None).await;
    let pending = state.engine.changes().list_pending().await;
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
    let restart = state
        .engine
        .changes()
        .requires_restart_since(state.started_at)
        .await;
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
    let mut change = state.engine.changes().get_by_id(id).await.ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "Change not found" })),
    ))?;
    crate::core::changes::enrich_thread_titles(
        state.engine.pool(),
        std::slice::from_mut(&mut change),
    )
    .await
    .map_err(internal_err)?;
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

    let (mut pending, mut applied, has_more) = state
        .engine
        .changes()
        .list_for_repo(&repo.path, limit, before_ts)
        .await;
    let (r1, r2) = tokio::join!(
        crate::core::changes::enrich_thread_titles(&state.pool, &mut pending),
        crate::core::changes::enrich_thread_titles(&state.pool, &mut applied),
    );
    let to_err = |e: sqlx::Error| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}"));
    r1.map_err(to_err)?;
    r2.map_err(to_err)?;

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
    let actor = user_actor_resolved(&headers, &state.pool, None).await;
    let pending = state.engine.changes().list_pending().await;
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
