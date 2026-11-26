use super::actor::require_user_actor;
use super::thread_reach::refuse_without_authority;
use super::*;
use crate::core::changes::{ChangeStatus, PendingScope};
use crate::engine::apply_all_driver::ApplyAllOutcome;
use crate::engine::permission::Action;
use crate::engine::standing_apply::{DisarmScope, StandingApply, DISARMED_BY_OWNER};
use crate::engine::LaneRemoval;
use crate::engine::{ApplyResult, ApplyStatus};
use axum::http::HeaderMap;

async fn refuse_change_verb(
    state: &AppState,
    headers: &HeaderMap,
    change_id: Uuid,
    verb: Action,
) -> Result<(), ApiError> {
    let row: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT thread_id FROM changes WHERE id = $1")
            .bind(change_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::db)?;
    let Some(thread_id) = row else {
        return Ok(());
    };
    Ok(refuse_without_authority(&state.engine.event_bus, headers, thread_id, verb).await?)
}

async fn refuse_batch_change_verb(
    state: &AppState,
    headers: &HeaderMap,
    batch: &[crate::core::changes::Change],
    verb: Action,
) -> Result<(), ApiError> {
    for change in batch {
        refuse_without_authority(&state.engine.event_bus, headers, change.thread_id, verb).await?;
    }
    Ok(())
}

pub(super) async fn list_changes(
    State(state): State<AppState>,
    Query(query): Query<ChangesListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    read_changes(&state, &query).await.map(Json)
}

pub(super) const CHANGES_DEFAULT_LIMIT: i64 = 15;

pub(super) async fn read_changes(
    state: &AppState,
    query: &ChangesListQuery,
) -> Result<serde_json::Value, ApiError> {
    let limit = query.limit.unwrap_or(CHANGES_DEFAULT_LIMIT).clamp(1, 100);
    let before_ts = query.before.map(super::parse_unix_ts);

    let pool = state.engine.pool();
    let proj = state.engine.changes();
    // INVARIANT: Fetch limit+1 on applied to detect has_more. The durable `apply_all_batches`
    // mirror holds a row only while a batch is in flight (inserted on start,
    // deleted on complete/cancel/recovery), so its non-emptiness is the
    // cross-reload truth for the "Applying changes…" toast — the driving
    // `applyAllInProgress` signal resets on reload and the ApplyAllBatch* SSE
    // events aren't replayed. Joined with the other reads — independent query.
    let (
        pending_r,
        set_aside_r,
        applied_r,
        client_update_r,
        restart_groups_r,
        apply_all_r,
        apply_estimates_r,
    ) = tokio::join!(
        crate::core::changes::list_pending_for_readers(
            pool,
            proj,
            query
                .sub_threads_of
                .map_or(PendingScope::All, PendingScope::SubThreadsOf),
        ),
        crate::core::changes::list_set_aside_for_readers(
            pool,
            proj,
            query
                .sub_threads_of
                .map_or(PendingScope::All, PendingScope::SubThreadsOf),
        ),
        proj.list_recently_applied(limit + 1, before_ts),
        proj.client_update_since(state.started_at),
        proj.restart_groups_since(state.started_at),
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM apply_all_batches)")
            .fetch_one(pool),
        state.engine.apply_estimates.current(pool),
    );
    let pending = pending_r.map_err(ApiError::db)?;
    let set_aside = set_aside_r.map_err(ApiError::db)?;
    let mut applied = applied_r.map_err(ApiError::db)?;
    let client_update = client_update_r.map_err(ApiError::db)?;
    let mut restart_groups = restart_groups_r.map_err(ApiError::db)?;
    let apply_all_in_progress = apply_all_r.map_err(ApiError::db)?;
    let apply_estimates = apply_estimates_r.map_err(ApiError::db)?;
    let has_more_applied = applied.len() as i64 > limit;
    if has_more_applied {
        applied.truncate(limit as usize);
    }

    let (r1, r2) = tokio::join!(
        crate::core::changes::enrich_thread_titles(pool, &mut applied),
        crate::core::changes::enrich_restart_group_titles(pool, &mut restart_groups),
    );
    r1.map_err(ApiError::db)?;
    r2.map_err(ApiError::db)?;

    Ok(serde_json::json!({
        "pending": pending,
        "set_aside": set_aside,
        "applied": applied,
        "total_pending": pending.len(),
        "restart_required": !restart_groups.is_empty(),
        "restart_groups": restart_groups,
        "client_update_available": client_update,
        "has_more_applied": has_more_applied,
        "apply_all_in_progress": apply_all_in_progress,
        "apply_all_batch": state.engine.apply_all_batches.lock().await.snapshot(),
        "landing_lane": state.engine.landing_lane_snapshot(),
        "main_check": state.engine.main_check_snapshot(),
        "standing_apply_thread_ids": state.engine.armed_standing_apply_threads(),
        "apply_estimates": apply_estimates,
    }))
}

pub(super) async fn list_applied_changes(
    State(state): State<AppState>,
    Query(query): Query<ChangesListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let limit = query.limit.unwrap_or(15).clamp(1, 100);
    let before_ts = query.before.map(super::parse_unix_ts);

    let mut applied = state
        .engine
        .changes()
        .list_recently_applied(limit + 1, before_ts)
        .await
        .map_err(ApiError::db)?;
    let has_more = applied.len() as i64 > limit;
    if has_more {
        applied.truncate(limit as usize);
    }
    crate::core::changes::enrich_thread_titles(state.engine.pool(), &mut applied)
        .await
        .map_err(ApiError::db)?;
    Ok(Json(
        serde_json::json!({ "applied": applied, "has_more": has_more }),
    ))
}

pub(super) async fn revert_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    // INVARIANT: Reverting an applied change is at least as destructive as discarding a
    // pending one, so it takes the same subtree-reach gate its siblings do. The
    // change keeps its `thread_id` after applying, so `refuse_change_verb` scopes
    // revert to the change's own thread exactly like discard.
    refuse_change_verb(&state, &headers, id, Action::Revert).await?;
    match state.engine.revert_change(id, actor).await {
        Ok(message) => {
            state.engine.broadcast_changes_updated().await;
            Ok(Json(serde_json::json!({ "message": message })))
        }
        Err(e) => Err(ApiError::bad_request(e.to_string())),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeActionRefusal {
    NoFilesLeft,
    ThreadSettling,
    ThreadParked,
    ActionUnavailable,
    ChangeSetAside,
}

pub(crate) async fn change_action_refusal(
    pool: &sqlx::PgPool,
    change_id: Uuid,
    action: crate::engine::thread_lifecycle::ThreadAction,
) -> Result<Option<ChangeActionRefusal>, sqlx::Error> {
    let row: Option<(Option<Uuid>, ChangeStatus, i32)> =
        sqlx::query_as("SELECT thread_id, status, file_count FROM changes WHERE id = $1")
            .bind(change_id)
            .fetch_optional(pool)
            .await?;
    if let Some((_, status, file_count)) = row.as_ref() {
        let apply = action == crate::engine::thread_lifecycle::ThreadAction::Apply;
        if *status == ChangeStatus::Pending && *file_count == 0 && apply {
            return Ok(Some(ChangeActionRefusal::NoFilesLeft));
        }
        if *status == ChangeStatus::SetAside && apply {
            return Ok(Some(ChangeActionRefusal::ChangeSetAside));
        }
    }
    let Some((Some(thread_id), status, _)) = row else {
        return Ok(None);
    };
    let set_aside_discard = status == ChangeStatus::SetAside
        && action == crate::engine::thread_lifecycle::ThreadAction::Discard;
    if status != ChangeStatus::Pending && !set_aside_discard {
        return Ok(None);
    }
    let unsettled = crate::core::changes::unsettled_thread_ids(pool, std::iter::once(thread_id))
        .await?
        .contains(&thread_id);
    if !unsettled {
        if set_aside_discard {
            return Ok(None);
        }
        let actions = crate::api::threads::available_thread_actions_for(pool, thread_id).await?;
        if actions.contains(&action) {
            return Ok(None);
        }
    }
    if crate::engine::question_park_sql::thread_parked_on_question(pool, thread_id).await? {
        return Ok(Some(ChangeActionRefusal::ThreadParked));
    }
    if crate::engine::standing_apply::settling_thread_ids(pool, std::iter::once(thread_id))
        .await?
        .contains(&thread_id)
    {
        return Ok(Some(ChangeActionRefusal::ThreadSettling));
    }
    if unsettled {
        return Ok(Some(ChangeActionRefusal::ThreadParked));
    }
    Ok(Some(ChangeActionRefusal::ActionUnavailable))
}

async fn guard_change_action(
    state: &AppState,
    change_id: Uuid,
    action: crate::engine::thread_lifecycle::ThreadAction,
    reject_msg: &str,
) -> Result<(), ApiError> {
    let refusal = change_action_refusal(&state.pool, change_id, action)
        .await
        .map_err(ApiError::db)?;
    let msg = match refusal {
        None => return Ok(()),
        Some(ChangeActionRefusal::NoFilesLeft) => {
            "This change has no file changes left. Discard it instead."
        }
        Some(ChangeActionRefusal::ChangeSetAside) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                crate::engine::SET_ASIDE_APPLY_REFUSAL,
            )
            .with_reason("change_set_aside"));
        }
        Some(_) => reject_msg,
    };
    Err(ApiError::new(StatusCode::CONFLICT, msg))
}

pub(super) async fn apply_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ApplyResult>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    refuse_change_verb(&state, &headers, id, Action::Apply).await?;
    guard_change_action(
        &state,
        id,
        crate::engine::thread_lifecycle::ThreadAction::Apply,
        "This change can't be applied in the thread's current state",
    )
    .await?;
    match state.engine.apply_change(id, actor).await {
        Ok(result) => {
            state.engine.broadcast_changes_updated().await;
            Ok(Json(result))
        }
        Err(e) => Err(ApiError::bad_request(e.to_string())),
    }
}

pub(super) async fn discard_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    refuse_change_verb(&state, &headers, id, Action::Discard).await?;
    guard_change_action(
        &state,
        id,
        crate::engine::thread_lifecycle::ThreadAction::Discard,
        "This change can't be discarded in the thread's current state",
    )
    .await?;
    match state.engine.discard_change(id, actor).await {
        Ok(()) => {
            state.engine.broadcast_changes_updated().await;
            Ok(Json(serde_json::json!({ "message": "Change discarded." })))
        }
        Err(e) => Err(ApiError::bad_request(e.to_string())),
    }
}

pub(super) async fn set_aside_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    refuse_change_verb(&state, &headers, id, Action::SetAside).await?;
    guard_change_action(
        &state,
        id,
        crate::engine::thread_lifecycle::ThreadAction::Hold,
        "This change can't be set aside in the thread's current state",
    )
    .await?;
    match state.engine.set_aside_change(id, None, actor).await {
        Ok(()) => Ok(Json(serde_json::json!({ "message": "Change set aside." }))),
        Err(e) => Err(ApiError::bad_request(e.to_string())),
    }
}

pub(super) async fn bring_back_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    refuse_change_verb(&state, &headers, id, Action::BringBack).await?;
    match state.engine.bring_back_change(id, actor).await {
        Ok(()) => Ok(Json(
            serde_json::json!({ "message": "Change brought back." }),
        )),
        Err(e) => Err(ApiError::bad_request(e.to_string())),
    }
}

#[derive(serde::Deserialize)]
pub(super) struct ArmStandingApplyBody {
    thread_id: Uuid,
    #[serde(default)]
    change_id: Option<Uuid>,
}

pub(super) async fn arm_standing_apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArmStandingApplyBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    // INVARIANT: Arming an apply on a thread IS applying it, one settle later, so it takes
    // the gate the immediate apply takes. The `apply_when_settled` LLM tool is
    // the other way to this same act, and asks the same rule.
    refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(body.thread_id),
        Action::Apply,
    )
    .await?;
    // INVARIANT: A change named here must be this thread's own pending one. Binding an
    // arm to somebody else's change would apply work the owner never saw on
    // this thread's settle.
    if let Some(change_id) = body.change_id {
        let row: Option<(Option<Uuid>, ChangeStatus)> =
            sqlx::query_as("SELECT thread_id, status FROM changes WHERE id = $1")
                .bind(change_id)
                .fetch_optional(&state.pool)
                .await
                .map_err(ApiError::db)?;
        match row {
            None => return Err(ApiError::not_found("Change not found")),
            Some((thread_id, _)) if thread_id != Some(body.thread_id) => {
                return Err(ApiError::bad_request(
                    "That change belongs to a different thread",
                ))
            }
            Some((_, status)) if status != ChangeStatus::Pending => {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "That change has already been applied or discarded",
                ))
            }
            Some(_) => {}
        }
    }
    state
        .engine
        .arm_standing_apply(StandingApply {
            thread_id: body.thread_id,
            change_id: body.change_id,
            batch_id: None,
            actor,
        })
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    state.engine.broadcast_changes_updated().await;
    Ok(Json(serde_json::json!({
        "message": "Will apply when the thread settles.",
    })))
}

pub(super) async fn disarm_standing_apply(
    State(state): State<AppState>,
    Path(thread_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(thread_id),
        Action::Apply,
    )
    .await?;
    // INVARIANT: A 404 must mean "nothing was armed": the client reads it as already off.
    // A failed delete leaves the arm live, so it answers 500 instead.
    let dropped = state
        .engine
        .drop_standing_apply(thread_id, DISARMED_BY_OWNER, actor)
        .await
        .map_err(ApiError::db)?;
    if !dropped {
        return Err(ApiError::not_found("No standing apply on that thread"));
    }
    state.engine.broadcast_changes_updated().await;
    Ok(Json(
        serde_json::json!({ "message": "Standing apply canceled." }),
    ))
}

pub(super) async fn disarm_all_standing_applies(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    refuse_without_authority(&state.engine.event_bus, &headers, None, Action::Apply).await?;
    let disarmed = state
        .engine
        .drop_standing_applies(DisarmScope::All, DISARMED_BY_OWNER, actor)
        .await
        .map_err(ApiError::db)?;
    state.engine.broadcast_changes_updated().await;
    Ok(Json(serde_json::json!({ "disarmed": disarmed })))
}

#[derive(serde::Deserialize, Default)]
pub(super) struct ApplyAllQuery {
    #[serde(default)]
    keep_going: bool,
}

pub(super) fn empty_apply_all_refusal(
    total_pending: usize,
    not_ready: usize,
    already_applying: usize,
) -> &'static str {
    if total_pending == 0 {
        "No pending changes"
    } else if not_ready == total_pending {
        "No pending change is ready: each thread is still working or waiting for something, \
         or its work is not hardened. Use \"Apply all on settle\", or wait for them to finish."
    } else if already_applying > 0 {
        "Every ready change is already applying or has no file changes left. \
         Each outcome arrives when its apply finishes."
    } else {
        "None of the ready changes can be applied: each has no file changes left."
    }
}

pub(super) fn apply_all_on_settle_message(armed: usize) -> String {
    match armed {
        0 => "No thread is still settling, so there is nothing to apply as it settles.".into(),
        1 => "Will apply 1 thread's change as it settles.".into(),
        n => format!("Will apply {n} threads' changes as they settle."),
    }
}

pub(super) async fn apply_all_changes(
    State(state): State<AppState>,
    Query(query): Query<ApplyAllQuery>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    // INVARIANT: Apply All aims at the workspace rather than at one thread, and as the
    // sweep it arms threads that have proposed nothing yet. No subtree
    // contains that, so clause 3 cannot cover it and a thread caller needs the
    // owner's standing instruction. Before the batch record and the first
    // merge, so a refusal leaves neither.
    refuse_without_authority(&state.engine.event_bus, &headers, None, Action::Apply).await?;
    let outcome = state
        .engine
        .run_apply_all(actor, query.keep_going)
        .await
        .map_err(ApiError::db)?;

    let (total_pending, not_ready, already_applying, armed) = match &outcome {
        ApplyAllOutcome::NothingToApply {
            total_pending,
            not_ready,
            already_applying,
            armed,
        } => (*total_pending, *not_ready, *already_applying, *armed),
        ApplyAllOutcome::Started { .. } => (0, 0, 0, 0),
    };
    if let ApplyAllOutcome::NothingToApply { .. } = outcome {
        if !query.keep_going {
            return Err(ApiError::bad_request(empty_apply_all_refusal(
                total_pending,
                not_ready,
                already_applying,
            )));
        }
        state.engine.broadcast_changes_updated().await;
        return Ok(Json(serde_json::json!({
            "batch_size": 0,
            "armed": armed,
            "message": apply_all_on_settle_message(armed),
        })));
    }
    let ApplyAllOutcome::Started {
        batch_id,
        batch_size,
        armed,
        first_branch,
        first_result,
    } = outcome
    else {
        unreachable!("the NothingToApply arm returned above")
    };

    state.engine.broadcast_changes_updated().await;
    let remaining = batch_size.saturating_sub(1);
    match first_result {
        Ok(result) => {
            let mut resp = serde_json::to_value(&result)
                .expect("ApplyResult contains only Serialize-safe primitives");
            resp["batch_id"] = serde_json::Value::String(batch_id.to_string());
            resp["batch_size"] = serde_json::Value::Number(batch_size.into());
            resp["armed"] = serde_json::Value::Number(armed.into());
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
                ApplyStatus::Queued => format!(
                    "Started Apply All. The first change waits in the landing lane, and the \
                     remaining {remaining} apply in turn."
                ),
            });
            Ok(Json(resp))
        }
        Err(e) => Ok(Json(serde_json::json!({
            "batch_id": batch_id.to_string(),
            "batch_size": batch_size,
            "armed": armed,
            "applied": 0,
            "failed": 1,
            "error": format!("{}: {}", first_branch, e),
            "message": format!(
                "Started Apply All — first change errored ({}), continuing with the remaining {}.",
                e,
                remaining,
            ),
        }))),
    }
}

pub(super) async fn cancel_apply_all_changes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    // INVARIANT: Cancel takes back the batch's Apply and the sweep's arms, so it asks the
    // gate Apply All asks, aimed at the workspace. Before the batch is touched,
    // so a refusal stops nothing and says nothing about whether one runs.
    refuse_without_authority(&state.engine.event_bus, &headers, None, Action::Apply).await?;
    let canceled = state.engine.cancel_apply_all_batches(actor.clone()).await;
    let disarmed = state
        .engine
        .drop_standing_applies(DisarmScope::Sweep, DISARMED_BY_OWNER, actor)
        .await
        .map_err(ApiError::db)?;
    if canceled == 0 && disarmed == 0 {
        return Err(ApiError::bad_request("No Apply All batch is running"));
    }
    state.engine.broadcast_changes_updated().await;
    Ok(Json(serde_json::json!({
        "canceled_batches": canceled,
        "disarmed": disarmed,
    })))
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct CancelLaneBatchBody {
    batch_id: Uuid,
}

pub(super) async fn cancel_lane_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CancelLaneBatchBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    // INVARIANT: Cancel takes back every member's Apply, so it asks the gate Apply asks,
    // member by member, before the batch fails. A batch id naming no open
    // batch falls through to the lane's own refusal.
    let members = state
        .engine
        .landing_lane_snapshot()
        .open_batch
        .filter(|batch| batch.batch_id == body.batch_id)
        .map(|batch| batch.members)
        .unwrap_or_default();
    let mut changes = Vec::with_capacity(members.len());
    for id in members {
        if let Some(change) = state
            .engine
            .changes()
            .get_by_id(id)
            .await
            .map_err(ApiError::db)?
        {
            changes.push(change);
        }
    }
    refuse_batch_change_verb(&state, &headers, &changes, Action::Apply).await?;
    state
        .engine
        .cancel_lane_batch(body.batch_id, actor)
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(serde_json::json!({ "canceled": body.batch_id })))
}

pub(super) async fn remove_from_landing_lane(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    // INVARIANT: Remove takes an Apply back, so it asks the gate Apply asks. It skips
    // the thread-state gate: the source thread stays free while its change
    // waits, and Remove touches no branch and no worktree.
    refuse_change_verb(&state, &headers, id, Action::Apply).await?;
    let known: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM changes WHERE id = $1)")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::db)?;
    if !known {
        return queue_removal_answer(None);
    }
    let removal = state
        .engine
        .remove_from_landing_lane(id, actor)
        .await
        .map_err(ApiError::internal)?;
    queue_removal_answer(Some(removal))
}

fn queue_removal_answer(
    removal: Option<Result<(), LaneRemoval>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match removal {
        None => Err(ApiError::not_found("Change not found")),
        Some(Ok(())) => Ok(Json(
            serde_json::json!({ "message": "Removed from the landing lane." }),
        )),
        Some(Err(LaneRemoval::HeldByLander)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "This change is held by the lander.",
        )
        .with_reason("held_by_lander")),
        Some(Err(LaneRemoval::Unavailable)) => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The landing lane could not read this change. Try again.",
        )),
        Some(Err(LaneRemoval::InBatch)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "This change is landing in a batch. Cancel the batch to take it out.",
        )
        .with_reason("in_lane_batch")),
        Some(Err(LaneRemoval::NotWaiting)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "This change is not waiting to land.",
        )
        .with_reason("not_waiting_to_land")),
    }
}

pub(super) async fn get_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::core::changes::Change>, ApiError> {
    let mut change = state
        .engine
        .changes()
        .get_by_id(id)
        .await
        .map_err(ApiError::db)?
        .ok_or_else(|| ApiError::not_found("Change not found"))?;
    crate::core::changes::enrich_thread_titles(
        state.engine.pool(),
        std::slice::from_mut(&mut change),
    )
    .await
    .map_err(ApiError::db)?;
    Ok(Json(change))
}

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

pub(super) async fn discard_all_changes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    let all_pending = state
        .engine
        .changes()
        .list_pending()
        .await
        .map_err(ApiError::db)?;
    if all_pending.is_empty() {
        return Err(ApiError::bad_request("No pending changes"));
    }
    // INVARIANT: Discard All sits on the Ready section, so it acts on the apply-ready
    // changes only. Discarding an unsettled one would delete the branch and
    // worktree out from under a session that is still going. Mirrors the
    // Apply All filter and `guard_change_action`.
    let pending = crate::core::changes::keep_apply_ready_changes(&state.pool, all_pending)
        .await
        .map_err(ApiError::db)?;
    if pending.is_empty() {
        return Err(ApiError::bad_request(
            "No pending change is ready: each thread is still working or waiting for something, or its work is not hardened.",
        ));
    }
    refuse_batch_change_verb(&state, &headers, &pending, Action::Discard).await?;
    let mut discarded = 0;
    let mut failed = 0;
    let mut errors = Vec::new();
    for change in &pending {
        match state.engine.discard_change(change.id, actor.clone()).await {
            Ok(()) => discarded += 1,
            Err(e) => {
                log_legacy!("[Changes] Failed to discard change {}: {}", change.id, e);
                failed += 1;
                errors.push(format!("{}: {}", change.branch_name, e));
            }
        }
    }
    state.engine.broadcast_changes_updated().await;
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

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/standing-applies",
            post(arm_standing_apply).delete(disarm_all_standing_applies),
        )
        .route(
            "/standing-applies/:thread_id",
            axum::routing::delete(disarm_standing_apply),
        )
        .route("/changes", get(list_changes))
        .route("/changes/applied", get(list_applied_changes))
        .route("/changes/apply-all", post(apply_all_changes))
        .route("/changes/apply-all/cancel", post(cancel_apply_all_changes))
        .route("/changes/lane/cancel", post(cancel_lane_batch))
        .route("/changes/discard-all", post(discard_all_changes))
        .route("/changes/for-repo/:repo_id", get(list_changes_for_repo))
        .route("/changes/:id/apply", post(apply_change))
        .route("/changes/:id/discard", post(discard_change))
        .route("/changes/:id/set-aside", post(set_aside_change))
        .route("/changes/:id/bring-back", post(bring_back_change))
        .route("/changes/:id/revert", post(revert_change))
        .route(
            "/changes/:id/queue",
            axum::routing::delete(remove_from_landing_lane),
        )
        .route(
            "/changes/:id/diff",
            get(super::repositories::get_change_diff),
        )
        .route(
            "/changes/:id/file",
            get(super::repositories::get_change_file),
        )
        .route("/changes/:id", get(get_change))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_answers_each_lane_decision_with_its_own_status() {
        let refused = |removal| queue_removal_answer(removal).expect_err("refused");
        let body = queue_removal_answer(Some(Ok(()))).expect("removed").0;
        assert_eq!(body["message"], "Removed from the landing lane.");

        let in_batch = refused(Some(Err(LaneRemoval::InBatch)));
        assert_eq!(in_batch.status, StatusCode::CONFLICT);
        assert_eq!(in_batch.reason, Some("in_lane_batch"));
        assert!(
            in_batch.message.contains("Cancel the batch"),
            "{}",
            in_batch.message
        );

        let not_waiting = refused(Some(Err(LaneRemoval::NotWaiting)));
        assert_eq!(not_waiting.status, StatusCode::CONFLICT);
        assert_eq!(not_waiting.reason, Some("not_waiting_to_land"));

        assert_eq!(refused(None).status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_refusal_with_nothing_ready_points_at_apply_all_on_settle() {
        let msg = empty_apply_all_refusal(2, 2, 0);
        assert!(msg.contains("\"Apply all on settle\""), "{msg}");
        assert!(!msg.contains("Keep going"), "{msg}");
    }

    #[test]
    fn a_refusal_with_every_ready_change_applying_says_so() {
        let msg = empty_apply_all_refusal(3, 0, 3);
        assert!(msg.contains("already applying"), "{msg}");
        let emptied = empty_apply_all_refusal(3, 0, 0);
        assert!(!emptied.contains("already applying"), "{emptied}");
    }

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

    async fn seed_cc_thread(
        pool: &sqlx::PgPool,
        thread_id: Uuid,
        status: &str,
        live_event_waits: i32,
        active_children: i32,
    ) {
        sqlx::query(
            "INSERT INTO thread_summaries
                (thread_id, is_coding_agent, status, coding_agent_change_state,
                 live_event_wait_count, active_children_count)
             VALUES ($1, true, $2, 'proposed', $3, $4)",
        )
        .bind(thread_id)
        .bind(status)
        .bind(live_event_waits)
        .bind(active_children)
        .execute(pool)
        .await
        .expect("seed thread_summaries");
    }

    async fn seed_change(
        pool: &sqlx::PgPool,
        change_id: Uuid,
        thread_id: Option<Uuid>,
        status: ChangeStatus,
        file_count: i32,
    ) {
        sqlx::query(
            "INSERT INTO changes
                (id, request_id, branch_name, repo_root, thread_id, status, file_count, files)
             VALUES ($1, $2, $3, '/tmp/repo', $4, $5, $6, $7)",
        )
        .bind(change_id)
        .bind(Uuid::new_v4())
        .bind(format!("branch-{}", change_id.as_simple()))
        .bind(thread_id)
        .bind(status)
        .bind(file_count)
        .bind(vec!["a.rs".to_string(); file_count.max(0) as usize])
        .execute(pool)
        .await
        .expect("seed changes");
    }

    #[tokio::test]
    async fn an_unsettled_thread_is_refused_and_settling_is_told_from_parked() {
        use crate::engine::thread_lifecycle::ThreadAction;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;

        for (status, waits, children, expected) in [
            ("running", 0, 0, ChangeActionRefusal::ThreadSettling),
            ("idle", 1, 0, ChangeActionRefusal::ThreadSettling),
            ("idle", 1, 1, ChangeActionRefusal::ThreadSettling),
            ("failed", 1, 0, ChangeActionRefusal::ThreadParked),
            (
                "waiting_for_user_answer",
                0,
                0,
                ChangeActionRefusal::ThreadParked,
            ),
        ] {
            let thread_id = Uuid::new_v4();
            let change_id = Uuid::new_v4();
            seed_cc_thread(&pool, thread_id, status, waits, children).await;
            seed_change(&pool, change_id, Some(thread_id), ChangeStatus::Pending, 3).await;

            let refusal = change_action_refusal(&pool, change_id, ThreadAction::Apply)
                .await
                .expect("ask the gate");
            assert_eq!(
                refusal,
                Some(expected),
                "status={status} waits={waits} children={children} refused for the wrong reason"
            );
        }

        teardown_test_db(&db_name).await;
    }

    #[test]
    fn only_the_settling_reason_is_one_a_standing_apply_waits_through() {
        use crate::engine::standing_apply::{
            standing_verdict, ArmedChange, SettleFacts, StandingVerdict, TurnSettle,
        };
        use crate::engine::thread_lifecycle::CodingAgentChangeState;

        let facts = |status: &str, waits: bool| SettleFacts {
            status: status.to_string(),
            parked_on_question: false,
            live_event_waits: waits,
            change_state: CodingAgentChangeState::Proposed {
                requires_restart: false,
            },
            armed_change: ArmedChange::Ready(Uuid::new_v4()),
            turn_settle: TurnSettle::Settled,
        };
        let waits_it_out = |f: SettleFacts| matches!(standing_verdict(&f), StandingVerdict::Wait);
        for (status, waits) in [("running", false), ("paused", false), ("idle", true)] {
            assert!(
                waits_it_out(facts(status, waits)),
                "a settling thread ({status}, waits={waits}) is what a standing apply waits through",
            );
        }
        assert!(
            !waits_it_out(facts("waiting_for_user_answer", false)),
            "a thread parked on a question drops the arm, so it must not read as settling",
        );
    }

    #[tokio::test]
    async fn a_settled_thread_is_not_refused_even_with_a_running_child() {
        use crate::engine::thread_lifecycle::ThreadAction;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;
        for children in [0, 1] {
            let thread_id = Uuid::new_v4();
            let change_id = Uuid::new_v4();
            seed_cc_thread(&pool, thread_id, "idle", 0, children).await;
            seed_change(&pool, change_id, Some(thread_id), ChangeStatus::Pending, 3).await;

            for action in [ThreadAction::Apply, ThreadAction::Discard] {
                assert_eq!(
                    change_action_refusal(&pool, change_id, action)
                        .await
                        .expect("ask the gate"),
                    None,
                    "children={children} {action:?} must pass the gate",
                );
            }
        }
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_thread_that_will_never_offer_apply_says_so() {
        use crate::engine::thread_lifecycle::ThreadAction;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;
        let thread_id = Uuid::new_v4();
        let change_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries (thread_id, is_coding_agent, status)
             VALUES ($1, false, 'idle')",
        )
        .bind(thread_id)
        .execute(&pool)
        .await
        .expect("seed a chat thread");
        seed_change(&pool, change_id, Some(thread_id), ChangeStatus::Pending, 3).await;

        assert_eq!(
            change_action_refusal(&pool, change_id, ThreadAction::Apply)
                .await
                .expect("ask the gate"),
            Some(ChangeActionRefusal::ActionUnavailable),
        );
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn an_idle_thread_with_an_open_question_is_refused_apply() {
        use crate::engine::thread_lifecycle::ThreadAction;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;
        let thread_id = Uuid::new_v4();
        let change_id = Uuid::new_v4();
        seed_cc_thread(&pool, thread_id, "idle", 0, 0).await;
        seed_change(&pool, change_id, Some(thread_id), ChangeStatus::Pending, 3).await;
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id)
             VALUES ($1, 'thread', $2, 'UserQuestionAsked', '{}'::jsonb, NOW(), $2::uuid)",
        )
        .bind(Uuid::new_v4())
        .bind(thread_id.to_string())
        .execute(&pool)
        .await
        .expect("record an open question");

        assert_eq!(
            change_action_refusal(&pool, change_id, ThreadAction::Apply)
                .await
                .expect("ask the gate"),
            Some(ChangeActionRefusal::ThreadParked),
        );
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn an_empty_pending_change_is_refused_for_apply_only() {
        use crate::engine::thread_lifecycle::ThreadAction;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;
        let thread_id = Uuid::new_v4();
        let change_id = Uuid::new_v4();
        seed_cc_thread(&pool, thread_id, "idle", 0, 0).await;
        seed_change(&pool, change_id, Some(thread_id), ChangeStatus::Pending, 0).await;

        assert_eq!(
            change_action_refusal(&pool, change_id, ThreadAction::Apply)
                .await
                .expect("ask the gate"),
            Some(ChangeActionRefusal::NoFilesLeft),
        );
        assert_eq!(
            change_action_refusal(&pool, change_id, ThreadAction::Discard)
                .await
                .expect("ask the gate"),
            None,
            "discard is how an empty change is resolved",
        );
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn set_aside_is_refused_where_discard_is() {
        use crate::engine::thread_lifecycle::ThreadAction;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;
        for (status, waits) in [
            ("idle", 0),
            ("running", 0),
            ("idle", 1),
            ("failed", 1),
            ("waiting_for_user_answer", 0),
        ] {
            let thread_id = Uuid::new_v4();
            let change_id = Uuid::new_v4();
            seed_cc_thread(&pool, thread_id, status, waits, 0).await;
            seed_change(&pool, change_id, Some(thread_id), ChangeStatus::Pending, 3).await;
            let ask = |action| change_action_refusal(&pool, change_id, action);
            assert_eq!(
                ask(ThreadAction::Hold).await.expect("ask the gate"),
                ask(ThreadAction::Discard).await.expect("ask the gate"),
                "status={status} waits={waits}: Set aside must follow Discard",
            );
        }
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_set_aside_change_is_not_discarded_under_a_working_thread() {
        use crate::engine::thread_lifecycle::ThreadAction;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;
        for (status, expected) in [
            ("running", Some(ChangeActionRefusal::ThreadSettling)),
            ("idle", None),
        ] {
            let thread_id = Uuid::new_v4();
            let change_id = Uuid::new_v4();
            seed_cc_thread(&pool, thread_id, status, 0, 0).await;
            seed_change(&pool, change_id, Some(thread_id), ChangeStatus::SetAside, 3).await;
            assert_eq!(
                change_action_refusal(&pool, change_id, ThreadAction::Discard)
                    .await
                    .expect("ask the gate"),
                expected,
                "thread {status}"
            );
        }
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_set_aside_change_is_refused_apply_until_brought_back() {
        use crate::engine::thread_lifecycle::ThreadAction;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;
        let thread_id = Uuid::new_v4();
        let change_id = Uuid::new_v4();
        seed_cc_thread(&pool, thread_id, "idle", 0, 0).await;
        seed_change(&pool, change_id, Some(thread_id), ChangeStatus::SetAside, 3).await;

        assert_eq!(
            change_action_refusal(&pool, change_id, ThreadAction::Apply)
                .await
                .expect("ask the gate"),
            Some(ChangeActionRefusal::ChangeSetAside),
        );
        assert_eq!(
            change_action_refusal(&pool, change_id, ThreadAction::Hold)
                .await
                .expect("ask the gate"),
            None,
        );
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn unknown_threadless_and_resolved_rows_fall_through() {
        use crate::engine::thread_lifecycle::ThreadAction;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;

        let unknown = Uuid::new_v4();
        assert_eq!(
            change_action_refusal(&pool, unknown, ThreadAction::Apply)
                .await
                .expect("ask the gate"),
            None,
            "an unknown id is the engine's 'Change not found'",
        );

        let threadless = Uuid::new_v4();
        seed_change(&pool, threadless, None, ChangeStatus::Pending, 2).await;
        assert_eq!(
            change_action_refusal(&pool, threadless, ThreadAction::Apply)
                .await
                .expect("ask the gate"),
            None,
        );

        let thread_id = Uuid::new_v4();
        let resolved = Uuid::new_v4();
        seed_cc_thread(&pool, thread_id, "running", 0, 0).await;
        seed_change(&pool, resolved, Some(thread_id), ChangeStatus::Applied, 2).await;
        assert_eq!(
            change_action_refusal(&pool, resolved, ThreadAction::Apply)
                .await
                .expect("ask the gate"),
            None,
        );

        teardown_test_db(&db_name).await;
    }
}
