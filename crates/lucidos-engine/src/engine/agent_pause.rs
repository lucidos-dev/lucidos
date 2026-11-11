//! The *agent pause* and the route of a typed message on a team gateway
//! (ADR 0649).
//!
//! A message either wakes the thread's agent or stays between people: a
//! *people-only message*. Mentions decide it, together with the pause, which
//! is shared thread state. The pause lives only in events: the last
//! `AgentPaused` or `AgentResumed` on the thread wins.

use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{AgentPauseCause, EventMeta, MessageOrigin, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A flip of the agent pause that a message makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseChange {
    Pause,
    Resume,
}

/// Who a typed message is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageAudience {
    /// The thread's agent, which starts a turn as any message does.
    Agent,
    /// Only the people in the thread. No turn starts.
    PeopleOnly,
}

/// What one typed message does on a team gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeopleMessageRoute {
    pub pause_change: Option<PauseChange>,
    pub audience: MessageAudience,
}

/// The one definition of a typed message's route.
///
/// - The agent mentioned: it wakes, and resumes if paused.
/// - Only people mentioned: it pauses the agent and stays people-only.
/// - Nobody mentioned: the pause decides, except that a reply to an open
///   question card always reaches the agent, which is blocked on it.
pub fn people_message_route(
    people_mentioned: bool,
    agent_mentioned: bool,
    paused: bool,
    question_open: bool,
) -> PeopleMessageRoute {
    let (pause_change, audience) = match (agent_mentioned, people_mentioned, paused) {
        (true, _, true) => (Some(PauseChange::Resume), MessageAudience::Agent),
        (true, _, false) => (None, MessageAudience::Agent),
        (false, true, false) => (Some(PauseChange::Pause), MessageAudience::PeopleOnly),
        (false, false, true) if question_open => (None, MessageAudience::Agent),
        (false, _, true) => (None, MessageAudience::PeopleOnly),
        (false, false, false) => (None, MessageAudience::Agent),
    };
    PeopleMessageRoute {
        pause_change,
        audience,
    }
}

/// Whether the thread's agent is paused: its last pause event was
/// `AgentPaused`. A thread with neither event listens.
pub async fn agent_paused(pool: &PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    let last: Option<String> = sqlx::query_scalar(
        "SELECT event_type FROM events \
         WHERE thread_id = $1 AND event_type IN ('AgentPaused', 'AgentResumed') \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(last.as_deref() == Some("AgentPaused"))
}

/// Record a pause flip on the thread, as `actor`.
pub async fn record_pause_change(
    bus: &EventBus,
    thread_id: Uuid,
    change: PauseChange,
    cause: AgentPauseCause,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    let event = match change {
        PauseChange::Pause => ThreadEvent::AgentPaused { cause },
        PauseChange::Resume => ThreadEvent::AgentResumed { cause },
    };
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::with_actor(actor),
    })
    .await
    .map(|_| ())
}

/// Set the pause by hand, from the chip. A repeat that changes nothing
/// records nothing. Answers the pause after the call.
pub async fn set_agent_paused(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    paused: bool,
    actor: Option<MessageOrigin>,
) -> Result<bool, BoxError> {
    let change = match (agent_paused(pool, thread_id).await?, paused) {
        (false, true) => PauseChange::Pause,
        (true, false) => PauseChange::Resume,
        _ => return Ok(paused),
    };
    record_pause_change(bus, thread_id, change, AgentPauseCause::Chip, actor).await?;
    Ok(paused)
}

#[cfg(test)]
#[path = "agent_pause_tests.rs"]
mod tests;
