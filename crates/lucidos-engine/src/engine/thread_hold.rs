//! *Hold*: a thread, and the sub-threads it reaches, start no turn by
//! themselves until the owner resumes it. Hold raises no card.
//!
//! - A Hold stops the running turn of each thread it reaches. It puts a
//!   `ThreadHoldPlaced` on each, and sets each pending change aside under its
//!   id. A sub-thread already on hold keeps its own hold.
//! - Resume lifts exactly the holds carrying that id, brings back exactly
//!   the changes set aside under it, and hands over what came due meanwhile.
//! - Waiting steps are recomputed, never replayed (ADR 0696).
//!
//! The hold folds from events (`core::store::thread_hold`). See
//! `docs/plans/2026-11-23-cancel-hold-and-the-card-rule.md`.

use std::sync::Arc;

use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, HoldLiftCause, MessageOrigin, ThreadEvent};
use crate::engine::LucidosEngine;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub use crate::core::store::thread_hold::{
    may_start_turn, thread_hold, thread_is_held, turn_hold, ThreadHold, TurnHold,
};

/// What a Hold or a Resume did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldChange {
    /// The call recorded new hold events.
    Recorded,
    /// The thread was already in the asked state. Nothing was recorded.
    Unchanged,
}

/// Why a Hold or a Resume did not happen. The routes answer `NotFound` with
/// 404, `Refused` with 400 and `Internal` with 500.
#[derive(Debug)]
pub enum HoldError {
    NotFound,
    /// The caller asked for something a hold does not allow.
    Refused(String),
    /// The database or the event bus failed.
    Internal(String),
}

impl HoldError {
    fn internal(error: impl std::fmt::Display) -> Self {
        Self::Internal(error.to_string())
    }
}

impl std::fmt::Display for HoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("thread not found"),
            Self::Refused(why) | Self::Internal(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for HoldError {}

/// The words a Resume on a reached sub-thread answers with.
pub const RESUME_ON_ROOT: &str =
    "This thread is on hold because a thread above it is. Resume that thread instead.";

/// One thread a Hold reaches: the root and every unarchived descendant.
#[derive(sqlx::FromRow)]
struct Reached {
    thread_id: Uuid,
    running: bool,
}

/// The thread and its unarchived descendants, root first.
async fn reach(pool: &PgPool, root: Uuid) -> Result<Vec<Reached>, sqlx::Error> {
    sqlx::query_as(
        "WITH RECURSIVE tree AS ( \
            SELECT $1::uuid AS thread_id \
            UNION \
            SELECT c.thread_id FROM thread_summaries c \
            JOIN tree t ON c.parent_thread_id = t.thread_id \
            WHERE c.archive_state IS DISTINCT FROM 'archived') \
         SELECT s.thread_id, s.status = 'running' AS running FROM thread_summaries s \
         JOIN tree USING (thread_id) ORDER BY s.depth, s.thread_id",
    )
    .bind(root)
    .fetch_all(pool)
    .await
}

/// Put `root` and its sub-threads on hold, as `actor`. `holder` is the thread
/// whose own agent asked, if any: its running turn ends by itself, so the
/// reply that says what it checkpointed is not cut. A repeat on a held root
/// keeps the first reason and finishes a placement an earlier call left
/// partial.
pub async fn place_hold(
    engine: &Arc<LucidosEngine>,
    root: Uuid,
    reason: &str,
    holder: Option<Uuid>,
    actor: Option<MessageOrigin>,
) -> Result<HoldChange, HoldError> {
    let pool = engine.pool();
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(HoldError::Refused("a hold needs a reason".into()));
    }
    let reached = reach(pool, root).await.map_err(HoldError::internal)?;
    if reached.first().map(|r| r.thread_id) != Some(root) {
        return Err(HoldError::NotFound);
    }
    let existing = thread_hold(pool, root).await.map_err(HoldError::internal)?;
    let (hold_id, hold_root, reason) = match &existing {
        Some(hold) => (hold.hold_id, hold.root_thread_id, hold.reason.as_str()),
        None => (Uuid::new_v4(), root, reason),
    };
    let mut change = HoldChange::Unchanged;
    for member in &reached {
        if thread_hold(pool, member.thread_id)
            .await
            .map_err(HoldError::internal)?
            .is_some()
        {
            continue;
        }
        change = HoldChange::Recorded;
        engine
            .event_bus
            .emit(BusEvent::Thread {
                thread_id: member.thread_id,
                event: ThreadEvent::ThreadHoldPlaced {
                    reason: reason.to_string(),
                    hold_id,
                    root_thread_id: hold_root,
                },
                meta: EventMeta::with_actor(actor.clone()),
            })
            .await
            .map_err(HoldError::internal)?;
        if member.running && Some(member.thread_id) != holder {
            engine
                .agent_host()
                .stop_turn_for_hold(engine, member.thread_id, actor.clone())
                .await;
        }
        set_aside_pending(engine, member.thread_id, hold_id, actor.clone()).await;
    }
    Ok(change)
}

/// Set each pending change of `thread_id` aside under `hold_id`. A change an
/// apply or a discard already works on stays where it is.
async fn set_aside_pending(
    engine: &LucidosEngine,
    thread_id: Uuid,
    hold_id: Uuid,
    actor: Option<MessageOrigin>,
) {
    let pending: Vec<Uuid> = match sqlx::query_scalar(
        "SELECT id FROM changes WHERE thread_id = $1 AND status = 'pending'",
    )
    .bind(thread_id)
    .fetch_all(engine.pool())
    .await
    {
        Ok(pending) => pending,
        Err(e) => {
            crate::log_legacy!("[Hold] Pending changes of {thread_id} unreadable: {e}");
            return;
        }
    };
    for change_id in pending {
        if let Err(e) = engine
            .set_aside_change(change_id, Some(hold_id), actor.clone())
            .await
        {
            crate::log_legacy!("[Hold] Change {change_id} of {thread_id} not set aside: {e}");
        }
    }
}

/// The reason a hold carries when a set-aside change placed it.
pub const SET_ASIDE_HOLD_REASON: &str = "This thread's change is set aside until you resume it.";

/// The hold a change set aside on `thread_id` names: the standing hold, or a
/// new one rooted on the thread. An archived thread gets none, since its work
/// is on no list. A set-aside change always names a hold on a live thread.
pub(crate) async fn hold_for_set_aside(
    bus: &EventBus,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<Option<Uuid>, BoxError> {
    let archived: bool = sqlx::query_scalar(
        "SELECT COALESCE(bool_or(archive_state = 'archived'), false) \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(bus.pool())
    .await?;
    if archived {
        return Ok(None);
    }
    if let Some(hold) = thread_hold(bus.pool(), thread_id).await? {
        return Ok(Some(hold.hold_id));
    }
    let hold_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadHoldPlaced {
            reason: SET_ASIDE_HOLD_REASON.to_string(),
            hold_id,
            root_thread_id: thread_id,
        },
        meta: EventMeta::with_actor(actor),
    })
    .await?;
    Ok(Some(hold_id))
}

/// Each unarchived thread with a change set aside by no hold goes on hold,
/// and the change names that hold, so Resume brings it back. `thread_id`
/// narrows the sweep to one thread. Boot runs it unnarrowed as the upgrade.
pub async fn hold_set_aside_changes(bus: &EventBus, thread_id: Option<Uuid>) {
    let rows: Vec<(Uuid, Uuid)> = match sqlx::query_as(
        "SELECT c.thread_id, c.id FROM changes c \
         JOIN thread_summaries s ON s.thread_id = c.thread_id \
         WHERE c.status = 'set_aside' \
           AND ($1::uuid IS NULL OR c.thread_id = $1) \
           AND s.archive_state IS DISTINCT FROM 'archived' \
           AND ( SELECT e.payload->>'hold_id' FROM events e \
                  WHERE e.event_type = 'ChangeSetAside' \
                    AND e.payload->>'change_id' = c.id::text \
                  ORDER BY e.sequence DESC LIMIT 1) IS NULL \
         ORDER BY c.thread_id",
    )
    .bind(thread_id)
    .fetch_all(bus.pool())
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            crate::log_legacy!("[Hold] Set-aside changes unreadable: {e}");
            return;
        }
    };
    for (thread_id, change_id) in rows {
        if let Err(e) = hold_one_set_aside_change(bus, thread_id, change_id).await {
            crate::log_legacy!("[Hold] Set-aside change {change_id} not held: {e}");
        }
    }
}

async fn hold_one_set_aside_change(
    bus: &EventBus,
    thread_id: Uuid,
    change_id: Uuid,
) -> Result<(), BoxError> {
    let Some(hold_id) = hold_for_set_aside(bus, thread_id, None).await? else {
        return Ok(());
    };
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeSetAside {
            change_id: change_id.to_string(),
            hold_id: Some(hold_id),
        },
        meta: EventMeta::with_actor(None),
    })
    .await?;
    Ok(())
}

/// Unarchive offers the thread's kept work again: each change it set aside
/// returns to pending. A thread still on hold keeps its changes set aside.
pub async fn bring_back_on_unarchive(
    engine: &LucidosEngine,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) {
    match thread_hold(engine.pool(), thread_id).await {
        Ok(None) => {}
        Ok(Some(_)) => return,
        Err(e) => {
            crate::log_legacy!("[Hold] Hold on unarchived {thread_id} unreadable: {e}");
            return;
        }
    }
    let changes: Vec<Uuid> = match sqlx::query_scalar(
        "SELECT id FROM changes WHERE thread_id = $1 AND status = 'set_aside'",
    )
    .bind(thread_id)
    .fetch_all(engine.pool())
    .await
    {
        Ok(changes) => changes,
        Err(e) => {
            crate::log_legacy!("[Hold] Changes of unarchived {thread_id} unreadable: {e}");
            return;
        }
    };
    for change_id in changes {
        if let Err(e) = engine.bring_back_change(change_id, actor.clone()).await {
            crate::log_legacy!("[Hold] Change {change_id} not brought back on unarchive: {e}");
        }
    }
}

/// Every thread whose standing hold is `hold_id`.
async fn holding(pool: &PgPool, hold_id: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
    let candidates: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT thread_id FROM events \
         WHERE (event_type = 'ThreadHoldPlaced' AND payload->>'hold_id' = $1::text) \
            OR (event_type = 'AutomationPaused' AND id = $1)",
    )
    .bind(hold_id)
    .fetch_all(pool)
    .await?;
    let mut held = Vec::new();
    for thread_id in candidates {
        if thread_hold(pool, thread_id)
            .await?
            .is_some_and(|hold| hold.hold_id == hold_id)
        {
            held.push(thread_id);
        }
    }
    Ok(held)
}

/// Resume `root`: lift its hold and the holds it placed on sub-threads,
/// bring back the changes it set aside, and hand over what came due. The
/// caller has already checked that `actor` may resume. A sub-thread a Hold
/// reached is resumed from the root, so it is refused here.
pub async fn resume_hold(
    engine: &Arc<LucidosEngine>,
    root: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<HoldChange, HoldError> {
    let pool = engine.pool();
    let Some(hold) = thread_hold(pool, root).await.map_err(HoldError::internal)? else {
        let reached = reach(pool, root).await.map_err(HoldError::internal)?;
        if reached.first().map(|r| r.thread_id) != Some(root) {
            return Err(HoldError::NotFound);
        }
        return Ok(HoldChange::Unchanged);
    };
    if hold.root_thread_id != root {
        return Err(HoldError::Refused(RESUME_ON_ROOT.into()));
    }
    let mut members = holding(pool, hold.hold_id)
        .await
        .map_err(HoldError::internal)?;
    // The root lifts last: if a lift fails, Resume on the root retries the rest.
    members.sort_by_key(|id| *id == root);
    let mut since = Vec::with_capacity(members.len());
    for thread_id in &members {
        let placed = thread_hold(pool, *thread_id)
            .await
            .map_err(HoldError::internal)?
            .map_or(hold.placed_sequence, |h| h.placed_sequence);
        lift(
            &engine.event_bus,
            *thread_id,
            hold.hold_id,
            HoldLiftCause::Resume,
            actor.clone(),
        )
        .await
        .map_err(HoldError::internal)?;
        since.push((*thread_id, placed));
    }
    bring_back_held_changes(engine, hold.hold_id, actor).await;
    for (thread_id, placed) in since {
        if let Err(e) = super::card_hold::hand_over_held_wakes(engine, thread_id, placed).await {
            crate::log_legacy!("[Hold] Wakes held on {thread_id} not handed over: {e}");
        }
    }
    Ok(HoldChange::Recorded)
}

/// Lift the hold standing on `thread_id` alone, because the owner archived
/// it. Its set-aside changes stay off every list with the archived thread.
pub async fn lift_hold_on_archive(
    bus: &EventBus,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<HoldChange, BoxError> {
    let Some(hold) = thread_hold(bus.pool(), thread_id).await? else {
        return Ok(HoldChange::Unchanged);
    };
    lift(bus, thread_id, hold.hold_id, HoldLiftCause::Archived, actor).await?;
    Ok(HoldChange::Recorded)
}

async fn lift(
    bus: &EventBus,
    thread_id: Uuid,
    hold_id: Uuid,
    cause: HoldLiftCause,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadHoldLifted { hold_id, cause },
        meta: EventMeta::with_actor(actor),
    })
    .await?;
    Ok(())
}

/// Bring back each change still set aside under `hold_id`.
async fn bring_back_held_changes(
    engine: &LucidosEngine,
    hold_id: Uuid,
    actor: Option<MessageOrigin>,
) {
    let changes: Vec<Uuid> = match sqlx::query_scalar(
        "SELECT c.id FROM changes c WHERE c.status = 'set_aside' AND ( \
           SELECT e.payload->>'hold_id' FROM events e WHERE e.event_type = 'ChangeSetAside' \
             AND e.payload->>'change_id' = c.id::text \
           ORDER BY e.sequence DESC LIMIT 1) = $1::text",
    )
    .bind(hold_id)
    .fetch_all(engine.pool())
    .await
    {
        Ok(changes) => changes,
        Err(e) => {
            crate::log_legacy!("[Hold] Changes held by {hold_id} unreadable: {e}");
            return;
        }
    };
    for change_id in changes {
        if let Err(e) = engine.bring_back_change(change_id, actor.clone()).await {
            crate::log_legacy!("[Hold] Change {change_id} not brought back: {e}");
        }
    }
}

/// Unread wait anchors after a durable hold boundary. Stop resumes, the card
/// hold and Resume share this reader. A coding agent's read receipt and a
/// chat turn's settings both mark an anchor read.
pub(crate) async fn unread_wait_reentries_since(
    pool: &PgPool,
    thread_id: Uuid,
    since: i64,
) -> Result<Vec<crate::engine::event_wait::WaitReentry>, sqlx::Error> {
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "WITH receipts AS MATERIALIZED ( \
           SELECT CASE r.event_type WHEN 'CodingAgentInputRead' THEN r.payload->>'input_event_id' \
                  ELSE r.payload->>'request_event_id' END AS read_event_id FROM events r \
            WHERE r.thread_id = $1 \
              AND r.event_type IN ('CodingAgentInputRead', 'ChatTurnSettingsResolved')) \
         SELECT p.id, p.payload->>'text' FROM events p \
         WHERE p.thread_id = $1 AND p.event_type = 'PromptInjected' \
           AND p.payload->'origin'->'reason'->>'kind' = $2 \
           AND p.sequence > $3 \
           AND NOT EXISTS ( \
             SELECT 1 FROM receipts WHERE receipts.read_event_id = p.id::text) \
         ORDER BY p.sequence",
    )
    .bind(thread_id)
    .bind(crate::engine::thread_events::EVENT_WAIT_REASON_KIND)
    .bind(since)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(anchor_event_id, text)| crate::engine::event_wait::WaitReentry {
                anchor_event_id,
                text: text.unwrap_or_default(),
            },
        )
        .collect())
}

impl LucidosEngine {
    /// Ask a coding agent to read the child reports filed while a hold or a
    /// card stood. A live session reads them in session. Otherwise the thread
    /// resumes, and its turn-gap note carries the reports.
    pub(crate) async fn request_report_read(
        &self,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), BoxError> {
        use crate::engine::continuation_reason::{
            REPORTS_REQUESTED_MESSAGE, REPORTS_REQUESTED_REASON,
        };
        if let Some(session) = self.live_session_info(thread_id).await {
            return session
                .msg_tx
                .send(crate::engine::AgentUserInput {
                    text: REPORTS_REQUESTED_MESSAGE.to_string(),
                    images: None,
                    origin_event_id: None,
                    kind: crate::engine::AgentInputKind::ReentryFromEngine,
                    completion: None,
                })
                .map_err(|_| "Session channel closed".into());
        }
        if crate::engine::thread_events::emit_continuation_requested_or_log(
            &self.event_bus,
            thread_id,
            REPORTS_REQUESTED_REASON,
            actor,
            "[Hold] ContinuationRequested",
        )
        .await
        {
            Ok(())
        } else {
            Err(format!("the {REPORTS_REQUESTED_REASON} resume could not be requested").into())
        }
    }
}

#[cfg(test)]
#[path = "thread_hold_tests.rs"]
mod tests;
