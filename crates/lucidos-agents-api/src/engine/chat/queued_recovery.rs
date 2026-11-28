use std::path::Path;

use uuid::Uuid;

use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::engine::types::ChatImage;
use crate::engine::{InjectedPrompt, InjectedPromptKind};

pub(crate) const STRANDED_QUEUED_MESSAGES_SQL: &str = "\
SELECT e.id, e.payload \
  FROM events e \
 WHERE e.aggregate = 'thread' \
   AND e.aggregate_id = $1 \
   AND e.event_type = 'MessageReceived' \
   AND e.payload->>'queued_for_permission' IS DISTINCT FROM 'true' \
   AND e.sequence > (SELECT sequence FROM events WHERE id = $2) \
   AND e.sequence <= $3 \
   AND NOT EXISTS ( \
         SELECT 1 FROM events u \
          WHERE u.aggregate = 'thread' AND u.aggregate_id = $1 \
            AND u.event_type = 'PromptInjected' \
            AND u.payload->>'injected_message_id' = e.id::text) \
   AND NOT EXISTS ( \
         SELECT 1 FROM events r \
          WHERE r.aggregate = 'thread' AND r.aggregate_id = $1 \
            AND r.event_type = 'QueuedMessageRemoved' \
            AND r.payload->>'removed_message_id' = e.id::text) \
   AND NOT EXISTS ( \
         SELECT 1 FROM events t \
          WHERE t.aggregate = 'thread' AND t.aggregate_id = $1 \
            AND t.payload->>'request_event_id' = e.id::text) \
 ORDER BY e.sequence ASC";

pub(crate) async fn window_end_sequence(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(sequence), 0) FROM events \
          WHERE aggregate = 'thread' AND aggregate_id = $1",
    )
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
}

pub(crate) async fn undrained_user_messages(
    pool: &sqlx::PgPool,
    workspace: &Path,
    thread_id: Uuid,
    after_event_id: Uuid,
    window_end: i64,
) -> Result<Vec<InjectedPrompt>, sqlx::Error> {
    let rows: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(STRANDED_QUEUED_MESSAGES_SQL)
        .bind(thread_id.to_string())
        .bind(after_event_id)
        .bind(window_end)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|(id, payload)| prompt_from_message_row(workspace, id, &payload))
        .collect())
}

pub(crate) fn images_for_row(
    workspace: &Path,
    payload: &serde_json::Value,
) -> Option<Vec<ChatImage>> {
    let images: Vec<ChatImage> = payload
        .get("user_image_hashes")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .filter_map(|hash| crate::core::blobs::read_blob_as_base64(workspace, hash))
        .map(|(base64, mime_type)| ChatImage { base64, mime_type })
        .collect();
    (!images.is_empty()).then_some(images)
}

fn prompt_from_message_row(
    workspace: &Path,
    id: Uuid,
    payload: &serde_json::Value,
) -> InjectedPrompt {
    InjectedPrompt {
        text: payload
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        event_id: Some(id),
        mode: payload
            .get("mode")
            .and_then(|v| v.as_str())
            .and_then(actor_mode_from_wire)
            .unwrap_or(ActorMode::Human),
        spawning_event_id: payload
            .get("spawning_event_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok()),
        images: images_for_row(workspace, payload),
        origin: payload
            .get("origin")
            .and_then(|v| serde_json::from_value::<MessageOrigin>(v.clone()).ok()),
        kind: InjectedPromptKind::UserText,
    }
}

fn actor_mode_from_wire(s: &str) -> Option<ActorMode> {
    match s {
        "human" => Some(ActorMode::Human),
        "agent" => Some(ActorMode::Agent),
        "engine" => Some(ActorMode::Engine),
        _ => None,
    }
}

#[cfg(test)]
#[path = "queued_recovery_tests.rs"]
mod tests;
