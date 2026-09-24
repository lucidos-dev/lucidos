//! Which queued follow-ups a thread still owes an answer for, asked of the
//! event store rather than of an in-memory channel.
//!
//! A follow-up typed mid-turn is persisted as `MessageReceived` before it
//! reaches the running turn's injection channel, so the message was never at
//! risk. Only the answer to "has the loop consumed it" lived in memory, as a
//! channel remainder that died with the process. Three already-written markers
//! answer it instead: a `UserPromptInjected.injected_message_id` (consumed), a
//! `QueuedMessageRemoved.removed_message_id` (retracted), and any event's
//! `request_event_id` (a turn picked it up).
//!
//! [`STRANDED_QUEUED_MESSAGES_SQL`] is asked where the channel is GONE, which
//! is the resume (`chat/rerun.rs`). The turn tail asks nothing: its drain is
//! total while the process lives. The channel keeps delivery and the wakeup,
//! since a row cannot un-park a tool blocked in `bash_output(wait_secs=…)`.
//!
//! Scope, rejected alternatives and the ordering decision:
//! `docs/plans/2026-09-21-a-queued-follow-up-survives-the-restart.md`.

use std::path::Path;

use uuid::Uuid;

use crate::api::ChatImage;
use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::engine::{InjectedPrompt, InjectedPromptKind};

/// The undrained queued messages on a thread, oldest first.
///
/// `$1` thread id as text, `$2` the originating event of the turn that owns the
/// drain, `$3` the window end sequence. A `const` so tests drive the exact
/// predicate the callers run, like `THREAD_START_EVENTS_SQL`.
///
/// Each arm is a separate reason a message is not owed:
///
/// * `sequence > $2` keeps the window behind the interrupted turn. No lower
///   bound would replay messages stranded weeks ago into a live conversation.
/// * `sequence <= $3` stops at the reader's fence ([`window_end_sequence`]).
/// * no `UserPromptInjected` naming it: the loop took it. Announcing a
///   recovered message writes one, which is what makes a second pass a no-op.
/// * no `QueuedMessageRemoved` naming it: a retraction beats a recovery.
/// * no event stamped with it as `request_event_id`: a turn has it, running or
///   finished. A TERMINATOR-only test let Continue take a live turn's message.
///
/// It never asks whether the message was spoken: a turn branching on a live
/// call answers one question two ways (ADR 0149). A caller's utterance is a
/// `SpokenMessageReceived`, which the event-type arm already excludes.
pub(crate) const STRANDED_QUEUED_MESSAGES_SQL: &str = "\
SELECT e.id, e.payload \
  FROM events e \
 WHERE e.aggregate = 'thread' \
   AND e.aggregate_id = $1 \
   AND e.event_type = 'MessageReceived' \
   AND e.sequence > (SELECT sequence FROM events WHERE id = $2) \
   AND e.sequence <= $3 \
   AND NOT EXISTS ( \
         SELECT 1 FROM events u \
          WHERE u.aggregate = 'thread' AND u.aggregate_id = $1 \
            AND u.event_type = 'UserPromptInjected' \
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

/// The highest sequence on `thread_id` right now, or 0 for an empty thread.
///
/// The reader samples this BEFORE the fence that closes its window, and passes
/// it as `$3`. Sampling inside the query would be a tautology: one snapshot
/// always satisfies `sequence <= MAX(sequence)`.
///
/// The resume's fence is the anchor emit. A message after it finds no live
/// handle, so the chat API gives it a turn of its own. Such a message belongs
/// to that turn, and recovering it here would answer it twice.
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

/// Run [`STRANDED_QUEUED_MESSAGES_SQL`] and rebuild each row as the
/// `InjectedPrompt` the live path would have carried, attachments included.
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

/// The images a recovered message was sent with, read back from the blob store.
///
/// The live injection path carries the bytes in memory. A recovery without them
/// would hand the model a different turn than the user sent, and an image-only
/// follow-up is the case that decides it: the text alone says nothing. A hash
/// whose blob is gone is skipped, since a missing attachment must not cost the
/// whole message.
pub(super) fn images_for_row(
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

/// Rebuild one `MessageReceived` row as a `UserText` injection.
///
/// `mode` falls back to `Human` for the same reason the event's own serde
/// default does: rows written before the field existed were all user-typed.
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

/// Decode the wire spelling of `ActorMode`. Paired with `ActorMode::as_str`,
/// which writes it.
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
