//! Cascading thread-archive: lock the family inside a transaction, classify
//! whether everyone can be archived, then emit `ThreadArchived` (plus
//! pending-change cleanup for external-repo CC descendants).

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::api::AppState;

use super::extract_thread_uuid;

/// Row shape pulled by the recursive CTE in `load_family_for_archive` — the
/// subset of `thread_summaries` that feeds `is_blocking` + the parent's own
/// `resolve_actions` gate.
#[derive(Debug, Clone, sqlx::FromRow)]
struct FamilyRow {
    thread_id: Uuid,
    is_coding_agent: bool,
    status: String,
    archive_state: String,
    coding_agent_proposed: bool,
    coding_agent_is_external_repo: bool,
}

impl FamilyRow {
    fn thread_type(&self) -> crate::engine::thread_lifecycle::ThreadType {
        if self.is_coding_agent {
            crate::engine::thread_lifecycle::ThreadType::CodingAgent
        } else {
            crate::engine::thread_lifecycle::ThreadType::Chat
        }
    }
    fn status_enum(&self) -> crate::engine::thread_lifecycle::ThreadStatus {
        crate::engine::thread_lifecycle::ThreadStatus::parse(&self.status)
    }
    fn archive_state_enum(&self) -> crate::engine::thread_lifecycle::ArchiveState {
        crate::engine::thread_lifecycle::ArchiveState::parse(&self.archive_state)
    }
}

/// Result of validating the family for cascading archive. Either everyone can
/// be archived (`Ok` carrying the list of non-already-archived `thread_id`s,
/// plus the external-repo CC subset that needs pending-change cleanup before
/// the `ThreadArchived` emit), or someone blocks (`Err` with the JSON body
/// the handler returns alongside 409).
enum ArchiveDecision {
    Proceed {
        /// All non-already-archived threads in family order — the cascade
        /// emits `ThreadArchived` for each.
        to_archive: Vec<Uuid>,
        /// Subset of `to_archive` that are external-repo CC threads with
        /// pending changes. Their changes get cleared via `ChangeApplied`
        /// before the `ThreadArchived` emit so the change row doesn't dangle.
        external_repo_pending: Vec<Uuid>,
    },
    Reject {
        status: StatusCode,
        body: serde_json::Value,
    },
}

/// Lock parent + every descendant inside a transaction so no state can flip
/// between validation and emit. The caller owns the transaction so the lock
/// can be released with `tx.commit()` once the cascade decision is made.
async fn load_family_for_archive(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_uuid: Uuid,
) -> Result<Vec<FamilyRow>, sqlx::Error> {
    sqlx::query_as::<_, FamilyRow>(
        "WITH RECURSIVE family AS (
            SELECT thread_id, parent_thread_id, is_coding_agent, status,
                   archive_state, coding_agent_proposed,
                   coding_agent_is_external_repo
            FROM thread_summaries
            WHERE thread_id = $1
            UNION ALL
            SELECT t.thread_id, t.parent_thread_id, t.is_coding_agent, t.status,
                   t.archive_state, t.coding_agent_proposed,
                   t.coding_agent_is_external_repo
            FROM thread_summaries t
            JOIN family f ON t.parent_thread_id = f.thread_id
        )
        SELECT thread_id, is_coding_agent, status, archive_state,
               coding_agent_proposed, coding_agent_is_external_repo
        FROM family
        FOR UPDATE",
    )
    .bind(thread_uuid)
    .fetch_all(&mut **tx)
    .await
}

/// Pure decision function — given a locked family snapshot and the target
/// `thread_uuid`, return either the cascade plan or a structured rejection.
/// Split out so tests can drive it without a live HTTP stack.
fn classify_archive_decision(family: &[FamilyRow], thread_uuid: Uuid) -> ArchiveDecision {
    use crate::engine::thread_lifecycle::{is_blocking, ArchiveState, ThreadStatus};

    let Some(parent_row) = family.iter().find(|r| r.thread_id == thread_uuid) else {
        return ArchiveDecision::Reject {
            status: StatusCode::NOT_FOUND,
            body: serde_json::json!({ "reason": "thread_not_found" }),
        };
    };

    // Parent gate: the parent is archivable unless one of:
    //   1. live work is running on it (Running) — reject `parent_not_archivable`
    //   2. in-workspace CC thread with a pending change — reject
    //      `parent_has_pending_changes`; user must Apply or Discard first.
    //      Aligns with `resolve_actions`, which already returns
    //      [Discard, Apply] (never Archive) in this state. External-repo CC
    //      is exempt — Apply can't merge into a foreign repo, so its only
    //      exit is Archive with the `external_repo_pending` cleanup path
    //      (`ChangeApplied` emitted before `ThreadArchived`).
    // An already-archived parent is NOT a rejection — archive is idempotent.
    // It falls through to Proceed; the `to_archive` filter below excludes
    // already-archived rows, so the response is a no-op success (`archived:
    // []`) or just any descendant that legitimately resurfaced to inbox.
    // Rejecting here instead produced a stuck Archive button: a frontend whose
    // `meta.section` had desynced to 'inbox' (a missed `ThreadArchived` SSE or
    // a failed archive HTTP response on a flaky PWA — backend at 'archived',
    // client at 'inbox') offered Archive, the 409 rolled the client's
    // optimistic flip back to 'inbox', and the button reappeared on every tap.
    // WaitingForUserAnswer is admitted — the per-thread cascade step below
    // stamps the dangling QuestionCard as Canceled. Descendants are
    // re-validated separately below with the authoritative locked state via
    // `is_blocking` (which already rejects in-workspace CC descendants with
    // pending changes).
    if parent_row.status_enum() == ThreadStatus::Running {
        return ArchiveDecision::Reject {
            status: StatusCode::CONFLICT,
            body: serde_json::json!({
                "reason": "parent_not_archivable",
                "parent_status": parent_row.status,
                "has_pending_changes": parent_row.coding_agent_proposed,
            }),
        };
    }
    if parent_row.is_coding_agent
        && parent_row.coding_agent_proposed
        && !parent_row.coding_agent_is_external_repo
    {
        return ArchiveDecision::Reject {
            status: StatusCode::CONFLICT,
            body: serde_json::json!({
                "reason": "parent_has_pending_changes",
            }),
        };
    }

    let blockers: Vec<&FamilyRow> = family
        .iter()
        .filter(|r| r.thread_id != thread_uuid)
        .filter(|r| {
            is_blocking(
                r.thread_type(),
                r.status_enum(),
                r.archive_state_enum(),
                r.coding_agent_proposed,
                r.coding_agent_is_external_repo,
            )
        })
        .collect();
    if !blockers.is_empty() {
        return ArchiveDecision::Reject {
            status: StatusCode::CONFLICT,
            body: serde_json::json!({
                "reason": "descendants_blocking",
                "blocking": blockers.iter().map(|r| serde_json::json!({
                    "thread_id": r.thread_id,
                    "status": r.status,
                    "has_pending_changes": r.coding_agent_proposed,
                })).collect::<Vec<_>>(),
            }),
        };
    }

    let to_archive: Vec<Uuid> = family
        .iter()
        .filter(|r| r.archive_state_enum() != ArchiveState::Archived)
        .map(|r| r.thread_id)
        .collect();
    let external_repo_pending: Vec<Uuid> = family
        .iter()
        .filter(|r| {
            r.archive_state_enum() != ArchiveState::Archived
                && r.is_coding_agent
                && r.coding_agent_is_external_repo
                && r.coding_agent_proposed
        })
        .map(|r| r.thread_id)
        .collect();

    ArchiveDecision::Proceed {
        to_archive,
        external_repo_pending,
    }
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

/// POST /api/v1/threads/archive — cascading archive of a thread + every descendant.
///
/// Inside one transaction the recursive CTE locks parent + all descendants
/// (`FOR UPDATE`), then `classify_archive_decision` runs the parent gate
/// (Running / in-workspace CC with pending change — an already-archived parent
/// is idempotent, not a rejection) and the per-descendant gate via
/// `is_blocking`. If anyone blocks, the lock is dropped and the caller gets
/// `409` with a structured body
/// (`reason: parent_not_archivable | parent_has_pending_changes | descendants_blocking`).
///
/// On success, the lock is committed (releasing FOR UPDATE — the actual event
/// emits go through EventBus, which runs its own per-emit transaction with
/// projection updates). External-repo CC descendants with pending changes get
/// their changes cleared via `emit_change_applied` first, then every member
/// goes through the per-thread cascade step:
///
///   1. `resolve_pending_question_as_canceled` cancel-stamps any orphaned
///      QuestionCard so its answer buttons render disabled rather than
///      dangling clickable on the archived thread. The cascade gate only
///      blocks `WaitingForUserAnswer` while the surrounding turn is *live* —
///      a question whose turn ended in `CodingAgentIdled` / `ResponseAborted`
///      / `ResponseFailed` without an answer leaves the row at `status=idle`
///      but the QuestionCard dangling, so we always cancel-stamp before
///      archiving. Fire-and-forget (returns void; conflicts are logged
///      inside the helper).
///   2. `stop_agent(StopReason::Archive)` kills any *live* Claude Code
///      subprocess so it doesn't leak in `running_sessions` until engine
///      restart — but ONLY when `is_agent_running_for` is true. We do not
///      run `stop_agent`'s no-live fallback (`end_stale_waiting_session`),
///      which performs heavy, blocking git teardown (auto-commit +
///      `worktree remove --force` + branch diff + propose) per descendant
///      inside this request; on a stale-waiting CC family that serialized
///      teardown is what made a big archive take ~60s, time the client out,
///      and provoke a duplicate-cascade retry. The `ThreadArchived` emit
///      below settles the projection on its own (Running is gated out;
///      WaitingForUserAnswer is cancel-stamped in step 1), and the async
///      worktree-cleanup worker GCs the worktree on its own schedule.
///      Best-effort — errors are logged at warning level, not propagated,
///      because we still want the `ThreadArchived` emit to land. Any live
///      session reached here was already in the post-idle window (the
///      cascade gate rejects `Running`) and doesn't need the 2s
///      terminal-event wait the legacy single-thread handler used for
///      actively-working CC.
///   3. `ThreadArchived` emit.
///
/// Already-archived rows are skipped — no duplicate emit. Response:
/// `{"archived": [<uuid>, ...]}`.
pub(in crate::api) async fn archive_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<axum::Json<serde_json::Value>, (StatusCode, axum::Json<serde_json::Value>)> {
    let thread_uuid = extract_thread_uuid(&request).map_err(|(s, m)| {
        (
            s,
            axum::Json(serde_json::json!({ "reason": "bad_request", "message": m })),
        )
    })?;
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;

    let mut tx = state.engine.pool().begin().await.map_err(internal_json)?;
    let family = load_family_for_archive(&mut tx, thread_uuid)
        .await
        .map_err(internal_json)?;

    let (to_archive, external_repo_pending) = match classify_archive_decision(&family, thread_uuid)
    {
        ArchiveDecision::Proceed {
            to_archive,
            external_repo_pending,
        } => (to_archive, external_repo_pending),
        ArchiveDecision::Reject { status, body } => {
            // Release the FOR UPDATE lock before bouncing.
            let _ = tx.rollback().await;
            return Err((status, axum::Json(body)));
        }
    };

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
        let pending = state
            .engine
            .changes()
            .pending_for_thread(*tid)
            .await
            .map_err(internal_json)?;
        for change in pending {
            state
                .engine
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
        state.engine.broadcast_changes_updated().await;
    }

    for tid in &to_archive {
        // Cancel-stamp any orphaned QuestionCard so its answer buttons render
        // disabled instead of dangling clickable on the archived thread.
        // See the doc comment above for why the cascade gate alone isn't
        // enough (it admits status=idle threads whose UserQuestionAsked has
        // no UserQuestionAnswered because the surrounding turn was
        // terminated without an answer).
        crate::engine::agent_question::resolve_pending_question_as_canceled(
            &state.engine,
            *tid,
            actor.clone(),
        )
        .await;

        // Stop any *live* Claude Code subprocess so it doesn't leak in
        // `running_sessions` until engine restart. We deliberately gate on a
        // live session and do NOT fall through to `stop_agent`'s no-live
        // path: that path runs `end_stale_waiting_session`, which auto-commits
        // the worktree, `git worktree remove --force`s it, recomputes the
        // branch diff, and tries to propose a change — heavy, blocking git I/O.
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
        // already rejected status=Running, and WaitingForUserAnswer was
        // cancel-stamped just above), and the async worktree-cleanup worker
        // GCs the worktree on its own schedule (Tier 0 reclaims merged/clean
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
        if state.engine.is_agent_running_for(*tid).await {
            if let Err(e) = state
                .engine
                .stop_agent(
                    crate::engine::claude_code::StopReason::Archive,
                    Some(*tid),
                    actor.clone(),
                )
                .await
            {
                log!("[API] Failed to end Claude Code session on archive: {}", e);
            }
        }

        state
            .engine
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: *tid,
                event: crate::engine::thread_events::ThreadEvent::ThreadArchived,
                meta: crate::engine::thread_events::EventMeta::with_actor(actor.clone()),
            })
            .await
            .map_err(internal_json)?;
    }

    Ok(axum::Json(serde_json::json!({ "archived": to_archive })))
}

#[cfg(test)]
#[path = "cascade_tests.rs"]
mod cascade_tests;
