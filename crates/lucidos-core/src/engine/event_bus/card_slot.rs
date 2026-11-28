//! INVARIANT: The *card slot*: one open card per thread. The Validate phase consults it
//! under the per-thread append lock, so "is a card open?" cannot race.

use sqlx::PgConnection;
use uuid::Uuid;

use crate::engine::question_park_sql::open_card_exists;
use crate::engine::thread_events::ThreadEvent;

#[derive(Debug)]
pub struct CardSlotTaken;

impl std::fmt::Display for CardSlotTaken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("card_slot_taken")
    }
}

impl std::error::Error for CardSlotTaken {}

pub(super) fn prompt_request_id(event: &ThreadEvent) -> Option<&str> {
    match event {
        ThreadEvent::CodingAgentPermissionRequest { request_id, .. }
        | ThreadEvent::CommandPermissionRequested { request_id, .. }
        | ThreadEvent::McpPermissionRequested { request_id, .. } => Some(request_id),
        _ => None,
    }
}

pub(super) fn can_open_card(event: &ThreadEvent) -> bool {
    matches!(event, ThreadEvent::UserQuestionAsked { .. }) || prompt_request_id(event).is_some()
}

pub fn prompt_resolution_id(event: &ThreadEvent) -> Option<&str> {
    match event {
        ThreadEvent::CodingAgentPermissionResolved { request_id, .. }
        | ThreadEvent::CommandPermissionResolved { request_id, .. }
        | ThreadEvent::McpPermissionResolved { request_id, .. } => Some(request_id),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum CardAdmission {
    Post,
    Queue(QueueReason),
    Drop,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum QueueReason {
    CardOpen,
    TurnRunning,
}

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

async fn turn_runs(tx: &mut PgConnection, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(tx)
            .await?;
    Ok(status.as_deref() == Some(crate::engine::thread_lifecycle::ThreadStatus::Running.as_str()))
}

/// INVARIANT: Whether the card `tool_use_id` names is queued and never posted. The owner
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

/// First persisted post for this logical card. The caller holds the source
/// thread append lock. A retry can retain its existing source-event behavior
/// while the opening keeps the first post's identity.
pub(super) async fn first_opening(
    tx: &mut PgConnection,
    thread_id: Uuid,
    event_id: Uuid,
    event: &ThreadEvent,
    resolved_in_batch: &[String],
) -> Result<Option<super::SubThreadCardKind>, sqlx::Error> {
    use super::SubThreadCardKind::{PermissionRequest, Question};

    let (key, id, kind) = match event {
        ThreadEvent::UserQuestionAsked {
            tool_use_id,
            permission_request,
            ..
        } => (
            "tool_use_id",
            tool_use_id.as_str(),
            if permission_request.is_some() {
                PermissionRequest
            } else {
                Question
            },
        ),
        _ => {
            let Some(id) =
                prompt_request_id(event).filter(|id| !resolved_in_batch.iter().any(|r| r == id))
            else {
                return Ok(None);
            };
            ("request_id", id, PermissionRequest)
        }
    };
    let first: Uuid = sqlx::query_scalar(
        "SELECT id FROM events WHERE aggregate = 'thread' AND aggregate_id = $1
         AND event_type = $2 AND payload->>$3 = $4 ORDER BY sequence LIMIT 1",
    )
    .bind(thread_id.to_string())
    .bind(event.event_type())
    .bind(key)
    .bind(id)
    .fetch_one(tx)
    .await?;
    Ok((first == event_id).then_some(kind))
}
