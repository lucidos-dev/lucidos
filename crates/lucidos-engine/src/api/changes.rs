use super::actor::user_actor_resolved;
use super::*;
use crate::engine::apply_all_driver::ApplyAllDriveMsg;
use crate::engine::{ApplyResult, ApplyStatus};
use axum::http::HeaderMap;

pub(super) async fn broadcast_changes(state: &AppState) {
    let proj = state.engine.changes();
    let (pending_r, applied_r, restart_r) = tokio::join!(
        proj.list_pending(),
        proj.list_recently_applied(15, None),
        proj.restart_groups_since(state.started_at),
    );
    let mut pending = match pending_r {
        Ok(v) => v,
        Err(e) => {
            crate::log!("[Changes] broadcast: list_pending: {} — skipping broadcast", e);
            return;
        }
    };
    let mut applied = match applied_r {
        Ok(v) => v,
        Err(e) => {
            crate::log!("[Changes] broadcast: list_recently_applied: {} — skipping broadcast", e);
            return;
        }
    };
    let restart_groups = match restart_r {
        Ok(v) => v,
        Err(e) => {
            crate::log!("[Changes] broadcast: restart_groups_since: {} — skipping broadcast", e);
            return;
        }
    };

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

/// GET /api/v1/changes — list pending + applied changes with pagination for applied
pub(super) async fn list_changes(
    State(state): State<AppState>,
    Query(query): Query<ChangesListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let limit = query.limit.unwrap_or(15).clamp(1, 100);
    let before_ts = query.before.map(super::parse_unix_ts);

    let pool = state.engine.pool();
    let proj = state.engine.changes();
    // Fetch limit+1 on applied to detect has_more.
    let (pending_r, applied_r, client_update_r, restart_groups_r) = tokio::join!(
        proj.list_pending(),
        proj.list_recently_applied(limit + 1, before_ts),
        proj.client_update_since(state.started_at),
        proj.restart_groups_since(state.started_at),
    );
    let mut pending = pending_r.map_err(internal_err)?;
    let mut applied = applied_r.map_err(internal_err)?;
    let client_update = client_update_r.map_err(internal_err)?;
    let mut restart_groups = restart_groups_r.map_err(internal_err)?;
    let has_more_applied = applied.len() as i64 > limit;
    if has_more_applied {
        applied.truncate(limit as usize);
    }

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

/// GET /api/v1/changes/applied — list recently applied changes with pagination
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
        .await
        .map_err(internal_err)?;
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

/// POST /api/v1/changes/:id/revert — revert an applied change
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

/// Reject a single-change action when the availability selector doesn't grant
/// it for the change's thread (server-side mirror of the UI gate). Unknown
/// change ids fall through so the engine returns its own "Change not found".
/// Internal apply paths (Apply All driver, conflict recovery) call
/// `engine.apply_change` directly and are intentionally NOT gated here.
///
/// The thread-state gate only applies while the change is still `pending`.
/// A change that's already in a terminal state (`applied` / `discarded`)
/// falls through to the engine, which is idempotent: a re-apply returns
/// `Noop` (200) echoing the original merge SHAs, a re-discard returns success.
/// Gating those would convert an idempotent retry into a spurious 409 — the
/// thread's `coding_agent_proposed` flag is cleared by `ChangeApplied` /
/// `ChangeDiscarded`, so `available_thread_actions_for` no longer offers
/// Apply/Discard once the change has resolved.
async fn guard_change_action(
    state: &AppState,
    change_id: Uuid,
    action: crate::engine::thread_lifecycle::Action,
    reject_msg: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let row: Option<(Option<Uuid>, String)> =
        sqlx::query_as("SELECT thread_id, status FROM changes WHERE id = $1")
            .bind(change_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(internal_err)?;
    // Unknown change id, change with no thread, or an already-resolved change:
    // defer to the engine (it returns "Change not found" or handles the
    // idempotent terminal-state retry).
    let Some((Some(thread_id), status)) = row else {
        return Ok(());
    };
    if status != "pending" {
        return Ok(());
    }
    let actions = crate::api::threads::available_thread_actions_for(&state.pool, thread_id)
        .await
        .map_err(internal_err)?;
    if !actions.contains(&action) {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": reject_msg })),
        ));
    }
    Ok(())
}

/// POST /api/v1/changes/:id/apply — apply a single change
pub(super) async fn apply_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ApplyResult>, (StatusCode, Json<serde_json::Value>)> {
    let actor = user_actor_resolved(&headers, &state.pool, None).await;
    guard_change_action(
        &state,
        id,
        crate::engine::thread_lifecycle::Action::Apply,
        "This change can't be applied in the thread's current state",
    )
    .await?;
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

/// POST /api/v1/changes/:id/discard — discard a single change
pub(super) async fn discard_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let actor = user_actor_resolved(&headers, &state.pool, None).await;
    guard_change_action(
        &state,
        id,
        crate::engine::thread_lifecycle::Action::Discard,
        "This change can't be discarded in the thread's current state",
    )
    .await?;
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

/// POST /api/v1/changes/apply-all — apply all pending changes
///
/// Emits durable `ApplyAllBatchStarted` with every pending change ID, applies
/// the first change synchronously so the HTTP caller gets an immediate
/// result, then hands off to the driver task. Subsequent `ChangeApplied` /
/// `ChangeApplyFailed` events feed the driver, which advances the batch and
/// fires the next apply — including across the conflict-recovery suspension
/// window — until every member resolves and `ApplyAllBatchCompleted` lands.
pub(super) async fn apply_all_changes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let actor = user_actor_resolved(&headers, &state.pool, None).await;
    let pending = state
        .engine
        .changes()
        .list_pending()
        .await
        .map_err(internal_err)?;
    if pending.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "No pending changes" })),
        ));
    }
    let change_ids: Vec<Uuid> = pending.iter().map(|c| c.id).collect();
    let batch_id = state
        .engine
        .start_apply_all_batch(change_ids.clone(), actor.clone())
        .await;
    // Apply the first change synchronously so the HTTP response carries
    // an immediate result. The driver task takes over from here —
    // subsequent ChangeApplied/Failed events feed it via the channel
    // and it spawns the next apply.
    let first = &pending[0];
    let first_result = state.engine.apply_change(first.id, actor.clone()).await;
    // Some apply_change outcomes don't emit a per-change terminator that
    // would notify the driver: `Noop` (already applied), `Conflict` (CC
    // handles it later via emit_change_applied/_failed), and several
    // early-Err paths (change-not-found, status-mismatch). Notify
    // explicitly for the cases that would otherwise leave the batch
    // stalled on the first change forever. The driver's record_*
    // methods are first-write-wins so over-notification is safe.
    match &first_result {
        Ok(r) if matches!(r.status, ApplyStatus::Noop) => {
            state
                .engine
                .notify_apply_all(ApplyAllDriveMsg::Applied(first.id));
        }
        Err(e) => {
            state.engine.notify_apply_all(ApplyAllDriveMsg::Failed(
                first.id,
                e.to_string(),
            ));
        }
        _ => {}
    }
    broadcast_changes(&state).await;
    let remaining = change_ids.len().saturating_sub(1);
    match first_result {
        Ok(result) => {
            let mut resp = serde_json::to_value(&result)
                .expect("ApplyResult contains only Serialize-safe primitives");
            resp["batch_id"] = serde_json::Value::String(batch_id.to_string());
            resp["batch_size"] = serde_json::Value::Number(change_ids.len().into());
            resp["message"] = serde_json::Value::String(match result.status {
                ApplyStatus::Conflict => format!(
                    "Started Apply All — first change hit a conflict, recovery is running. \
                     The remaining {remaining} change(s) will apply automatically once the conflict resolves."
                ),
                ApplyStatus::Hardening => format!(
                    "Started Apply All — hardening the first change. \
                     The remaining {remaining} will apply automatically after that."
                ),
                ApplyStatus::Applied | ApplyStatus::Noop => format!(
                    "Started Apply All — first change applied. {remaining} more queued."
                ),
            });
            Ok(Json(resp))
        }
        Err(e) => Ok(Json(serde_json::json!({
            "batch_id": batch_id.to_string(),
            "batch_size": change_ids.len(),
            "applied": 0,
            "failed": 1,
            "error": format!("{}: {}", first.branch_name, e),
            "message": format!(
                "Started Apply All — first change errored ({}), continuing with the remaining {}.",
                e,
                remaining,
            ),
        }))),
    }
}

/// GET /api/v1/changes/:id — get a single change by ID
pub(super) async fn get_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::core::changes::Change>, (StatusCode, Json<serde_json::Value>)> {
    let mut change = state
        .engine
        .changes()
        .get_by_id(id)
        .await
        .map_err(internal_err)?
        .ok_or((
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

/// GET /api/v1/changes/for-repo/:repo_id — list changes for a specific repo
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

    let to_err = |e: sqlx::Error| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}"));
    let (mut pending, mut applied, has_more) = state
        .engine
        .changes()
        .list_for_repo(&repo.path, limit, before_ts)
        .await
        .map_err(to_err)?;
    let (r1, r2) = tokio::join!(
        crate::core::changes::enrich_thread_titles(&state.pool, &mut pending),
        crate::core::changes::enrich_thread_titles(&state.pool, &mut applied),
    );
    r1.map_err(to_err)?;
    r2.map_err(to_err)?;

    Ok(Json(serde_json::json!({
        "pending": pending,
        "applied": applied,
        "has_more": has_more,
    })))
}

/// POST /api/v1/changes/discard-all — discard all pending changes
pub(super) async fn discard_all_changes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let actor = user_actor_resolved(&headers, &state.pool, None).await;
    let pending = state
        .engine
        .changes()
        .list_pending()
        .await
        .map_err(internal_err)?;
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
