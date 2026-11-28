use axum::http::StatusCode;
use uuid::Uuid;

use std::collections::HashMap;

use crate::engine::agent_question::{
    answer_pending_question, lookup_pending_question_tool_use_id, AnswerResult,
};
use crate::engine::thread_events::{ActorMode, AnswerKind, MessageOrigin};
use crate::engine::thread_lifecycle::{ArchiveState, Blocker, LifecycleViolation, ThreadStatus};
use crate::engine::LucidosEngine;

use crate::engine::archive_family_rules::{
    classify_family, external_repo_pending, load_family, not_yet_archived, with_blocker,
    FamilyDecision, FamilyRow, FamilyVerb, HOME_THREAD,
};

fn internal_json<E: std::fmt::Display>(e: E) -> (StatusCode, axum::Json<serde_json::Value>) {
    log_legacy!("[API] archive_thread: {}", e);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(serde_json::json!({
            "reason": "internal_error",
            "message": e.to_string(),
        })),
    )
}

/// INVARIANT: What a member that changed state after the lock dropped is told. The raw
/// lifecycle error names engine internals, so it goes to the log instead.
const LEFT_OPEN_MESSAGE: &str = "It changed state after the archive began, so it was left open";

fn skipped_member(thread_id: uuid::Uuid, reason: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "thread_id": thread_id, "reason": reason, "message": message })
}

fn claimed_target_rejection(
    holder: crate::engine::types::ChangeClaim,
) -> (StatusCode, axum::Json<serde_json::Value>) {
    (
        StatusCode::CONFLICT,
        axum::Json(serde_json::json!({
            "reason": crate::engine::session_claim_refusal::claim_refusal_slug(holder),
            "message": crate::engine::session_claim_refusal::claim_refusal_message(holder),
        })),
    )
}

pub type ArchiveRejection = (StatusCode, axum::Json<serde_json::Value>);

#[derive(Default)]
pub struct ArchiveOutcome {
    pub archived: Vec<Uuid>,
    pub skipped: Vec<serde_json::Value>,
    pub(crate) summary_versions: HashMap<Uuid, i64>,
}

impl ArchiveOutcome {
    pub fn into_body(self) -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({
            "archived": self.archived,
            "skipped": self.skipped,
            "summary_versions": self.summary_versions,
        }))
    }
}

pub fn rejection_text((_, body): &ArchiveRejection) -> String {
    let field = |key: &str| body.get(key).and_then(|v| v.as_str()).unwrap_or_default();
    format!("{} ({})", field("message"), field("reason"))
}

pub fn waiting_rejection(thread_id: Uuid, has_pending_changes: bool) -> ArchiveRejection {
    let mut body = with_blocker(
        crate::engine::archive_family_rules::parent_blocked_body(
            FamilyVerb::Archive,
            ThreadStatus::WaitingForUserAnswer.as_str(),
            has_pending_changes,
        ),
        Blocker::Question,
    );
    body["message"] = gate_refusal_message(&body, thread_id).into();
    (StatusCode::CONFLICT, axum::Json(body))
}

pub const THREAD_PINNED: &str = "thread_pinned";

fn pinned_message(thread_id: Uuid) -> String {
    format!(
        "Thread {thread_id} is pinned by the user, so an agent cannot archive it. \
         Leave it open, or ask the user to archive it."
    )
}

pub fn pinned_rejection(thread_id: Uuid) -> ArchiveRejection {
    (
        StatusCode::CONFLICT,
        axum::Json(serde_json::json!({
            "reason": THREAD_PINNED,
            "message": pinned_message(thread_id),
        })),
    )
}

pub fn home_thread_rejection(thread_id: Uuid) -> ArchiveRejection {
    let mut body = with_blocker(serde_json::json!({ "reason": HOME_THREAD }), Blocker::Home);
    body["message"] = gate_refusal_message(&body, thread_id).into();
    (StatusCode::CONFLICT, axum::Json(body))
}

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
        Some("parent_holds_held_proposal") => format!(
            "Thread {thread_id} is waiting on an event with unproposed work. Stop waiting first, \
             so its change is proposed."
        ),
        Some("descendants_blocking") => format!(
            "A sub-thread of {thread_id} is running, waiting on the user, or holds a change, \
             so the family cannot be archived yet."
        ),
        _ => format!("Thread {thread_id} cannot be archived right now."),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinnedMembers {
    ByActor,
    LeaveOpen,
}

struct CascadePlan {
    to_archive: Vec<Uuid>,
    external_repo_pending: Vec<Uuid>,
    left_pinned: Vec<Uuid>,
}

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

async fn notify_root_done(
    engine: &std::sync::Arc<LucidosEngine>,
    thread_id: Uuid,
    actor: Option<&MessageOrigin>,
) {
    use crate::engine::thread_users::{notify, UserNotice};
    let snapshot = crate::gateway_users::current();
    let notified = match crate::engine::thread_family::family_root(engine.pool(), thread_id).await {
        Ok(root) if root == thread_id => {
            let by = actor.and_then(|a| snapshot.user_of_origin(a));
            notify(engine, root, UserNotice::Completed { by }, &snapshot).await
        }
        Ok(_) => Ok(()),
        Err(e) => Err(e.into()),
    };
    if let Err(e) = notified {
        log_legacy!(
            "[API] archive_thread: could not tell {}'s users it is done: {}",
            thread_id,
            e
        );
    }
}

pub async fn archive_family(
    engine: &std::sync::Arc<LucidosEngine>,
    thread_uuid: Uuid,
    actor: Option<MessageOrigin>,
    pinned: PinnedMembers,
) -> Result<ArchiveOutcome, ArchiveRejection> {
    archive_family_scoped(engine, thread_uuid, actor, pinned, None).await
}

pub async fn archive_completed_family(
    engine: &std::sync::Arc<LucidosEngine>,
    thread_uuid: Uuid,
    actor: MessageOrigin,
    parent: Uuid,
    confirmed: &[Uuid],
) -> Result<ArchiveOutcome, ArchiveRejection> {
    archive_family_scoped(
        engine,
        thread_uuid,
        Some(actor),
        PinnedMembers::LeaveOpen,
        Some((parent, confirmed)),
    )
    .await
}

async fn archive_family_scoped(
    engine: &std::sync::Arc<LucidosEngine>,
    thread_uuid: Uuid,
    actor: Option<MessageOrigin>,
    pinned: PinnedMembers,
    completed: Option<(Uuid, &[Uuid])>,
) -> Result<ArchiveOutcome, ArchiveRejection> {
    // INVARIANT: Before the transaction, so a refused target changes nothing. A
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
        // INVARIANT: Release the FOR UPDATE lock before bouncing.
        let _ = tx.rollback().await;
        body["message"] = gate_refusal_message(&body, thread_uuid).into();
        return Err((status, axum::Json(body)));
    }
    let target_was_archived = family
        .iter()
        .any(|r| r.thread_id == thread_uuid && r.archive_state_enum() == ArchiveState::Archived);
    let leave_pinned = pinned == PinnedMembers::LeaveOpen
        || actor.as_ref().is_some_and(|a| a.mode() == ActorMode::Agent);
    let mut plan = match plan_cascade(&family, thread_uuid, leave_pinned) {
        Ok(plan) => plan,
        Err(rejection) => {
            let _ = tx.rollback().await;
            return Err(rejection);
        }
    };
    if let Some((parent, confirmed)) = completed {
        use crate::engine::triage_facts::{descendant_ids, load, TriageScope};
        use crate::engine::triage_policy::refuse_completed;
        // INVARIANT: Read after locking the family: changes to its projected state must
        // wait until this selection finishes. Recheck scope after detach too.
        let selection = async {
            let scope = descendant_ids(engine.pool(), parent).await?;
            let ids: Vec<_> = family
                .iter()
                .map(|r| r.thread_id)
                .filter(|id| confirmed.contains(id) && scope.contains(id))
                .collect();
            load(engine.pool(), TriageScope::CompletedIds(&ids)).await
        }
        .await;
        let rows = match selection {
            Ok(rows) => rows,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(internal_json(error));
            }
        };
        let eligible = |id: &Uuid| {
            confirmed.contains(id)
                && rows
                    .iter()
                    .find(|r| r.facts.thread_id == *id)
                    .is_some_and(|r| refuse_completed(Some(r)).is_none())
        };
        if !eligible(&thread_uuid) {
            let _ = tx.rollback().await;
            return Err((
                StatusCode::CONFLICT,
                axum::Json(serde_json::json!({
                    "reason": "not_completed", "message": "The sub-thread is no longer completed and safe to archive."
                })),
            ));
        }
        plan.to_archive.retain(eligible);
        plan.external_repo_pending.retain(eligible);
    }
    let CascadePlan {
        to_archive,
        external_repo_pending,
        left_pinned,
    } = plan;

    // INVARIANT: Under the lock no member can park: a new question's projection write
    // waits on it. So every question still pending here is an orphan. The loop
    // cancels exactly these, never one a member asks after the lock drops.
    let mut orphaned_questions = HashMap::new();
    for tid in &to_archive {
        if let Some(tool_use_id) = lookup_pending_question_tool_use_id(engine.pool(), *tid).await {
            orphaned_questions.insert(*tid, tool_use_id);
        }
    }

    // INVARIANT: Commit the FOR UPDATE lock first; emits go through EventBus, each in
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

    // INVARIANT: External-repo CC carve-out: clear each pending change via
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
            // INVARIANT: An apply already running for this change announces it instead.
            let Some(claim) = engine
                .claim_pending_change(change.id)
                .await
                .map_err(internal_json)?
            else {
                continue;
            };
            engine
                .emit_change_applied(
                    &claim,
                    *tid,
                    false,
                    false,
                    Vec::new(),
                    change.thread_title.clone(),
                    actor.clone(),
                    None,
                )
                .await
                .map_err(internal_json)?;
            broadcast_changes = true;
        }
    }
    if broadcast_changes {
        engine.broadcast_changes_updated().await;
    }

    let mut archived = Vec::with_capacity(to_archive.len());
    let mut summary_versions = HashMap::with_capacity(to_archive.len());
    let mut skipped: Vec<serde_json::Value> = left_pinned
        .iter()
        .map(|tid| skipped_member(*tid, THREAD_PINNED, &pinned_message(*tid)))
        .collect();
    for tid in &to_archive {
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
                log_legacy!(
                    "[API] archive_thread: orphaned question on {}: {}",
                    tid,
                    msg
                );
            }
        }

        // INVARIANT: Stop any *live* Claude Code subprocess so it doesn't leak in
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
                .agent_host()
                .stop_agent(
                    engine,
                    crate::engine::types::StopReason::Archive,
                    Some(*tid),
                    actor.clone(),
                )
                .await
            {
                // INVARIANT: An apply or discard holding the session refuses the stop.
                // Archiving anyway would leave it and its live session running
                // on an archived thread, so the member stays unarchived.
                let message = e.to_string();
                if let Some(reason) =
                    crate::engine::session_claim_refusal::claim_refusal_reason(&message)
                {
                    log_legacy!("[API] archive_thread: {} not archived: {}", tid, message);
                    skipped.push(skipped_member(*tid, reason, &message));
                    continue;
                }
                log_legacy!("[API] Failed to end Claude Code session on archive: {}", e);
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
            Ok(result) => {
                archived.push(*tid);
                if let Some(version) = result.and_then(|r| r.summary_version) {
                    summary_versions.insert(*tid, version);
                }
                if matches!(actor, Some(MessageOrigin::Device { .. })) {
                    if let Err(e) = crate::engine::thread_hold::lift_hold_on_archive(
                        &engine.event_bus,
                        *tid,
                        actor.clone(),
                    )
                    .await
                    {
                        log_legacy!("[API] archive_thread: hold on {} not lifted: {}", tid, e);
                    }
                }
            }
            Err(e) if e.downcast_ref::<LifecycleViolation>().is_some() => {
                log_legacy!("[API] archive_thread: {} not archived: {}", tid, e);
                skipped.push(skipped_member(*tid, "not_archivable", LEFT_OPEN_MESSAGE));
            }
            Err(e) => return Err(internal_json(e)),
        }
    }

    if archived.contains(&thread_uuid) {
        notify_root_done(engine, thread_uuid, actor.as_ref()).await;
    }

    if target_was_archived {
        engine
            .event_bus
            .recount_family_counts(thread_uuid)
            .await
            .map_err(internal_json)?;
    }

    if archived.contains(&thread_uuid) {
        engine
            .event_bus
            .settle_child(thread_uuid, crate::engine::event_bus::ChildSettle::Archived)
            .await;
    }

    Ok(ArchiveOutcome {
        archived,
        skipped,
        summary_versions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::session_claim_refusal::{
        APPLY_IN_PROGRESS_MESSAGE, DISCARD_IN_PROGRESS_MESSAGE,
    };
    use crate::engine::thread_lifecycle::ChangeStateKind;
    use crate::engine::types::ChangeClaim;

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
            coding_agent_change_state: ChangeStateKind::None,
            coding_agent_is_external_repo: false,
            holds_live_wait: false,
            is_saved,
            is_home: false,
        }
    }

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

    #[test]
    fn the_waiting_refusal_is_the_family_gates_body() {
        let thread_id = Uuid::new_v4();
        let row = crate::engine::archive_family_rules::FamilyRow {
            thread_id,
            is_coding_agent: true,
            status: ThreadStatus::WaitingForUserAnswer.as_str().to_string(),
            archive_state: "inbox".to_string(),
            coding_agent_change_state: ChangeStateKind::None,
            coding_agent_is_external_repo: false,
            holds_live_wait: false,
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
            message(serde_json::json!({ "reason": "parent_holds_held_proposal" }))
                .contains("Stop waiting first")
        );
        assert!(
            message(serde_json::json!({ "reason": "descendants_blocking" })).contains("sub-thread")
        );
    }

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

    mod guard_tests {
        #[test]
        fn a_claimed_target_is_refused_before_anything_is_touched() {
            let src = crate::test_support::source_scan::read_production_source(
                &crate::test_support::source_scan::engine_source("engine/archive_family.rs"),
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
}
