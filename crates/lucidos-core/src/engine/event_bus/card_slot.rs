//! The *card slot*: one open card per thread. The Validate phase consults it
//! under the per-thread append lock, so "is a card open?" cannot race.

use sqlx::PgConnection;
use uuid::Uuid;

use crate::engine::question_park_sql::open_card_exists;
use crate::engine::thread_events::ThreadEvent;

/// Validate refused an agent permission prompt because another card is open.
/// The prompt site waits for a free slot and emits again.
#[derive(Debug)]
pub struct CardSlotTaken;

impl std::fmt::Display for CardSlotTaken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("card_slot_taken")
    }
}

impl std::error::Error for CardSlotTaken {}

/// The `request_id` of an agent permission prompt request.
pub(super) fn prompt_request_id(event: &ThreadEvent) -> Option<&str> {
    match event {
        ThreadEvent::CodingAgentPermissionRequest { request_id, .. }
        | ThreadEvent::CommandPermissionRequested { request_id, .. }
        | ThreadEvent::McpPermissionRequested { request_id, .. } => Some(request_id),
        _ => None,
    }
}

/// The `request_id` an agent permission prompt resolution closes.
pub fn prompt_resolution_id(event: &ThreadEvent) -> Option<&str> {
    match event {
        ThreadEvent::CodingAgentPermissionResolved { request_id, .. }
        | ThreadEvent::CommandPermissionResolved { request_id, .. }
        | ThreadEvent::McpPermissionResolved { request_id, .. } => Some(request_id),
        _ => None,
    }
}

/// What a `UserQuestionAsked` on a thread becomes.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CardAdmission {
    /// The slot is free, or the card re-raises one already posted: record it.
    Post,
    /// Record it as a *queued card*, for the card queue to post later.
    Queue(QueueReason),
    /// A release of a queued card that cannot post now: the slot is taken, a
    /// turn still runs, or the card was already posted or retired. Record
    /// nothing.
    Drop,
}

/// Why a card queued.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum QueueReason {
    /// Another card is open.
    CardOpen,
    /// Lucidos raised the card while a turn runs, and no call holds for it.
    /// It posts when the turn ends.
    TurnRunning,
}

/// Decide what a `UserQuestionAsked` carrying `tool_use_id` becomes.
/// `waits_for_turn_end` says the card may not post while a turn runs.
///
/// A card already queued under the same `tool_use_id` makes this emit a
/// *release*. A release never queues a second copy.
pub(super) async fn admit_question(
    tx: &mut PgConnection,
    thread_id: Uuid,
    tool_use_id: &str,
    waits_for_turn_end: bool,
) -> Result<CardAdmission, sqlx::Error> {
    if card_event_exists(tx, thread_id, "UserQuestionQueued", tool_use_id).await? {
        let settled = card_event_exists(tx, thread_id, "UserQuestionAsked", tool_use_id).await?
            || card_event_exists(tx, thread_id, "UserQuestionAnswered", tool_use_id).await?;
        if settled
            || open_card_exists(tx, thread_id).await?
            || (waits_for_turn_end && turn_runs(tx, thread_id).await?)
        {
            return Ok(CardAdmission::Drop);
        }
        return Ok(CardAdmission::Post);
    }
    if card_event_exists(tx, thread_id, "UserQuestionAsked", tool_use_id).await? {
        return Ok(CardAdmission::Post);
    }
    if open_card_exists(tx, thread_id).await? {
        return Ok(CardAdmission::Queue(QueueReason::CardOpen));
    }
    if waits_for_turn_end && turn_runs(tx, thread_id).await? {
        return Ok(CardAdmission::Queue(QueueReason::TurnRunning));
    }
    Ok(CardAdmission::Post)
}

/// Whether a turn runs on the thread, read from its summary row inside the
/// append transaction. The terminal's projection writes the row in its own
/// transaction, so the card queue's release after it sees the turn ended.
async fn turn_runs(tx: &mut PgConnection, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(tx)
            .await?;
    Ok(status.as_deref() == Some(crate::engine::thread_lifecycle::ThreadStatus::Running.as_str()))
}

/// Whether the card `tool_use_id` names is queued and never posted. The owner
/// has not seen such a card, so no answer of theirs may land on it.
pub(crate) async fn is_queued_unposted(
    tx: &mut PgConnection,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<bool, sqlx::Error> {
    Ok(
        card_event_exists(tx, thread_id, "UserQuestionQueued", tool_use_id).await?
            && !card_event_exists(tx, thread_id, "UserQuestionAsked", tool_use_id).await?,
    )
}

async fn card_event_exists(
    conn: &mut PgConnection,
    thread_id: Uuid,
    event_type: &str,
    tool_use_id: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM events WHERE aggregate_id = $1 \
         AND event_type = $2 AND payload->>'tool_use_id' = $3)",
    )
    .bind(thread_id.to_string())
    .bind(event_type)
    .bind(tool_use_id)
    .fetch_one(conn)
    .await
}
