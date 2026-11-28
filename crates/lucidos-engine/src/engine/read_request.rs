use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::agent_thread_actor;
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::gateway_users::Reader;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub use crate::llm::tools::READ_ARG;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadDecision {
    Requested,
    NotRequested,
}

impl ReadDecision {
    pub fn from_read(read: bool) -> Self {
        if read {
            Self::Requested
        } else {
            Self::NotRequested
        }
    }

    pub fn from_tool_args(args: &serde_json::Value) -> Result<Self, String> {
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

    fn event(self, parent_carries: bool) -> ThreadEvent {
        match self {
            Self::Requested => ThreadEvent::ThreadReadRequested {
                parent_report: parent_carries,
            },
            Self::NotRequested => ThreadEvent::ThreadReadNotRequested {
                supersedes_request: parent_carries,
            },
        }
    }

    pub fn ack(self, integrated_parent_carries: bool) -> &'static str {
        match self {
            Self::Requested if integrated_parent_carries => {
                "Recorded. Your parent thread carries the read for this reply."
            }
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

pub async fn record_agent_read_decision(
    bus: &EventBus,
    thread_id: Uuid,
    decision: ReadDecision,
    parent_carries: bool,
) -> Result<(), BoxError> {
    record_read_decision(
        bus,
        thread_id,
        decision,
        Some(agent_thread_actor(thread_id)),
        parent_carries,
    )
    .await
}

pub async fn record_read_decision(
    bus: &EventBus,
    thread_id: Uuid,
    decision: ReadDecision,
    actor: Option<MessageOrigin>,
    parent_carries: bool,
) -> Result<(), BoxError> {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: decision.event(parent_carries),
        meta: EventMeta::with_actor(actor),
    })
    .await?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReplySeen {
    Cleared,
    NothingPending,
    Stale,
    UnknownThread,
}

pub async fn record_reply_seen(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    seen_version: i64,
    actor: MessageOrigin,
    reader: &Reader,
) -> Result<ReplySeen, BoxError> {
    let pending: Option<bool> = sqlx::query_scalar(
        "SELECT read_requested AND NOT (reply_seen_devices && COALESCE($2::text[], '{}')) \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .bind(reader.devices())
    .fetch_optional(pool)
    .await?;
    match pending {
        None => Ok(ReplySeen::UnknownThread),
        Some(false) => Ok(ReplySeen::NothingPending),
        Some(true) => {
            let emitted = bus
                .emit(BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ThreadReplySeen {
                        seen_version,
                        personal: matches!(reader, Reader::User { .. }),
                    },
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
