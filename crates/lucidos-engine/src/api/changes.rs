use super::actor::require_user_actor;
use super::thread_reach::{refuse_without_authority, ThreadReachVerb};
use super::*;
use crate::core::changes::{ChangeStatus, PendingScope};
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
    let (pending_r, applied_r, client_update_r, restart_groups_r, apply_all_r, settling_r) = tokio::join!(
        crate::core::changes::list_pending_for_readers(
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
        crate::engine::standing_apply::count_sweep_candidates(pool),
    );
    let pending = pending_r.map_err(ApiError::db)?;
    let mut applied = applied_r.map_err(ApiError::db)?;
    let client_update = client_update_r.map_err(ApiError::db)?;
    let mut restart_groups = restart_groups_r.map_err(ApiError::db)?;
    let apply_all_in_progress = apply_all_r.map_err(ApiError::db)?;
    let settling_thread_count = settling_r.map_err(ApiError::db)?;
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
        // Coding-agent threads still settling, so a sweep has something to arm.
        // The panel offers "Apply as they settle" off this, and cannot derive
        // it: its thread map holds only the loaded window.
        "settling_thread_count": settling_thread_count,
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
    // Reverting an applied change is at least as destructive as discarding a
    // pending one, so it takes the same subtree-reach gate its siblings do. The
    // change keeps its `thread_id` after applying, so `refuse_change_verb` scopes
    // revert to the change's own thread exactly like discard.
    refuse_change_verb(&state, &headers, id, ThreadReachVerb::Revert).await?;
    match state.engine.revert_change(id, actor).await {
        Ok(message) => {
            state.engine.broadcast_changes_updated().await;
            Ok(Json(serde_json::json!({ "message": message })))
        }
        Err(e) => Err(ApiError::bad_request(e.to_string())),
    }
}

/// Why a single-change action is refused right now. See
/// [`change_action_refusal`], which owns the rule.
///
/// The three thread-state variants stay apart because only ONE of them can be
/// waited out with a *standing apply*. `standing_verdict` waits through a
/// *settling* thread, and drops on a parked one. Collapse the two
/// and a surface points the caller at a control that ends the moment it is
/// pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeActionRefusal {
    /// A pending change whose branch left nothing to merge. Apply only.
    NoFilesLeft,
    /// The thread is *settling*: running, paused, or watching an event. It will
    /// settle by itself, and a *standing apply* is what waits for that.
    ThreadSettling,
    /// The thread is *parked*: unsettled, and not settling. It is on a
    /// question card, or its turn failed while it still watches an event.
    /// Only the user moves it on, so a standing apply cannot wait it out.
    ThreadParked,
    /// The selector withholds the action for a reason no wait resolves.
    ActionUnavailable,
}

/// Why the availability selector refuses `action` on this change, or `None`.
///
/// **The single definition of the per-change gate.** Two surfaces ask it and
/// write their own sentence: the HTTP handlers as a 409, the `changes` LLM
/// tool as a tool error. The reason is typed because a 409 body and a message
/// an agent reads want different words. ADR 0106 turned down a frontend-only
/// hide for splitting one rule in two, and a tool with no gate was that split
/// again. See
/// `docs/plans/2026-09-20-an-apply-the-agent-asks-for-takes-the-same-gate.md`.
///
/// Internal apply paths are intentionally NOT gated. They run while the thread
/// legitimately reads running: the Apply All driver, `apply_now`, the
/// post-hardening auto-apply, the standing-apply resolver.
/// `change_ops_tests.rs` enrolls every one of them.
///
/// Three rows fall through to the engine, which answers them better: an
/// unknown id, a threadless change, and a resolved one. Gating a terminal row
/// would turn an idempotent retry into a spurious 409.
pub(crate) async fn change_action_refusal(
    pool: &sqlx::PgPool,
    change_id: Uuid,
    action: crate::engine::thread_lifecycle::Action,
) -> Result<Option<ChangeActionRefusal>, sqlx::Error> {
    let row: Option<(Option<Uuid>, ChangeStatus, i32)> =
        sqlx::query_as("SELECT thread_id, status, file_count FROM changes WHERE id = $1")
            .bind(change_id)
            .fetch_optional(pool)
            .await?;
    // A pending change with no files left has nothing to apply. Merging it only
    // pushes no-op commits, and can spend a harden-at-apply session on an empty
    // diff. Refused before the thread-state gate so the message names the real
    // reason, and only for Apply: Discard is how the user resolves one. See
    // `core::changes::is_empty_pending_change`.
    if let Some((_, status, file_count)) = row.as_ref() {
        if *status == ChangeStatus::Pending
            && *file_count == 0
            && action == crate::engine::thread_lifecycle::Action::Apply
        {
            return Ok(Some(ChangeActionRefusal::NoFilesLeft));
        }
    }
    let Some((Some(thread_id), status, _)) = row else {
        return Ok(None);
    };
    if status != ChangeStatus::Pending {
        return Ok(None);
    }
    let actions = crate::api::threads::available_thread_actions_for(pool, thread_id).await?;
    if actions.contains(&action) {
        return Ok(None);
    }
    // Two extra reads, on the refusal path only. They ask the two canonical
    // predicates rather than a third copy of their SQL, which is the drift this
    // whole function exists to stop. `settling` is asked first because a
    // settling thread is often also unsettled, and settling is the stronger,
    // actionable answer.
    if crate::engine::standing_apply::settling_thread_ids(pool, std::iter::once(thread_id))
        .await?
        .contains(&thread_id)
    {
        return Ok(Some(ChangeActionRefusal::ThreadSettling));
    }
    if crate::core::changes::unsettled_thread_ids(pool, std::iter::once(thread_id))
        .await?
        .contains(&thread_id)
    {
        return Ok(Some(ChangeActionRefusal::ThreadParked));
    }
    Ok(Some(ChangeActionRefusal::ActionUnavailable))
}

/// Reject a single-change action the selector does not grant, as a 409. The
/// HTTP rendering of [`change_action_refusal`].
async fn guard_change_action(
    state: &AppState,
    change_id: Uuid,
    action: crate::engine::thread_lifecycle::Action,
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
        // The three thread-state refusals are one 409 here. The panel already
        // draws the difference, and the caller's sentence has always said
        // "in the thread's current state".
        Some(_) => reject_msg,
    };
    Err(ApiError::new(StatusCode::CONFLICT, msg))
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
            state.engine.broadcast_changes_updated().await;
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
            state.engine.broadcast_changes_updated().await;
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
    // A 404 must mean "nothing was armed": the client reads it as already off.
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
        .await
        .map_err(ApiError::db)?;
    state.engine.broadcast_changes_updated().await;
    Ok(Json(serde_json::json!({ "disarmed": disarmed })))
}

/// Query for `POST /api/v1/changes/apply-all`.
#[derive(serde::Deserialize, Default)]
pub(super) struct ApplyAllQuery {
    /// "Keep going as the rest settle": arm every thread still settling, so its
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
        0 => "No thread is still settling, so there is nothing to apply as it settles.".into(),
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
        state.engine.broadcast_changes_updated().await;
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

    // ── the one per-change gate ──
    //
    // `change_action_refusal` is the whole rule. Both surfaces render it, so a
    // hole here is a hole in the Apply button AND in the `changes` LLM tool.

    /// Seed a coding-agent thread row in one state.
    async fn seed_cc_thread(
        pool: &sqlx::PgPool,
        thread_id: Uuid,
        status: &str,
        live_event_waits: i32,
        active_children: i32,
    ) {
        sqlx::query(
            "INSERT INTO thread_summaries
                (thread_id, is_coding_agent, status, coding_agent_proposed,
                 live_event_wait_count, active_children_count)
             VALUES ($1, true, $2, true, $3, $4)",
        )
        .bind(thread_id)
        .bind(status)
        .bind(live_event_waits)
        .bind(active_children)
        .execute(pool)
        .await
        .expect("seed thread_summaries");
    }

    /// Seed a change row owned by `thread_id`.
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

    /// Every unsettled thread is refused Apply, and the reason tells settling
    /// from parked.
    ///
    /// The split is the whole point. `standing_verdict` waits through a
    /// running thread and an event wait. It drops on a question card. So one
    /// shared reason would send the caller to a control that ends on its first
    /// look.
    #[tokio::test]
    async fn an_unsettled_thread_is_refused_and_settling_is_told_from_parked() {
        use crate::engine::thread_lifecycle::Action;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;

        // Mid-turn, watching an event (with and without a child), and parked
        // on a question. All of them wake and may commit again.
        for (status, waits, children, expected) in [
            ("running", 0, 0, ChangeActionRefusal::ThreadSettling),
            ("idle", 1, 0, ChangeActionRefusal::ThreadSettling),
            ("idle", 1, 1, ChangeActionRefusal::ThreadSettling),
            // A failed turn keeps its waits, and nothing but the user moves it.
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

            let refusal = change_action_refusal(&pool, change_id, Action::Apply)
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

    /// The reported reason and `standing_verdict` agree about who can be waited
    /// out. Only `ThreadSettling` may be answered with a standing apply.
    #[test]
    fn only_the_settling_reason_is_one_a_standing_apply_waits_through() {
        use crate::engine::standing_apply::{
            standing_verdict, ArmedChange, SettleFacts, StandingVerdict, TurnSettle,
        };

        let facts = |status: &str, waits: bool| SettleFacts {
            status: status.to_string(),
            live_event_waits: waits,
            has_diff: true,
            armed_change: ArmedChange::Ready(Uuid::new_v4()),
            turn_settle: TurnSettle::Settled,
        };
        let waits_it_out = |f: SettleFacts| matches!(standing_verdict(&f), StandingVerdict::Wait);
        // The states behind ThreadSettling. The arm keeps its place.
        for (status, waits) in [("running", false), ("paused", false), ("idle", true)] {
            assert!(
                waits_it_out(facts(status, waits)),
                "a settling thread ({status}, waits={waits}) is what a standing apply waits through",
            );
        }
        // The state behind ThreadParked. It ends the arm on its first look.
        assert!(
            !waits_it_out(facts("waiting_for_user_answer", false)),
            "a thread parked on a question drops the arm, so it must not read as settling",
        );
    }

    /// The control: a settled coding-agent thread with a real diff applies.
    /// Without this, a gate that refused everything would look correct.
    ///
    /// A delegating parent, idle apart from a running child, is settled too.
    /// The child writes its own worktree (ADR 0249), so Apply and Discard both
    /// pass the per-change gate.
    #[tokio::test]
    async fn a_settled_thread_is_not_refused_even_with_a_running_child() {
        use crate::engine::thread_lifecycle::Action;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;
        for children in [0, 1] {
            let thread_id = Uuid::new_v4();
            let change_id = Uuid::new_v4();
            seed_cc_thread(&pool, thread_id, "idle", 0, children).await;
            seed_change(&pool, change_id, Some(thread_id), ChangeStatus::Pending, 3).await;

            for action in [Action::Apply, Action::Discard] {
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

    /// A thread that withholds Apply for a reason no wait resolves reports
    /// `ActionUnavailable`. A chat thread is the plain case. Telling an agent
    /// to arm `apply_when_settled` here would arm a wait that never ends.
    #[tokio::test]
    async fn a_thread_that_will_never_offer_apply_says_so() {
        use crate::engine::thread_lifecycle::Action;
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
            change_action_refusal(&pool, change_id, Action::Apply)
                .await
                .expect("ask the gate"),
            Some(ChangeActionRefusal::ActionUnavailable),
        );
        teardown_test_db(&db_name).await;
    }

    /// An empty pending change is refused for Apply before the thread-state
    /// question, and Discard stays the way out of it.
    #[tokio::test]
    async fn an_empty_pending_change_is_refused_for_apply_only() {
        use crate::engine::thread_lifecycle::Action;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;
        let thread_id = Uuid::new_v4();
        let change_id = Uuid::new_v4();
        seed_cc_thread(&pool, thread_id, "idle", 0, 0).await;
        seed_change(&pool, change_id, Some(thread_id), ChangeStatus::Pending, 0).await;

        assert_eq!(
            change_action_refusal(&pool, change_id, Action::Apply)
                .await
                .expect("ask the gate"),
            Some(ChangeActionRefusal::NoFilesLeft),
        );
        assert_eq!(
            change_action_refusal(&pool, change_id, Action::Discard)
                .await
                .expect("ask the gate"),
            None,
            "discard is how an empty change is resolved",
        );
        teardown_test_db(&db_name).await;
    }

    /// Three rows the engine answers better than the gate does. Refusing them
    /// here would turn an idempotent retry into a 409.
    #[tokio::test]
    async fn unknown_threadless_and_resolved_rows_fall_through() {
        use crate::engine::thread_lifecycle::Action;
        use crate::test_support::{setup_test_db, teardown_test_db};

        let (pool, db_name) = setup_test_db().await;

        let unknown = Uuid::new_v4();
        assert_eq!(
            change_action_refusal(&pool, unknown, Action::Apply)
                .await
                .expect("ask the gate"),
            None,
            "an unknown id is the engine's 'Change not found'",
        );

        let threadless = Uuid::new_v4();
        seed_change(&pool, threadless, None, ChangeStatus::Pending, 2).await;
        assert_eq!(
            change_action_refusal(&pool, threadless, Action::Apply)
                .await
                .expect("ask the gate"),
            None,
        );

        // Already applied, on a thread that is mid-turn again. The thread state
        // must not reach a retry of a resolved change.
        let thread_id = Uuid::new_v4();
        let resolved = Uuid::new_v4();
        seed_cc_thread(&pool, thread_id, "running", 0, 0).await;
        seed_change(&pool, resolved, Some(thread_id), ChangeStatus::Applied, 2).await;
        assert_eq!(
            change_action_refusal(&pool, resolved, Action::Apply)
                .await
                .expect("ask the gate"),
            None,
        );

        teardown_test_db(&db_name).await;
    }
}
