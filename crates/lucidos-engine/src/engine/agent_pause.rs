use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{AgentPauseCause, EventMeta, MessageOrigin, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseChange {
    Pause,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageAudience {
    Agent,
    UserOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserOnlyMessageRoute {
    pub pause_change: Option<PauseChange>,
    pub audience: MessageAudience,
}

/// INVARIANT: The one definition of a typed message's route.
///
/// - The agent mentioned: it wakes, and resumes if paused.
/// - Only users mentioned: it pauses the agent and stays user-only.
/// - Nobody mentioned: the pause decides, except that a reply to an open
///   question card always reaches the agent, which is blocked on it.
pub fn user_only_message_route(
    users_mentioned: bool,
    agent_mentioned: bool,
    paused: bool,
    question_open: bool,
) -> UserOnlyMessageRoute {
    let (pause_change, audience) = match (agent_mentioned, users_mentioned, paused) {
        (true, _, true) => (Some(PauseChange::Resume), MessageAudience::Agent),
        (true, _, false) => (None, MessageAudience::Agent),
        (false, true, false) => (Some(PauseChange::Pause), MessageAudience::UserOnly),
        (false, false, true) if question_open => (None, MessageAudience::Agent),
        (false, _, true) => (None, MessageAudience::UserOnly),
        (false, false, false) => (None, MessageAudience::Agent),
    };
    UserOnlyMessageRoute {
        pause_change,
        audience,
    }
}

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
