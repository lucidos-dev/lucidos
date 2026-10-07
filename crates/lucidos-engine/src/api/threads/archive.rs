//! Cascading thread-archive: lock the family inside a transaction, classify
//! whether everyone can be archived, then emit `ThreadArchived` (plus
//! pending-change cleanup for external-repo CC descendants).
//!
//! The lock, the row shape and the decision live in [`super::family`], which
//! delete shares. The cascade itself is [`archive_family`], which the Archive
//! button's route and an agent's archive (ADR 0310) both run.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use std::collections::HashMap;

use crate::api::actor::SubprocessOrigin;
use crate::api::AppState;
use crate::engine::agent_question::{
    answer_pending_question, lookup_pending_question_tool_use_id, AnswerResult,
};
use crate::engine::thread_events::{ActorMode, AnswerKind, MessageOrigin};
use crate::engine::thread_lifecycle::{ArchiveState, Blocker, LifecycleViolation, ThreadStatus};
use crate::engine::{AgentArchiveAck, AgentArchiveError, LucidosEngine};

use super::extract_thread_uuid;
use super::family::{
    classify_family, external_repo_pending, load_family, not_yet_archived, with_blocker,
    FamilyDecision, FamilyRow, FamilyVerb, HOME_THREAD,
};

/// Map a reach refusal into this handler's `{reason, message}` body, the same
/// shape as every other rejection here. Status and slug both come from the
/// error, so this cannot drift from the taxonomy.
fn reach_rejection(
    e: crate::api::thread_reach::ThreadReachError,
) -> (StatusCode, axum::Json<serde_json::Value>) {
    (
        e.status_code(),
        axum::Json(serde_json::json!({
            "reason": e.reason(),
            "message": e.to_string(),
        })),
    )
}

/// Map any error into a JSON 500. Mirrors the `(StatusCode, String)` pattern
/// used elsewhere in the file, but with a JSON body so the cascade handler's
/// rejections (also JSON) share a consistent error shape.
fn internal_json<E: std::fmt::Display>(e: E) -> (StatusCode, axum::Json<serde_json::Value>) {
    log!("[API] archive_thread: {}", e);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(serde_json::json!({
            "reason": "internal_error",
            "message": e.to_string(),
        })),
    )
}

/// What a member that changed state after the lock dropped is told. The raw
/// lifecycle error names engine internals, so it goes to the log instead.
const LEFT_OPEN_MESSAGE: &str = "It changed state after the archive began, so it was left open";

/// The `skipped` entry for a member the cascade left unarchived: the change-claim
/// slug and the engine's words when a claim refused its stop, or
/// `not_archivable` with [`LEFT_OPEN_MESSAGE`].
fn skipped_member(thread_id: uuid::Uuid, reason: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "thread_id": thread_id, "reason": reason, "message": message })
}

/// The `409` for a target thread whose session a change claim holds.
fn claimed_target_rejection(
    holder: crate::engine::types::ChangeClaim,
) -> (StatusCode, axum::Json<serde_json::Value>) {
    (
        StatusCode::CONFLICT,
        axum::Json(serde_json::json!({
            "reason": crate::engine::claude_code::claim_refusal_slug(holder),
            "message": crate::engine::claude_code::claim_refusal_message(holder),
        })),
    )
}

/// A refusal as a route renders it: the status and a `{reason, message, ...}`
/// body.
pub(crate) type ArchiveRejection = (StatusCode, axum::Json<serde_json::Value>);

/// What one cascade archived, and the members it left open.
pub(crate) struct ArchiveOutcome {
    pub(crate) archived: Vec<Uuid>,
    pub(crate) skipped: Vec<serde_json::Value>,
}

impl ArchiveOutcome {
    fn into_body(self) -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({ "archived": self.archived, "skipped": self.skipped }))
    }
}

/// A refusal in the words an agent reads: the message, then the slug the
/// Archive route answers with.
pub(crate) fn rejection_text((_, body): &ArchiveRejection) -> String {
    let field = |key: &str| body.get(key).and_then(|v| v.as_str()).unwrap_or_default();
    format!("{} ({})", field("message"), field("reason"))
}

/// The Archive route's refusal for a thread waiting on the user (ADR 0259),
/// for a caller that checks before any family is locked.
pub(crate) fn waiting_rejection(thread_id: Uuid, has_pending_changes: bool) -> ArchiveRejection {
    let mut body = with_blocker(
        super::family::parent_blocked_body(
            FamilyVerb::Archive,
            ThreadStatus::WaitingForUserAnswer.as_str(),
            has_pending_changes,
        ),
        Blocker::Question,
    );
    body["message"] = gate_refusal_message(&body, thread_id).into();
    (StatusCode::CONFLICT, axum::Json(body))
}

/// Slug for a pinned thread an agent tried to archive (ADR 0312).
pub(crate) const THREAD_PINNED: &str = "thread_pinned";

/// What an agent is told about a pinned thread. The pin is the user's own
/// "keep this at hand", so only the user's Archive may unpin it.
fn pinned_message(thread_id: Uuid) -> String {
    format!(
        "Thread {thread_id} is pinned by the user, so an agent cannot archive it. \
         Leave it open, or ask the user to archive it."
    )
}

/// The refusal an agent gets for a pinned target (ADR 0312).
pub(crate) fn pinned_rejection(thread_id: Uuid) -> ArchiveRejection {
    (
        StatusCode::CONFLICT,
        axum::Json(serde_json::json!({
            "reason": THREAD_PINNED,
            "message": pinned_message(thread_id),
        })),
    )
}

/// The refusal an agent gets for the home thread (ADR 0362): the family
/// gate's own body and words, so the tool and the route say the same thing.
pub(crate) fn home_thread_rejection(thread_id: Uuid) -> ArchiveRejection {
    let mut body = with_blocker(serde_json::json!({ "reason": HOME_THREAD }), Blocker::Home);
    body["message"] = gate_refusal_message(&body, thread_id).into();
    (StatusCode::CONFLICT, axum::Json(body))
}

/// The family gate's refusal in words, for the `message` beside its slug.
fn gate_refusal_message(body: &serde_json::Value, thread_id: Uuid) -> String {
    let field = |key: &str| body.get(key).and_then(|v| v.as_str());
    match field("reason") {
        Some("thread_not_found") => format!("No thread {thread_id} exists in this workspace."),
        Some(HOME_THREAD) => format!(
            "Thread {thread_id} is the home thread. It never ends, so it cannot be archived."
        ),
        Some("parent_not_archivable")
            if field("parent_status") == Some(ThreadStatus::WaitingForUserAnswer.as_str()) =>
        {
            format!(
                "Thread {thread_id} is waiting on the user, so it cannot be archived. \
                 Answer or stop it first."
            )
        }
        Some("parent_not_archivable") => {
            format!("Thread {thread_id} is running, so it cannot be archived until its turn ends.")
        }
        Some("parent_has_pending_changes") => format!(
            "Thread {thread_id} holds a pending change. Apply or discard it before archiving."
        ),
        Some("descendants_blocking") => format!(
            "A sub-thread of {thread_id} is running, waiting on the user, or holds a pending \
             change, so the family cannot be archived yet."
        ),
        _ => format!("Thread {thread_id} cannot be archived right now."),
    }
}

/// POST /api/v1/threads/archive — cascading archive of a thread + every descendant.
///
/// The caller's reach is weighed first, then [`archive_family`] runs the
/// cascade. Response:
/// `{"archived": [<uuid>, ...], "skipped": [{thread_id, reason, message}, ...]}`.
pub(in crate::api) async fn archive_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<axum::Json<serde_json::Value>, ArchiveRejection> {
    let thread_uuid = extract_thread_uuid(&request).map_err(|(s, m)| {
        (
            s,
            axum::Json(serde_json::json!({ "reason": "bad_request", "message": m })),
        )
    })?;
    // Before the cascade, so a refusal writes nothing at all: no locked
    // family, no cleared pending change, no cancel-stamped question card.
    crate::api::thread_reach::refuse_without_authority(
        &state.pool,
        &headers,
        Some(thread_uuid),
        crate::api::thread_reach::ThreadReachVerb::Archive,
    )
    .await
    .map_err(reach_rejection)?;
    let actor = crate::api::actor::user_actor(&headers, None);
    archive_family(&state.engine, thread_uuid, actor, PinnedMembers::ByActor)
        .await
        .map(ArchiveOutcome::into_body)
}

/// `POST /api/v1/threads/:thread_id/archive`: archive on the caller's own
/// authority (ADR 0310). Shaped like the detach route, and for the same
/// reason: who is asking comes from the verified origin token, never the body.
///
/// - **A token-bearing caller** (an agent, or the CLI inside an agent session)
///   archives only itself or one of its own direct children.
/// - **A caller with no token** is the user's device or the local API, and
///   runs the Archive button's cascade on any thread.
pub(in crate::api) async fn archive_thread_as_caller(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<axum::Json<serde_json::Value>, ArchiveRejection> {
    // The token names the caller, which is what `current` resolves to. A
    // caller with no token has no thread, so the alias is refused there.
    let caller = match crate::api::actor::subprocess_origin(&headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } => Some(source_thread_id),
        SubprocessOrigin::NotSubprocess => None,
    };
    let target = crate::api::resolve_thread_id_arg(&thread_id, caller.flatten()).map_err(|m| {
        (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({ "reason": "bad_request", "message": m })),
        )
    })?;
    match caller {
        Some(caller_thread) => match state.engine.archive_as_agent(caller_thread, target).await {
            Ok(ack) => Ok(agent_ack_body(ack)),
            Err(AgentArchiveError::Refused(rejection)) => Err(rejection),
            Err(e) => Err((
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::FORBIDDEN),
                axum::Json(serde_json::json!({ "reason": e.reason(), "message": e.to_string() })),
            )),
        },
        None => {
            let actor = crate::api::actor::user_actor(&headers, None);
            archive_family(&state.engine, target, actor, PinnedMembers::ByActor)
                .await
                .map(ArchiveOutcome::into_body)
        }
    }
}

/// The body an agent's archive answers with: what was archived now, or that
/// the calling thread will be archived once its turn ends.
fn agent_ack_body(ack: AgentArchiveAck) -> axum::Json<serde_json::Value> {
    match ack {
        AgentArchiveAck::Archived(outcome) => outcome.into_body(),
        AgentArchiveAck::Requested { thread_id } => axum::Json(serde_json::json!({
            "requested": thread_id,
            "detail": "This thread is archived once its turn ends and it has settled.",
        })),
    }
}

/// What a cascade does with a pinned member (ADR 0312).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PinnedMembers {
    /// An agent leaves them open; the user's own Archive takes them.
    ByActor,
    /// Leave them open whoever asks. Archive all puts away only what is safe,
    /// and a pin is the user's "keep this at hand" (ADR 0349).
    LeaveOpen,
}

/// Which members of a locked family a cascade touches.
struct CascadePlan {
    to_archive: Vec<Uuid>,
    external_repo_pending: Vec<Uuid>,
    /// Pinned members an agent's archive leaves open, reported as skipped.
    left_pinned: Vec<Uuid>,
}

/// Plan the cascade over a family the gate has already admitted. Pure, so the
/// pin rule is testable without an engine.
///
/// An agent never archives a pinned thread (ADR 0312), and neither does
/// Archive all (ADR 0349). Those refuse a pinned target and leave a pinned
/// member open. The user's own Archive takes every member, and the drawer
/// confirms before it unpins.
fn plan_cascade(
    family: &[FamilyRow],
    target: Uuid,
    leave_pinned: bool,
) -> Result<CascadePlan, ArchiveRejection> {
    let left_pinned: Vec<Uuid> = if leave_pinned {
        family
            .iter()
            .filter(|r| r.is_saved)
            .map(|r| r.thread_id)
            .collect()
    } else {
        Vec::new()
    };
    if left_pinned.contains(&target) {
        return Err(pinned_rejection(target));
    }
    let keep = |tid: &Uuid| !left_pinned.contains(tid);
    Ok(CascadePlan {
        to_archive: not_yet_archived(family).into_iter().filter(keep).collect(),
        external_repo_pending: external_repo_pending(family)
            .into_iter()
            .filter(keep)
            .collect(),
        left_pinned,
    })
}

/// The cascade the Archive button and an agent's archive both run.
///
/// Inside one transaction the recursive CTE locks parent + all descendants
/// (`FOR UPDATE`), then `classify_family` runs the parent gate and the
/// per-descendant gate via `is_blocking`. The parent gate refuses Running, a
/// thread waiting on the user (ADR 0259), and an in-workspace CC with a pending
/// change. An already-archived parent is idempotent, not a rejection. If anyone
/// blocks, the lock is dropped and the caller gets
/// `409` with a structured body
/// (`reason: parent_not_archivable | parent_has_pending_changes | descendants_blocking`).
///
/// On success, the lock is committed (releasing FOR UPDATE — the actual event
/// emits go through EventBus, which runs its own per-emit transaction with
/// projection updates). External-repo CC descendants with pending changes get
/// their changes cleared via `emit_change_applied` first, then every member
/// goes through the per-thread cascade step:
///
///   1. The orphaned QuestionCard, if any, is cancel-stamped, so its answer
///      buttons render disabled on the archived thread. A turn can end without
///      an answer (`CodingAgentIdled`, `ResponseAborted`, `ResponseFailed`).
///      That leaves the row idle with the card dangling, and the gate admits
///      it. The card is picked while the family lock is held, so a question
///      asked after the lock drops is never cancelled (ADR 0259). A conflict
///      (the user answered first) is logged, not propagated.
///   2. `stop_agent(StopReason::Archive)` kills any *live* Claude Code
///      subprocess so it doesn't leak in `running_sessions` until engine
///      restart — but ONLY when `is_agent_running_for` is true. We do not
///      run `stop_agent`'s no-live fallback (`end_stale_waiting_session`),
///      which performs heavy, blocking git teardown (auto-commit + branch
///      diff + propose) per descendant
///      inside this request; on a stale-waiting CC family that serialized
///      teardown is what made a big archive take ~60s, time the client out,
///      and provoke a duplicate-cascade retry. The `ThreadArchived` emit
///      below settles the projection on its own (the gate refuses Running
///      and WaitingForUserAnswer), and the async worktree-cleanup worker
///      GCs the worktree on its own schedule.
///      Best-effort — errors are logged at warning level, not propagated,
///      because we still want the `ThreadArchived` emit to land. Any live
///      session reached here was already in the post-idle window (the
///      cascade gate rejects `Running`) and doesn't need the 2s
///      terminal-event wait the legacy single-thread handler used for
///      actively-working CC.
///   3. `ThreadArchived` emit. The bus refuses it on a member that parked
///      after the lock dropped, and that member is skipped rather than
///      failing the call.
///
/// Then, when the root still owed its parent a card, the parent gets the
/// canceled card (ADR 0252, ADR 0254).
///
/// A change claim holding the TARGET's session refuses the whole call first,
/// before anything is locked or emitted. The claim is an apply or an in-session
/// Discard, and the `409` names which. A member whose stop a claim refuses later,
/// or one that parked after the lock dropped, stays unarchived and is reported.
///
/// Already-archived rows are skipped, so nothing is emitted twice. The caller
/// has already weighed who may ask; this decides only whether the family can go.
pub(crate) async fn archive_family(
    engine: &std::sync::Arc<LucidosEngine>,
    thread_uuid: Uuid,
    actor: Option<MessageOrigin>,
    pinned: PinnedMembers,
) -> Result<ArchiveOutcome, ArchiveRejection> {
    // Before the transaction, so a refused target changes nothing. A
    // single-thread archive is exactly this case, and must not answer 200.
    if let Some(holder) = engine.change_claim_holder(thread_uuid).await {
        return Err(claimed_target_rejection(holder));
    }

    let mut tx = engine.pool().begin().await.map_err(internal_json)?;
    let family = load_family(&mut tx, thread_uuid)
        .await
        .map_err(internal_json)?;

    if let FamilyDecision::Reject { status, mut body } =
        classify_family(&family, thread_uuid, FamilyVerb::Archive)
    {
        // Release the FOR UPDATE lock before bouncing.
        let _ = tx.rollback().await;
        body["message"] = gate_refusal_message(&body, thread_uuid).into();
        return Err((status, axum::Json(body)));
    }
    let target_was_archived = family
        .iter()
        .any(|r| r.thread_id == thread_uuid && r.archive_state_enum() == ArchiveState::Archived);
    let leave_pinned = pinned == PinnedMembers::LeaveOpen
        || actor.as_ref().is_some_and(|a| a.mode() == ActorMode::Agent);
    let plan = match plan_cascade(&family, thread_uuid, leave_pinned) {
        Ok(plan) => plan,
        Err(rejection) => {
            let _ = tx.rollback().await;
            return Err(rejection);
        }
    };
    let CascadePlan {
        to_archive,
        external_repo_pending,
        left_pinned,
    } = plan;

    // Under the lock no member can park: a new question's projection write
    // waits on it. So every question still pending here is an orphan. The loop
    // cancels exactly these, never one a member asks after the lock drops.
    let mut orphaned_questions = HashMap::new();
    for tid in &to_archive {
        if let Some(tool_use_id) = lookup_pending_question_tool_use_id(engine.pool(), *tid).await {
            orphaned_questions.insert(*tid, tool_use_id);
        }
    }

    // Commit the FOR UPDATE lock first; emits go through EventBus, each in
    // its own transaction with projection updates. The race window between
    // commit and the per-thread emit loop is narrow and benign — at worst a
    // descendant transitions to Running before its archive lands, and we
    // still archive it (we've already decided based on the locked snapshot).
    //
    // Partial-cascade failure mode: if a per-descendant emit below fails
    // mid-loop (EventBus broadcast error, database hiccup), prior emits in
    // this cascade are already committed (each EventBus.emit runs its own
    // transaction) and the family is left half-archived. The handler returns
    // 500 to the caller. Re-invocation is safe and idempotent — descendants
    // whose `ThreadArchived` already landed are detected as
    // `archive_state = 'archived'` by the next call's `classify_archive_decision`
    // pass and skipped (see the `to_archive` filter in that function). The
    // frontend currently surfaces this as a generic 500; the user can
    // re-click Archive after a refresh.
    tx.commit().await.map_err(internal_json)?;

    // External-repo CC carve-out: clear each pending change via
    // ChangeApplied (no merge, no commits) before the ThreadArchived emit.
    // Mirrors the legacy single-thread handler's behaviour for the same
    // case; without this the change row would dangle in `pending` after
    // the thread is gone from the inbox. See `is_blocking` doc for why
    // these threads bypass the blocking predicate.
    let mut broadcast_changes = false;
    for tid in &external_repo_pending {
        let pending = engine
            .changes()
            .pending_for_thread(*tid)
            .await
            .map_err(internal_json)?;
        for change in pending {
            engine
                .emit_change_applied(
                    *tid,
                    change.id,
                    false,
                    false,
                    Vec::new(),
                    change.thread_title.clone(),
                    actor.clone(),
                    None,
                    None,
                )
                .await;
            broadcast_changes = true;
        }
    }
    if broadcast_changes {
        engine.broadcast_changes_updated().await;
    }

    let mut archived = Vec::with_capacity(to_archive.len());
    let mut skipped: Vec<serde_json::Value> = left_pinned
        .iter()
        .map(|tid| skipped_member(*tid, THREAD_PINNED, &pinned_message(*tid)))
        .collect();
    for tid in &to_archive {
        // Cancel-stamp the orphaned QuestionCard, if any, so its answer
        // buttons render disabled instead of dangling clickable on the
        // archived thread. See the doc comment above for how a card is
        // orphaned while its thread sits idle.
        if let Some(tool_use_id) = orphaned_questions.remove(tid) {
            if let AnswerResult::Conflict(msg) = answer_pending_question(
                engine,
                *tid,
                tool_use_id,
                AnswerKind::Canceled,
                actor.clone(),
            )
            .await
            {
                log!(
                    "[API] archive_thread: orphaned question on {}: {}",
                    tid,
                    msg
                );
            }
        }

        // Stop any *live* Claude Code subprocess so it doesn't leak in
        // `running_sessions` until engine restart. We deliberately gate on a
        // live session and do NOT fall through to `stop_agent`'s no-live
        // path: that path runs `end_stale_waiting_session`, which auto-commits
        // the worktree, recomputes the branch diff, and tries to propose a
        // change. That is heavy, blocking git I/O.
        // The archive loop runs this per descendant, serialized inside the one
        // HTTP request, so a stale-waiting CC family (the post-restart shape,
        // no live subprocess) took ~1s × N to archive — a big 8-track family
        // ≈60s, which timed the client out, left each child visibly stuck
        // until its slow settle landed, and provoked a retry that ran two
        // cascades over the same worktrees concurrently.
        //
        // For archive that teardown is both wrong-intent (we're discarding the
        // thread, not proposing its work) and unnecessary: the `ThreadArchived`
        // emit below settles the projection on its own (the cascade gate
        // already rejected Running and WaitingForUserAnswer), and the async
        // worktree-cleanup worker GCs the worktree on its own schedule (Tier 0
        // reclaims merged/clean
        // worktrees after a short grace; it needs no in-memory session). So
        // archiving a non-live thread is just the cheap `ThreadArchived` emit.
        //
        // Best-effort — logged on failure, not propagated, because we still
        // want the `ThreadArchived` emit to land. The cascade gate already
        // rejected status=Running, so any live session reached here was in the
        // post-idle window; no need for the legacy 2s STOP_FALLOUT_TIMEOUT_MS
        // wait (that synchronizes with actively-working CC, which can't be
        // here). `StopReason::Archive` suppresses the otherwise-spurious
        // `ResponseCanceled` (the `ThreadArchived` emit below is the
        // terminator).
        if engine.is_agent_running_for(*tid).await {
            if let Err(e) = engine
                .stop_agent(
                    crate::engine::claude_code::StopReason::Archive,
                    Some(*tid),
                    actor.clone(),
                )
                .await
            {
                // An apply or discard holding the session refuses the stop.
                // Archiving anyway would leave it and its live session running
                // on an archived thread, so the member stays unarchived.
                let message = e.to_string();
                if let Some(reason) = crate::engine::claude_code::claim_refusal_reason(&message) {
                    log!("[API] archive_thread: {} not archived: {}", tid, message);
                    skipped.push(skipped_member(*tid, reason, &message));
                    continue;
                }
                log!("[API] Failed to end Claude Code session on archive: {}", e);
            }
        }

        let emitted = engine
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: *tid,
                event: crate::engine::thread_events::ThreadEvent::ThreadArchived,
                meta: crate::engine::thread_events::EventMeta::with_actor(actor.clone()),
            })
            .await;
        match emitted {
            Ok(_) => archived.push(*tid),
            // The bus refused: the member parked after the lock dropped.
            Err(e) if e.downcast_ref::<LifecycleViolation>().is_some() => {
                log!("[API] archive_thread: {} not archived: {}", tid, e);
                skipped.push(skipped_member(*tid, "not_archivable", LEFT_OPEN_MESSAGE));
            }
            Err(e) => return Err(internal_json(e)),
        }
    }

    // A target already stored archived sits in Current only while a count says
    // something under it is live. The gate just found nothing live, so recount:
    // a drifted count then lets the thread go to Archive now (ADR 0378).
    if target_was_archived {
        engine
            .event_bus
            .recount_family_counts(thread_uuid)
            .await
            .map_err(internal_json)?;
    }

    // Archiving a child that still owes its parent a card settles it: a
    // stopped child (ADR 0252) or a waiting one (ADR 0254). Only the thread
    // the caller archived: a descendant caught in this cascade owes its parent
    // nothing, because that parent is archived too.
    if archived.contains(&thread_uuid) {
        engine
            .event_bus
            .settle_child(thread_uuid, crate::engine::event_bus::ChildSettle::Archived)
            .await;
    }

    Ok(ArchiveOutcome { archived, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::claude_code::{APPLY_IN_PROGRESS_MESSAGE, DISCARD_IN_PROGRESS_MESSAGE};
    use crate::engine::types::ChangeClaim;

    /// A member a change claim kept from being archived is reported with the
    /// holder's reason and words, so the caller can say which and why.
    #[test]
    fn a_skipped_member_carries_its_reason_and_message() {
        let tid = uuid::Uuid::new_v4();
        assert_eq!(
            skipped_member(tid, "discard_in_progress", DISCARD_IN_PROGRESS_MESSAGE),
            serde_json::json!({
                "thread_id": tid,
                "reason": "discard_in_progress",
                "message": DISCARD_IN_PROGRESS_MESSAGE,
            })
        );
        assert!(
            !LEFT_OPEN_MESSAGE.to_lowercase().contains("lifecycle"),
            "a member left open is told in plain words, not engine internals"
        );
    }

    fn idle_row(thread_id: Uuid, is_saved: bool) -> FamilyRow {
        FamilyRow {
            thread_id,
            is_coding_agent: false,
            status: ThreadStatus::Idle.as_str().to_string(),
            archive_state: "inbox".to_string(),
            coding_agent_proposed: false,
            coding_agent_is_external_repo: false,
            is_saved,
            is_home: false,
        }
    }

    /// An agent's archive refuses a pinned target, and leaves a pinned member
    /// open while it archives the rest. The user's Archive takes them all
    /// (ADR 0312).
    #[test]
    fn an_agents_cascade_leaves_pinned_threads_alone() {
        let (target, pinned_child, child) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let family = [
            idle_row(target, false),
            idle_row(pinned_child, true),
            idle_row(child, false),
        ];

        let plan = plan_cascade(&family, target, true).expect("an unpinned target proceeds");
        assert_eq!(plan.to_archive, vec![target, child]);
        assert_eq!(plan.left_pinned, vec![pinned_child]);

        let plan = plan_cascade(&family, target, false).expect("the user's archive proceeds");
        assert_eq!(plan.to_archive, vec![target, pinned_child, child]);
        assert!(plan.left_pinned.is_empty());

        let pinned_target = [idle_row(target, true), idle_row(child, false)];
        let Err((status, body)) = plan_cascade(&pinned_target, target, true) else {
            panic!("an agent must not archive a pinned target");
        };
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason"], THREAD_PINNED);
        assert!(plan_cascade(&pinned_target, target, false).is_ok());
    }

    /// The agent's early refusal for a waiting thread is the family gate's
    /// own body, plus the `message` every gate refusal now carries (ADR 0310).
    #[test]
    fn the_waiting_refusal_is_the_family_gates_body() {
        let thread_id = Uuid::new_v4();
        let row = super::super::family::FamilyRow {
            thread_id,
            is_coding_agent: true,
            status: ThreadStatus::WaitingForUserAnswer.as_str().to_string(),
            archive_state: "inbox".to_string(),
            coding_agent_proposed: false,
            coding_agent_is_external_repo: false,
            is_saved: false,
            is_home: false,
        };
        let FamilyDecision::Reject { status, mut body } =
            classify_family(&[row], thread_id, FamilyVerb::Archive)
        else {
            panic!("a waiting thread must be refused");
        };
        body["message"] = gate_refusal_message(&body, thread_id).into();

        let (early_status, early_body) = waiting_rejection(thread_id, false);
        assert_eq!(early_status, status);
        assert_eq!(early_body.0, body);
        assert!(rejection_text(&(early_status, early_body)).ends_with("(parent_not_archivable)"));
    }

    /// Each gate slug reads as its own sentence, and a running thread is not
    /// told it is waiting on the user.
    #[test]
    fn each_gate_refusal_says_what_to_do() {
        let id = Uuid::new_v4();
        let message = |body: serde_json::Value| gate_refusal_message(&body, id);
        assert!(message(serde_json::json!({
            "reason": "parent_not_archivable", "parent_status": "running",
        }))
        .contains("running"));
        assert!(message(serde_json::json!({
            "reason": "parent_not_archivable", "parent_status": "waiting_for_user_answer",
        }))
        .contains("Answer or stop it first"));
        assert!(
            message(serde_json::json!({ "reason": "parent_has_pending_changes" }))
                .contains("Apply or discard")
        );
        assert!(
            message(serde_json::json!({ "reason": "descendants_blocking" })).contains("sub-thread")
        );
    }

    /// A refused target fails the call as a 409 naming the holder, so a
    /// single-thread archive never answers 200 with nothing archived.
    #[test]
    fn a_claimed_target_is_a_conflict_that_names_the_holder() {
        for (holder, slug, message) in [
            (
                ChangeClaim::Apply,
                "apply_in_progress",
                APPLY_IN_PROGRESS_MESSAGE,
            ),
            (
                ChangeClaim::Discard,
                "discard_in_progress",
                DISCARD_IN_PROGRESS_MESSAGE,
            ),
        ] {
            let (status, body) = claimed_target_rejection(holder);
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(
                body.0,
                serde_json::json!({ "reason": slug, "message": message })
            );
        }
    }

    /// The target's claim is checked before the family transaction opens, so a
    /// refused target changes nothing: no lock, no cleared change, no emit.
    #[test]
    fn a_claimed_target_is_refused_before_anything_is_touched() {
        let src = crate::test_support::source_scan::read_production_source(
            &crate::test_support::source_scan::src_root().join("api/threads/archive.rs"),
        );
        let body = &src[src.find("async fn archive_family(").expect("the cascade")..];
        let check = body.find("change_claim_holder(").expect("the pre-check");
        let begin = body.find(".begin()").expect("the family transaction");
        assert!(
            check < begin,
            "the claim check must run before the transaction"
        );
    }
}
