//! The *read request*: a thread's own agent asks the user to read its latest
//! reply (ADR 0409), as the yes half of the turn's *read decision* (ADR 0417).
//!
//! The `request_read` tool and `lucidos request-read` record the same
//! decision, with the agent thread as its actor: `ThreadReadRequested` for a
//! yes, `ThreadReadNotRequested` for a no. The drawer records
//! `ThreadReplySeen` once the user has seen the reply. A human message or an
//! archive also clears a request, in the projection, with no event of its
//! own. A change that already lists the thread clears and blocks one, by a
//! database trigger (ADR 0421). A no clears nothing.

use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::chat::agent_archive::agent_thread_actor;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub(crate) use crate::llm::tools::READ_ARG;

/// The yes or no every agent turn ends with, about its reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadDecision {
    /// The reply is worth reading: a *read request*.
    Requested,
    /// The reply needs no reading.
    NotRequested,
}

impl ReadDecision {
    pub(crate) fn from_read(read: bool) -> Self {
        if read {
            Self::Requested
        } else {
            Self::NotRequested
        }
    }

    /// Read `request_read`'s arguments. A missing or non-boolean `read` is
    /// an error the model reads back, never a default.
    pub(crate) fn from_tool_args(args: &serde_json::Value) -> Result<Self, String> {
        args[READ_ARG]
            .as_bool()
            .map(Self::from_read)
            .ok_or_else(|| {
                format!(
                    "Error: request_read needs `{READ_ARG}`: true when your reply is worth \
                 reading, false when it is not."
                )
            })
    }

    fn event(self) -> ThreadEvent {
        match self {
            Self::Requested => ThreadEvent::ThreadReadRequested,
            Self::NotRequested => ThreadEvent::ThreadReadNotRequested,
        }
    }

    /// What the agent reads back after deciding, from the tool and the CLI.
    pub(crate) fn ack(self) -> &'static str {
        match self {
            Self::Requested => {
                "Recorded. Once this turn ends, the thread is listed under \
                Review until the user has read your reply. A ready change or a \
                harden hold that lists it already takes its place."
            }
            Self::NotRequested => {
                "Recorded. The thread does not wait for the user to read \
                this reply."
            }
        }
    }
}

/// Record `thread_id`'s own agent's read decision. The `request_read` tool's
/// actor: the agent thread itself.
pub(crate) async fn record_agent_read_decision(
    bus: &EventBus,
    thread_id: Uuid,
    decision: ReadDecision,
) -> Result<(), BoxError> {
    record_read_decision(
        bus,
        thread_id,
        decision,
        Some(agent_thread_actor(thread_id)),
    )
    .await
}

/// Record a read decision on `thread_id`, by `actor`. `None` is the engine
/// deciding, when the turn-end gate forced the decision itself.
pub(crate) async fn record_read_decision(
    bus: &EventBus,
    thread_id: Uuid,
    decision: ReadDecision,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: decision.event(),
        meta: EventMeta::with_actor(actor),
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
