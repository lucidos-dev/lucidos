//! The *read request*: a thread's own agent asks the user to read its latest
//! reply (ADR 0409).
//!
//! The `request_read` tool and `lucidos request-read` record the same
//! `ThreadReadRequested`, with the agent thread as its actor. The drawer
//! records `ThreadReplySeen` once the user has seen the reply. A human message
//! or an archive also clears the request, in the projection, with no event of
//! its own.

use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::chat::agent_archive::agent_thread_actor;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// What the agent reads back after asking, from the tool and the CLI alike.
pub(crate) const READ_REQUESTED_ACK: &str = "Recorded. Once this turn ends, the thread is listed under Review until the user has read your reply.";

/// Record that `thread_id`'s own agent asked the user to read its latest
/// reply. The `request_read` tool's actor: the agent thread itself.
pub(crate) async fn record_agent_read_request(
    bus: &EventBus,
    thread_id: Uuid,
) -> Result<(), BoxError> {
    record_read_request(bus, thread_id, agent_thread_actor(thread_id)).await
}

/// Record a read request on `thread_id`, by `actor`.
pub(crate) async fn record_read_request(
    bus: &EventBus,
    thread_id: Uuid,
    actor: MessageOrigin,
) -> Result<(), BoxError> {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadReadRequested,
        meta: EventMeta::with_actor(Some(actor)),
    })
    .await?;
    Ok(())
}

/// What a sighting found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReplySeen {
    /// A request was pending, and `ThreadReplySeen` now clears it.
    Cleared,
    /// No request was pending, so nothing was recorded.
    NothingPending,
    /// The thread changed after the client saw it, so the request it saw may
    /// not be the pending one. Nothing was recorded; the client asks again.
    Stale,
    /// The workspace holds no such thread.
    UnknownThread,
}

/// Record that the user saw the reply a pending read request points at, as of
/// the thread's `seen_version`. Records nothing when no request is pending, so
/// a second device seeing the same reply adds no event.
pub(crate) async fn record_reply_seen(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    seen_version: i64,
    actor: MessageOrigin,
) -> Result<ReplySeen, BoxError> {
    let pending: Option<bool> =
        sqlx::query_scalar("SELECT read_requested FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await?;
    match pending {
        None => Ok(ReplySeen::UnknownThread),
        Some(false) => Ok(ReplySeen::NothingPending),
        Some(true) => {
            let emitted = bus
                .emit(BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ThreadReplySeen { seen_version },
                    meta: EventMeta::with_actor(Some(actor)),
                })
                .await?;
            Ok(if emitted.is_some() {
                ReplySeen::Cleared
            } else {
                ReplySeen::Stale
            })
        }
    }
}

#[cfg(test)]
#[path = "read_request_tests.rs"]
mod tests;
