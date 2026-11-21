//! The *consumption claim*: a human message is either taken back or consumed,
//! never both. The event bus decides it under the thread append lock, so a
//! removal and a delivery cannot both commit (ADR 0323 for coding agents).

use uuid::Uuid;

use crate::core::EventRow;
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::thread_lifecycle::{ArchiveState, LifecycleViolation, ThreadType};

/// What one thread event claims about a human message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageClaim {
    /// Records the message itself.
    Records(Uuid),
    /// Delivers the message to its recipient.
    Consumes(Uuid),
    /// Takes the message back.
    Removes(Uuid),
}

/// The refusal reason when a removed message would be recorded or consumed.
pub(crate) const REMOVED_REASON: &str = "the user took this message back";
/// The refusal reason when a consumed message would be taken back.
pub(crate) const CONSUMED_REASON: &str = "this message has already been delivered";

/// The claim `event` makes, if any. A message claims under its own event id,
/// so an event persisted under a fresh id names no message.
pub(crate) fn claim_of(event: &ThreadEvent, meta: &EventMeta) -> Option<MessageClaim> {
    match event {
        ThreadEvent::MessageReceived { .. } => meta.event_id.map(MessageClaim::Records),
        ThreadEvent::PromptInjected {
            injected_message_id,
            ..
        } => injected_message_id.map(MessageClaim::Consumes),
        ThreadEvent::ChatTurnSettingsResolved { .. } => {
            meta.request_event_id.map(MessageClaim::Consumes)
        }
        ThreadEvent::UserQuestionAnswered { .. } | ThreadEvent::UserOnlyMessagePosted { .. } => {
            meta.event_id.map(MessageClaim::Consumes)
        }
        ThreadEvent::QueuedMessageRemoved {
            removed_message_id, ..
        } => Some(MessageClaim::Removes(*removed_message_id)),
        _ => None,
    }
}

/// Did the user take `message` back on `thread_id`?
pub async fn was_removed<'e, E>(
    executor: E,
    thread_id: Uuid,
    message: Uuid,
) -> Result<bool, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events WHERE thread_id = $1 \
           AND event_type = 'QueuedMessageRemoved' \
           AND payload->>'removed_message_id' = $2::text)",
    )
    .bind(thread_id)
    .bind(message)
    .fetch_one(executor)
    .await
}

/// One definition for both the claim guard and Stop's candidate selection.
const CONSUMED_PREDICATE: &str = "e.thread_id = candidate.thread_id AND ( \
    (e.event_type = 'PromptInjected' AND e.payload->>'injected_message_id' = candidate.id::text) \
 OR (e.event_type = 'ChatTurnSettingsResolved' AND e.payload->>'request_event_id' = candidate.id::text) \
 OR (e.event_type IN ('UserQuestionAnswered', 'UserOnlyMessagePosted') AND e.id = candidate.id))";

/// Delivery receipts for separately persisted held messages.
pub const HELD_MESSAGE_RECEIPT_TYPES: &[&str] = &[
    "PromptInjected",
    "ChatTurnSettingsResolved",
    "CodingAgentInputRead",
];

/// Identify a receipt for a separately persisted message in the event stream.
pub fn held_message_receipt_target(event: &ThreadEvent, meta: &EventMeta) -> Option<Uuid> {
    if let ThreadEvent::CodingAgentInputRead { input_event_id, .. } = event {
        return Some(*input_event_id);
    }
    if !HELD_MESSAGE_RECEIPT_TYPES.contains(&event.event_type()) {
        return None;
    }
    match claim_of(event, meta) {
        Some(MessageClaim::Consumes(id)) => Some(id),
        _ => None,
    }
}

/// Whether a stored row needs a receipt before it can enter model context.
pub fn is_permission_held(event: &EventRow) -> bool {
    event.event_type == "MessageReceived"
        && event.payload["queued_for_permission"].as_bool() == Some(true)
}

pub(crate) const HELD_MESSAGE_SQL: &str =
    "candidate.event_type = 'MessageReceived' AND candidate.payload @> '{\"queued_for_permission\":true}'";

/// Use the chat append guard's predicate and the coding agent's read receipt.
/// The first receipt supplies ordering, while the original row supplies content.
pub(crate) fn model_delivery_join() -> String {
    format!(
        "LEFT JOIN LATERAL (
             SELECT e.sequence, e.created FROM events e
             WHERE ({HELD_MESSAGE_SQL})
               AND ({CONSUMED_PREDICATE} OR (
                   e.thread_id = candidate.thread_id AND e.event_type = 'CodingAgentInputRead'
                   AND e.payload->>'input_event_id' = candidate.id::text))
               AND e.sequence > candidate.sequence
               AND NOT EXISTS (
                   SELECT 1 FROM events removed
                   WHERE removed.thread_id = candidate.thread_id
                     AND removed.event_type = 'QueuedMessageRemoved'
                     AND removed.payload->>'removed_message_id' = candidate.id::text)
             ORDER BY e.sequence LIMIT 1
         ) delivery ON TRUE"
    )
}

pub(crate) fn model_event_visible() -> String {
    format!("(NOT ({HELD_MESSAGE_SQL}) OR delivery.sequence IS NOT NULL)")
}

/// Resolve a live or replayed receipt to its original message exactly once.
pub async fn held_message_at_receipt(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    message: Uuid,
    receipt_sequence: i64,
) -> Result<Option<EventRow>, sqlx::Error> {
    let delivery = model_delivery_join();
    sqlx::query_as(&format!(
        "SELECT candidate.* FROM events candidate {delivery}
         WHERE candidate.id = $1 AND candidate.thread_id = $2 AND delivery.sequence = $3"
    ))
    .bind(message)
    .bind(thread_id)
    .bind(receipt_sequence)
    .fetch_optional(pool)
    .await
}

/// Was `message` delivered? A turn starter counts once its turn stamps
/// `ChatTurnSettingsResolved`. A removal before that stamp refuses the stamp,
/// so the turn never runs.
async fn was_consumed<'e, E>(
    executor: E,
    thread_id: Uuid,
    message: Uuid,
) -> Result<bool, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_scalar(&format!(
        "WITH candidate AS (SELECT $1::uuid AS thread_id, $2::uuid AS id) \
         SELECT EXISTS (SELECT 1 FROM events e CROSS JOIN candidate WHERE {CONSUMED_PREDICATE})"
    ))
    .bind(thread_id)
    .bind(message)
    .fetch_one(executor)
    .await
}

/// Refuse an event whose claim contradicts one already committed. Runs inside
/// the emit's transaction, after the thread append lock.
pub(crate) async fn check(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
    event: &ThreadEvent,
    meta: &EventMeta,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(claim) = claim_of(event, meta) else {
        return Ok(());
    };
    let (reason, (thread_type, current_section)) = match claim {
        MessageClaim::Records(message) | MessageClaim::Consumes(message) => {
            if !was_removed(&mut **tx, thread_id, message).await? {
                return Ok(());
            }
            (REMOVED_REASON, thread_kind(tx, thread_id).await?)
        }
        MessageClaim::Removes(message) => {
            // A coding agent's withdrawal is already confirmed by its holder.
            let kind = thread_kind(tx, thread_id).await?;
            if kind.0 != ThreadType::Chat || !was_consumed(&mut **tx, thread_id, message).await? {
                return Ok(());
            }
            (CONSUMED_REASON, kind)
        }
    };
    Err(Box::new(LifecycleViolation {
        event_type: event.event_type().to_string(),
        thread_type,
        current_section,
        reason: reason.to_string(),
    }))
}

async fn thread_kind(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
) -> Result<(ThreadType, ArchiveState), sqlx::Error> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT source, archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row.map_or(
        (ThreadType::Chat, ArchiveState::Inbox),
        |(source, section)| {
            (
                ThreadType::from_source(&source),
                ArchiveState::parse(&section),
            )
        },
    ))
}

fn refusal_reason<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a str> {
    error
        .downcast_ref::<LifecycleViolation>()
        .map(|violation| violation.reason.as_str())
}

/// The bus refused an emit because the user took its message back.
pub fn is_removed_refusal(error: &(dyn std::error::Error + 'static)) -> bool {
    refusal_reason(error) == Some(REMOVED_REASON)
}

/// The bus refused a removal because an agent already has the message.
pub(crate) fn is_consumed_refusal(error: &(dyn std::error::Error + 'static)) -> bool {
    refusal_reason(error) == Some(CONSUMED_REASON)
}

/// A taken-back input, including content a second device may not have loaded.
#[derive(serde::Serialize)]
pub struct TakenBackMessage {
    pub thread_id: Uuid,
    pub id: Uuid,
    pub text: String,
    pub image_hashes: Vec<String>,
    pub compose: crate::core::store::ComposeReport,
}

/// Partial failures are visible without preventing the live turn from stopping.
#[derive(Default, serde::Serialize)]
pub struct TakeBackReport {
    pub taken_back_messages: Vec<TakenBackMessage>,
    pub failed_take_back_ids: Vec<Uuid>,
}

/// The result of taking back one input, shared by Stop and the removal endpoint.
pub enum TakeBackOutcome {
    Removed,
    AlreadyRemoved,
    Consumed,
}

pub async fn take_back_message(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    message_id: Uuid,
    actor: Option<crate::engine::thread_events::MessageOrigin>,
) -> Result<TakeBackOutcome, Box<dyn std::error::Error + Send + Sync>> {
    take_back(bus, thread_id, message_id, actor, false).await
}

async fn take_back(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    message_id: Uuid,
    actor: Option<crate::engine::thread_events::MessageOrigin>,
    restore_to_compose: bool,
) -> Result<TakeBackOutcome, Box<dyn std::error::Error + Send + Sync>> {
    if was_removed(bus.pool(), thread_id, message_id).await? {
        return Ok(TakeBackOutcome::AlreadyRemoved);
    }
    match bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: ThreadEvent::QueuedMessageRemoved {
                removed_message_id: message_id,
                restore_to_compose,
                compose: None,
            },
            meta: EventMeta {
                // All take-back callers compete for the same event identity.
                // A duplicate insert rolls back before projecting or broadcasting.
                event_id: Some(Uuid::new_v5(
                    &thread_id,
                    format!("take-back:{message_id}").as_bytes(),
                )),
                channel: Some(crate::engine::thread_events::EventChannel::Chat),
                actor,
                ..EventMeta::NONE
            },
        })
        .await
    {
        Ok(_) => Ok(TakeBackOutcome::Removed),
        Err(error) if is_consumed_refusal(&*error) => Ok(TakeBackOutcome::Consumed),
        Err(error) => {
            if was_removed(bus.pool(), thread_id, message_id).await? {
                Ok(TakeBackOutcome::AlreadyRemoved)
            } else {
                Err(error)
            }
        }
    }
}

/// Snapshot the durable acceptance set before canceling. Acceptance survives
/// queue handoff, unlike its projection row, so recorded injections remain
/// discoverable. The claim guard arbitrates any consumption race after this read.
/// With no thread id, take back inputs on every Lucidos Agent thread.
/// Only a device Stop withdraws human inputs; automation cancels live work alone.
pub async fn take_back_unconsumed(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Option<Uuid>,
    actor: Option<crate::engine::thread_events::MessageOrigin>,
) -> Result<TakeBackReport, Box<dyn std::error::Error + Send + Sync>> {
    use crate::engine::thread_events::{ActorMode, MessageOrigin};
    let eligible = match &actor {
        Some(MessageOrigin::Device { .. }) => true,
        Some(MessageOrigin::UnattestedCaller {
            mode: ActorMode::Human,
            claimed_device_id: Some(id),
            ..
        }) => crate::core::DeviceStore::is_registered(bus.pool(), id).await?,
        _ => false,
    };
    if !eligible {
        return Ok(TakeBackReport::default());
    }
    let rows: Vec<(Uuid, Uuid, serde_json::Value)> = sqlx::query_as(&format!(
        "WITH candidate AS ( \
           SELECT (payload->'data'->'request'->>'event_id')::uuid AS id, \
                  (payload->'data'->'request'->>'thread_id')::uuid AS thread_id, \
                  payload->'data'->'request' AS request, sequence, \
                  COALESCE(payload->'data'->>'entry_id', \
                    payload->'data'->'receipt'->>'queue_entry_id') AS entry_id \
             FROM events WHERE event_type IN ('OrdinarySubmissionAccepted', 'DraftSubmissionAccepted') \
              AND payload->'data'->'request'->>'type' = 'agent-chat' \
              AND payload->'data'->'request'->>'mode' = 'human' \
              AND ($1::uuid IS NULL OR payload->'data'->'request'->>'thread_id' = $1::text) \
         ) SELECT id, thread_id, request FROM candidate \
          WHERE NOT EXISTS (SELECT 1 FROM events e WHERE {CONSUMED_PREDICATE}) \
            AND NOT EXISTS (SELECT 1 FROM events e WHERE e.thread_id = candidate.thread_id \
                 AND e.event_type = ANY($3::text[]) \
                 AND e.payload->>'request_event_id' = candidate.id::text) \
            AND NOT EXISTS (SELECT 1 FROM events e WHERE e.thread_id = candidate.thread_id \
                 AND e.event_type = 'QueuedMessageRemoved' \
                 AND e.payload->>'removed_message_id' = candidate.id::text) \
            AND NOT EXISTS (SELECT 1 FROM events e WHERE e.event_type = 'ThreadQueueDropped' \
                 AND e.payload->'data'->>'entry_id' = candidate.entry_id) \
            AND NOT EXISTS (SELECT 1 FROM thread_summaries t WHERE t.thread_id = candidate.thread_id \
                 AND t.source = $2) \
            AND COALESCE(request->>'use_coding_agent', 'false') <> 'true' \
          ORDER BY sequence"
    ))
    .bind(thread_id)
    .bind(crate::engine::thread_lifecycle::CODING_AGENT_SOURCE)
    .bind(ThreadEvent::TERMINATOR_EVENT_TYPES)
    .fetch_all(bus.pool()).await?;

    let mut report = TakeBackReport::default();
    for (id, thread_id, request) in rows {
        let content = match serde_json::from_value::<AcceptedContent>(request) {
            Ok(content) => content,
            Err(error) => {
                crate::log_legacy!("[Stop] Could not read accepted input {}: {}", id, error);
                report.failed_take_back_ids.push(id);
                continue;
            }
        };
        match take_back(bus, thread_id, id, actor.clone(), true).await {
            Ok(TakeBackOutcome::Removed) => report.taken_back_messages.push(TakenBackMessage {
                thread_id,
                id,
                text: content.message,
                image_hashes: content.image_hashes,
                compose: {
                    let value: serde_json::Value = sqlx::query_scalar(
                        "SELECT payload->'compose' FROM events WHERE thread_id=$1 AND event_type='QueuedMessageRemoved' AND payload->>'removed_message_id'=$2"
                    ).bind(thread_id).bind(id.to_string()).fetch_one(bus.pool()).await?;
                    serde_json::from_value(value)?
                },
            }),
            Ok(TakeBackOutcome::AlreadyRemoved | TakeBackOutcome::Consumed) => {}
            Err(error) => {
                crate::log_legacy!("[Stop] Could not take back input {}: {}", id, error);
                report.failed_take_back_ids.push(id);
            }
        }
    }
    Ok(report)
}

/// Restore accepted content while holding the removal's append transaction.
/// The stored acceptance is authoritative even before MessageReceived records.
pub(crate) async fn restore_accepted_input(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    thread_id: Uuid,
    message_id: Uuid,
) -> Result<crate::core::store::ComposeReport, Box<dyn std::error::Error + Send + Sync>> {
    let request: serde_json::Value = sqlx::query_scalar(
        "SELECT payload->'data'->'request' FROM events \
         WHERE event_type IN ('OrdinarySubmissionAccepted','DraftSubmissionAccepted') \
           AND payload->'data'->'request'->>'thread_id'=$1 \
           AND payload->'data'->'request'->>'event_id'=$2",
    )
    .bind(thread_id.to_string())
    .bind(message_id.to_string())
    .fetch_one(&mut **tx)
    .await?;
    let content: AcceptedContent = serde_json::from_value(request)?;
    crate::core::store::restore_compose_slot(tx, thread_id, &content.message, &content.image_hashes)
        .await
}

#[derive(serde::Deserialize)]
struct AcceptedContent {
    message: String,
    #[serde(default)]
    image_hashes: Vec<String>,
}

#[cfg(test)]
#[path = "consumption_claim_tests.rs"]
mod tests;
