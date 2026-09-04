use super::actor::require_user_actor;
use super::thread_reach::{refuse_without_authority, ThreadReachVerb};
use super::*;
use crate::engine::apply_all_driver::ApplyAllOutcome;
use crate::engine::standing_apply::{DisarmScope, StandingApply, DISARMED_BY_OWNER};
use crate::engine::{ApplyResult, ApplyStatus};
use axum::http::HeaderMap;

/// Refuse a change verb this caller has no authority for (ADR 0168 clause 4).
///
/// The change's own thread is the target: applying a change acts on the thread
/// that proposed it, so a parent applying its child's work stays in-subtree.
///
/// A change id naming nothing falls through. The engine's own "Change not
/// found" is the honest answer, and a gate here would leak whether the id
/// exists. A change naming no thread sits in nobody's subtree, so it takes the
/// owner's standing instruction.
async fn refuse_change_verb(
    state: &AppState,
    headers: &HeaderMap,
    change_id: Uuid,
    verb: ThreadReachVerb,
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
    Ok(refuse_without_authority(&state.pool, headers, thread_id, verb).await?)
}

/// The same gate for a batch, whose target is every change it will touch.
///
/// A thread caller passes only when every member sits in its own subtree, which
/// is clause 3 applied member by member. The first member outside it needs the
/// owner's standing instruction, exactly as a single change would.
///
/// Gated on the FILTERED list, the one the batch actually applies, so the
/// authority question covers what happens rather than what was proposed.
async fn refuse_batch_change_verb(
    state: &AppState,
    headers: &HeaderMap,
    batch: &[crate::core::changes::Change],
    verb: ThreadReachVerb,
) -> Result<(), ApiError> {
    for change in batch {
        refuse_without_authority(&state.pool, headers, change.thread_id, verb).await?;
    }
    Ok(())
}

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
            crate::log!(
                "[Changes] broadcast: list_pending: {}, skipping broadcast",
                e
            );
            return;
        }
    };
    let mut applied = match applied_r {
        Ok(v) => v,
        Err(e) => {
            crate::log!(
                "[Changes] broadcast: list_recently_applied: {}, skipping broadcast",
                e
            );
            return;
        }
    };
    let restart_groups = match restart_r {
        Ok(v) => v,
        Err(e) => {
            crate::log!(
                "[Changes] broadcast: restart_groups_since: {}, skipping broadcast",
                e
            );
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
    if let Err(e) = crate::core::changes::enrich_thread_unsettled(pool, &mut pending).await {
        crate::log!(
            "[Changes] broadcast: enrich pending thread_unsettled: {}",
            e
        );
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
) -> Result<Json<serde_json::Value>, ApiError> {
    let limit = query.limit.unwrap_or(15).clamp(1, 100);
    let before_ts = query.before.map(super::parse_unix_ts);

    let pool = state.engine.pool();
    let proj = state.engine.changes();
    // Fetch limit+1 on applied to detect has_more. The durable `apply_all_batches`
    // mirror holds a row only while a batch is in flight (inserted on start,
    // deleted on complete/cancel/recovery), so its non-emptiness is the
    // cross-reload truth for the "Applying changes…" toast — the driving
    // `applyAllInProgress` signal resets on reload and the ApplyAllBatch* SSE
    // events aren't replayed. Joined with the other reads — independent query.
    let (pending_r, applied_r, client_update_r, restart_groups_r, apply_all_r, working_r) = tokio::join!(
        proj.list_pending(),
        proj.list_recently_applied(limit + 1, before_ts),
        proj.client_update_since(state.started_at),
        proj.restart_groups_since(state.started_at),
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM apply_all_batches)")
            .fetch_one(pool),
        crate::engine::standing_apply::count_sweep_candidates(pool),
    );
    let mut pending = pending_r.map_err(ApiError::db)?;
    let mut applied = applied_r.map_err(ApiError::db)?;
    let client_update = client_update_r.map_err(ApiError::db)?;
    let mut restart_groups = restart_groups_r.map_err(ApiError::db)?;
    let apply_all_in_progress = apply_all_r.map_err(ApiError::db)?;
    let working_thread_count = working_r.map_err(ApiError::db)?;
    let has_more_applied = applied.len() as i64 > limit;
    if has_more_applied {
        applied.truncate(limit as usize);
    }

    let (r1, r2, r3) = tokio::join!(
        crate::core::changes::enrich_thread_titles(pool, &mut pending),
        crate::core::changes::enrich_thread_titles(pool, &mut applied),
        crate::core::changes::enrich_restart_group_titles(pool, &mut restart_groups),
    );
    r1.map_err(ApiError::db)?;
    r2.map_err(ApiError::db)?;
    r3.map_err(ApiError::db)?;

    // Flag pending changes whose thread has not settled, mid-turn or parked, so
    // the UI disables Apply and the bulk paths drop them. Same gate the
    // per-change endpoint enforces server-side via guard_change_action.
    crate::core::changes::enrich_thread_unsettled(pool, &mut pending)
        .await
        .map_err(ApiError::db)?;

    Ok(Json(serde_json::json!({
        "pending": pending,
        "applied": applied,
        "total_pending": pending.len(),
        "restart_required": !restart_groups.is_empty(),
        "restart_groups": restart_groups,
        "client_update_available": client_update,
        "has_more_applied": has_more_applied,
        "apply_all_in_progress": apply_all_in_progress,
        // Threads carrying a standing apply. Keyed by THREAD, not by change: a
        // sweep arms a thread that has proposed nothing yet, and the prompt row
        // still has to render its armed state.
        "standing_apply_thread_ids": state.engine.armed_standing_apply_threads(),
        // Coding-agent threads still working, so a sweep has something to arm.
        // The panel offers "Apply as they settle" off this, and cannot derive
        // it: its thread map holds only the loaded window.
        "working_thread_count": working_thread_count,
    })))
}

/// GET /api/v1/changes/applied — list recently applied changes with pagination
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

/// POST /api/v1/changes/:id/revert — revert an applied change
pub(super) async fn revert_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    match state.engine.revert_change(id, actor).await {
        Ok(message) => {
            broadcast_changes(&state).await;
            Ok(Json(serde_json::json!({ "message": message })))
        }
        Err(e) => Err(ApiError::bad_request(e.to_string())),
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
) -> Result<(), ApiError> {
    let row: Option<(Option<Uuid>, String, i32)> =
        sqlx::query_as("SELECT thread_id, status, file_count FROM changes WHERE id = $1")
            .bind(change_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::db)?;
    // A pending change with no files left has nothing to apply — merging it
    // only pushes no-op commits and can spend a harden-at-apply session on an
    // empty diff. Refused before the thread-state gate so the message names the
    // real reason, and only for Apply: Discard is how the user resolves one.
    // See `core::changes::is_empty_pending_change`.
    if let Some((_, status, file_count)) = row.as_ref() {
        if status == "pending"
            && *file_count == 0
            && action == crate::engine::thread_lifecycle::Action::Apply
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "This change has no file changes left — discard it instead",
            ));
        }
    }
    // Unknown change id, change with no thread, or an already-resolved change:
    // defer to the engine (it returns "Change not found" or handles the
    // idempotent terminal-state retry).
    let Some((Some(thread_id), status, _)) = row else {
        return Ok(());
    };
    if status != "pending" {
        return Ok(());
    }
    let actions = crate::api::threads::available_thread_actions_for(&state.pool, thread_id)
        .await
        .map_err(ApiError::db)?;
    if !actions.contains(&action) {
        return Err(ApiError::new(StatusCode::CONFLICT, reject_msg));
    }
    Ok(())
}

/// POST /api/v1/changes/:id/apply — apply a single change
pub(super) async fn apply_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ApplyResult>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    refuse_change_verb(&state, &headers, id, ThreadReachVerb::Apply).await?;
    guard_change_action(
        &state,
        id,
        crate::engine::thread_lifecycle::Action::Apply,
        "This change can't be applied in the thread's current state",
    )
    .await?;
    // Apply-time reconcile of orphaned sibling pending changes is handled inside
    // `apply_change` itself (gated on a real Applied transition), so it covers
    // this handler, the no-live apply_now path, and the Apply-All driver uniformly.
    match state.engine.apply_change(id, actor).await {
        Ok(result) => {
            broadcast_changes(&state).await;
            Ok(Json(result))
        }
        Err(e) => Err(ApiError::bad_request(e.to_string())),
    }
}

/// POST /api/v1/changes/:id/discard — discard a single change
pub(super) async fn discard_change(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    refuse_change_verb(&state, &headers, id, ThreadReachVerb::Discard).await?;
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
        Err(e) => Err(ApiError::bad_request(e.to_string())),
    }
}

/// Body of `POST /api/v1/standing-applies`.
#[derive(serde::Deserialize)]
pub(super) struct ArmStandingApplyBody {
    thread_id: Uuid,
    /// The change to apply. Omit it for a thread that has proposed nothing yet,
    /// and the arm takes whatever it proposes.
    #[serde(default)]
    change_id: Option<Uuid>,
}

/// POST /api/v1/standing-applies: arm a standing apply for one thread.
///
/// The owner's instruction to apply once the thread settles (ADR 0168 clause
/// 5). Re-arming a thread replaces its previous arm.
pub(super) async fn arm_standing_apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArmStandingApplyBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    // Arming an apply on a thread IS applying it, one settle later, so it takes
    // the gate the immediate apply takes. The `apply_when_settled` LLM tool is
    // the other way to this same act, and asks the same rule.
    refuse_without_authority(
        &state.pool,
        &headers,
        Some(body.thread_id),
        ThreadReachVerb::Apply,
    )
    .await?;
    // A change named here must be this thread's own pending one. Binding an
    // arm to somebody else's change would apply work the owner never saw on
    // this thread's settle.
    if let Some(change_id) = body.change_id {
        let row: Option<(Option<Uuid>, String)> =
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
            Some((_, status)) if status != "pending" => {
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
    broadcast_changes(&state).await;
    Ok(Json(serde_json::json!({
        "message": "Will apply when the thread settles.",
    })))
}

/// DELETE /api/v1/standing-applies/:thread_id: take the instruction back.
pub(super) async fn disarm_standing_apply(
    State(state): State<AppState>,
    Path(thread_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    // Taking an apply back acts on the same thread's apply, so it asks the gate
    // the arm asks. A caller that could not have armed here may not cancel here.
    refuse_without_authority(
        &state.pool,
        &headers,
        Some(thread_id),
        ThreadReachVerb::Apply,
    )
    .await?;
    let dropped = state
        .engine
        .drop_standing_apply(thread_id, DISARMED_BY_OWNER, actor)
        .await;
    if !dropped {
        return Err(ApiError::not_found("No standing apply on that thread"));
    }
    broadcast_changes(&state).await;
    Ok(Json(
        serde_json::json!({ "message": "Standing apply canceled." }),
    ))
}

/// DELETE /api/v1/standing-applies: take back every standing apply here.
///
/// The workspace-scope off, which the Changes panel's "Apply as they settle"
/// toggle presses. It drops a single arm as readily as a swept one. That panel
/// draws ONE armed state for the workspace, so its off has to mean the same.
///
/// Nothing armed answers 0 rather than 404. This is an off switch, and the
/// owner pressing it on an already-off state got what they asked for. The
/// per-thread route keeps its 404: naming a thread is a claim about that
/// thread.
///
/// It stops no Apply All batch. Cancelling one is
/// `POST /api/v1/changes/apply-all/cancel`, which also takes the sweep's arms.
pub(super) async fn disarm_all_standing_applies(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    // Workspace scope, so no subtree contains it. Same reasoning as Apply All.
    refuse_without_authority(&state.pool, &headers, None, ThreadReachVerb::Apply).await?;
    let disarmed = state
        .engine
        .drop_standing_applies(DisarmScope::All, DISARMED_BY_OWNER, actor)
        .await;
    broadcast_changes(&state).await;
    Ok(Json(serde_json::json!({ "disarmed": disarmed })))
}

/// Query for `POST /api/v1/changes/apply-all`.
#[derive(serde::Deserialize, Default)]
pub(super) struct ApplyAllQuery {
    /// "Keep going as the rest settle": arm every thread still working, so its
    /// change applies when it lands.
    #[serde(default)]
    keep_going: bool,
}

/// What Apply All says when nothing could be applied now and the checkbox was
/// off. Pure, so each refusal names the real reason rather than one blanket
/// message.
pub(super) fn empty_apply_all_refusal(total_pending: usize, unsettled: usize) -> &'static str {
    if total_pending == 0 {
        "No pending changes"
    } else if unsettled == total_pending {
        "All pending changes belong to threads that are still working or waiting for something. \
         Turn on \"Keep going as the rest settle\", or wait for them to finish."
    } else {
        "All pending changes that could be applied have no file changes left. Discard them instead."
    }
}

/// What "Apply as they settle" says once it has armed. Pure.
pub(super) fn apply_as_they_settle_message(armed: usize) -> String {
    match armed {
        0 => "Nothing is working right now, so there is nothing to apply as it settles.".into(),
        1 => "Will apply 1 thread's change as it settles.".into(),
        n => format!("Will apply {n} threads' changes as they settle."),
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
    Query(query): Query<ApplyAllQuery>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    // Apply All aims at the workspace rather than at one thread, and with the
    // checkbox on it arms threads that have proposed nothing yet. No subtree
    // contains that, so clause 3 cannot cover it and a thread caller needs the
    // owner's standing instruction. Before the batch record and the first
    // merge, so a refusal leaves neither.
    refuse_without_authority(&state.pool, &headers, None, ThreadReachVerb::Apply).await?;
    // The rule lives on the engine, so the agent's `apply_as_they_settle` runs
    // the same press this button does.
    let outcome = state
        .engine
        .run_apply_all(actor, query.keep_going)
        .await
        .map_err(ApiError::db)?;

    let (total_pending, unsettled, armed) = match &outcome {
        ApplyAllOutcome::NothingToApply {
            total_pending,
            unsettled,
            armed,
        } => (*total_pending, *unsettled, *armed),
        ApplyAllOutcome::Started { .. } => (0, 0, 0),
    };
    if let ApplyAllOutcome::NothingToApply { .. } = outcome {
        // With the checkbox on, the sweep IS the action: "Apply as they settle".
        if !query.keep_going {
            return Err(ApiError::bad_request(empty_apply_all_refusal(
                total_pending,
                unsettled,
            )));
        }
        broadcast_changes(&state).await;
        return Ok(Json(serde_json::json!({
            "batch_size": 0,
            "armed": armed,
            "message": apply_as_they_settle_message(armed),
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

    broadcast_changes(&state).await;
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

/// POST /api/v1/changes/apply-all/cancel — cancel the running Apply All batch.
///
/// Stops the driver from advancing to further members, interrupts the in-flight
/// hardening/merge session, and emits `ApplyAllBatchCompleted`. Already-applied
/// members stay applied; the in-flight apply aborts back to pending (best-effort
/// for an in-progress merge); queued members stay pending. See
/// `cancel_apply_all_batches` for the full semantics.
///
/// It also takes back every arm a sweep set. Cancel means "stop applying", and
/// leaving the sweep running would keep applying for hours afterwards. A single
/// arm the owner set on one change is not part of the sweep and survives.
pub(super) async fn cancel_apply_all_changes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = Some(require_user_actor(&headers, &state.pool, None).await?);
    let canceled = state.engine.cancel_apply_all_batches(actor.clone()).await;
    let disarmed = state
        .engine
        .drop_standing_applies(DisarmScope::Sweep, DISARMED_BY_OWNER, actor)
        .await;
    if canceled == 0 && disarmed == 0 {
        return Err(ApiError::bad_request("No Apply All batch is running"));
    }
    broadcast_changes(&state).await;
    Ok(Json(serde_json::json!({
        "canceled_batches": canceled,
        "disarmed": disarmed,
    })))
}

/// GET /api/v1/changes/:id — get a single change by ID
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
    // Skip changes whose thread has not settled, mid-turn or parked. Discarding
    // would delete the branch and worktree out from under a session that is
    // still going. Mirrors the Apply All filter and `guard_change_action`.
    let pending = crate::core::changes::drop_unsettled_thread_changes(&state.pool, all_pending)
        .await
        .map_err(ApiError::db)?;
    if pending.is_empty() {
        return Err(ApiError::bad_request(
            "All pending changes belong to threads that are still working or waiting for something. Wait for them to finish, then discard.",
        ));
    }
    // Before the first discard, so a refusal deletes no branch and no worktree.
    refuse_batch_change_verb(&state, &headers, &pending, ThreadReachVerb::Discard).await?;
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

/// Routes for the `/changes*` URL surface. The diff/file routes register
/// here even though their handlers live in `api::repositories` — grouped by
/// path, not handler location.
///
/// `/standing-applies` joins them: it is a change action, and one row per
/// armed thread is the resource it acts on. The collection takes a DELETE of
/// its own, which is the workspace-scope off.
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
        .route("/changes/discard-all", post(discard_all_changes))
        .route("/changes/for-repo/:repo_id", get(list_changes_for_repo))
        .route("/changes/:id/apply", post(apply_change))
        .route("/changes/:id/discard", post(discard_change))
        .route("/changes/:id/revert", post(revert_change))
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
