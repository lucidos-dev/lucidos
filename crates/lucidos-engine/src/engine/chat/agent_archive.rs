//! An agent archiving a thread on its own authority (ADR 0310).
//!
//! The ladder is detach's (ADR 0278): the caller is ambient, never a parameter,
//! and it reaches itself and its own direct children, nothing else. No
//! standing instruction widens it.
//!
//! What happens next depends on the target:
//!
//! - **A direct child** runs the Archive button's cascade now, gate included.
//! - **The caller itself** is mid-turn, so the cascade would refuse it, and the
//!   turn's own end would move it back to the inbox anyway. The call records a
//!   `ThreadArchiveRequested` instead, and the archive request resolver
//!   (`engine::archive_request`) archives it once the turn has settled.

use uuid::Uuid;

use crate::api::threads::archive::{
    archive_family, ArchiveOutcome, ArchiveRejection, PinnedMembers,
};
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{ActorMode, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::thread_lifecycle::ThreadStatus;

/// Why an agent's archive was refused before the cascade ran, or by it. A
/// pinned target is a `Refused` with the cascade's own `thread_pinned` body.
#[derive(Debug)]
pub(crate) enum AgentArchiveError {
    /// A verified subprocess with no thread of its own.
    NoCaller,
    /// No `thread_summaries` row for the target id.
    UnknownThread(Uuid),
    /// The target is neither the caller nor one of its direct children. One
    /// variant for a parent, a sibling and a grandchild, as detach has.
    NotYourThread(Uuid),
    /// The target was thrown away by the user.
    Discarded(Uuid),
    /// The Archive button's gate refused, with the route's own status and body.
    Refused(ArchiveRejection),
    /// A row read or an emit failed.
    Internal(String),
}

impl AgentArchiveError {
    /// HTTP status for `POST /api/v1/threads/:thread_id/archive`.
    pub fn status_code(&self) -> u16 {
        match self {
            Self::UnknownThread(_) => 404,
            Self::Discarded(_) => 409,
            Self::NoCaller | Self::NotYourThread(_) => 403,
            Self::Refused((status, _)) => status.as_u16(),
            Self::Internal(_) => 500,
        }
    }

    /// Machine-readable slug, beside the status.
    pub fn reason(&self) -> &str {
        match self {
            Self::NoCaller => "no_caller_thread",
            Self::UnknownThread(_) => "thread_not_found",
            Self::NotYourThread(_) => "not_your_thread",
            Self::Discarded(_) => "thread_discarded",
            Self::Refused((_, body)) => body
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("archive_refused"),
            Self::Internal(_) => "internal_error",
        }
    }
}

impl std::fmt::Display for AgentArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCaller => write!(
                f,
                "Archiving a thread needs a caller thread, and this request has none."
            ),
            Self::UnknownThread(id) => write!(f, "No thread {id} exists in this workspace."),
            Self::NotYourThread(id) => write!(
                f,
                "Thread {id} is neither this thread nor one of its direct children. A thread \
                 can only archive itself and the child threads it spawned."
            ),
            Self::Discarded(id) => {
                write!(f, "Thread {id} was discarded, so it cannot be archived.")
            }
            Self::Refused(rejection) => write!(
                f,
                "{}",
                crate::api::threads::archive::rejection_text(rejection)
            ),
            Self::Internal(msg) => write!(f, "Archiving the thread failed: {msg}"),
        }
    }
}

impl std::error::Error for AgentArchiveError {}

/// What the ladder allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentArchiveTarget {
    /// The caller archives itself, once its turn ends.
    Caller,
    /// One of the caller's direct children, archived now.
    DirectChild,
}

/// What the caller gets back.
pub(crate) enum AgentArchiveAck {
    /// A direct child's cascade ran.
    Archived(ArchiveOutcome),
    /// The caller's own archive is recorded, and lands once its turn settles.
    Requested { thread_id: Uuid },
}

/// The actor every agent archive carries: an agent, and the thread it ran in.
/// The shape an agent subprocess already gets on any route, so the tool and
/// the CLI record the same event.
pub(crate) fn agent_thread_actor(caller: Uuid) -> MessageOrigin {
    MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: Some(caller),
    }
}

/// Record that `thread_id`'s own agent asked to be archived once its turn
/// ends. The actor names that thread, as every agent archive does.
pub(crate) async fn record_archive_request(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchiveRequested,
        meta: EventMeta::with_actor(Some(agent_thread_actor(thread_id))),
    })
    .await?;
    crate::log!(
        "[AgentArchive] {} asked to be archived when it settles",
        thread_id
    );
    Ok(())
}

/// The target's row as the ladder reads it:
/// `(parent_thread_id, state, status, coding_agent_proposed, is_saved, is_home)`.
type TargetRow = (Option<Uuid>, Option<String>, String, bool, bool, bool);

impl crate::engine::LucidosEngine {
    /// Load the target's row and run the ladder. Reads one row and writes
    /// nothing, so it is testable without an engine.
    ///
    /// A waiting caller is refused here with the Archive route's own error
    /// (ADR 0259). A running one is not: that is the ordinary case, and the
    /// request waits for the turn to end.
    pub(crate) async fn authorize_agent_archive(
        pool: &sqlx::PgPool,
        caller: Option<Uuid>,
        target: Uuid,
    ) -> Result<AgentArchiveTarget, AgentArchiveError> {
        let caller = caller.ok_or(AgentArchiveError::NoCaller)?;
        let row: Option<TargetRow> = sqlx::query_as(
            "SELECT parent_thread_id, state, status, coding_agent_proposed, is_saved, is_home \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(target)
        .fetch_optional(pool)
        .await
        .map_err(|e| AgentArchiveError::Internal(e.to_string()))?;
        let Some((parent, state, status, has_pending_changes, is_saved, is_home)) = row else {
            return Err(AgentArchiveError::UnknownThread(target));
        };
        let kind = if target == caller {
            AgentArchiveTarget::Caller
        } else if parent == Some(caller) {
            AgentArchiveTarget::DirectChild
        } else {
            return Err(AgentArchiveError::NotYourThread(target));
        };
        if state.as_deref() == Some("discarded") {
            return Err(AgentArchiveError::Discarded(target));
        }
        // The home thread never ends (ADR 0362). Refused here too, so its own
        // agent is told at once rather than recording a request nothing grants.
        if is_home {
            return Err(AgentArchiveError::Refused(
                crate::api::threads::archive::home_thread_rejection(target),
            ));
        }
        // A pinned thread is the user's to archive, never an agent's (ADR 0312).
        // Refused here too, so a self-archive is told at once instead of
        // recording a request the pin would hold back.
        if is_saved {
            return Err(AgentArchiveError::Refused(
                crate::api::threads::archive::pinned_rejection(target),
            ));
        }
        if kind == AgentArchiveTarget::Caller
            && ThreadStatus::parse(&status) == ThreadStatus::WaitingForUserAnswer
        {
            return Err(AgentArchiveError::Refused(
                crate::api::threads::archive::waiting_rejection(target, has_pending_changes),
            ));
        }
        Ok(kind)
    }

    /// Archive `target` on `caller`'s authority: a direct child now, the caller
    /// itself once its turn ends.
    pub(crate) async fn archive_as_agent(
        &self,
        caller: Option<Uuid>,
        target: Uuid,
    ) -> Result<AgentArchiveAck, AgentArchiveError> {
        let caller = caller.ok_or(AgentArchiveError::NoCaller)?;
        let kind = Self::authorize_agent_archive(self.pool(), Some(caller), target).await?;
        let actor = Some(agent_thread_actor(caller));
        match kind {
            AgentArchiveTarget::DirectChild => {
                let engine = self.clone_arc();
                // Boxed: the cascade's emits reach the fan-in, which can
                // re-enter an agentic loop whose tool dispatch calls back here.
                Box::pin(archive_family(
                    &engine,
                    target,
                    actor,
                    PinnedMembers::ByActor,
                ))
                .await
                .map(AgentArchiveAck::Archived)
                .map_err(AgentArchiveError::Refused)
            }
            AgentArchiveTarget::Caller => {
                Box::pin(record_archive_request(&self.event_bus, target))
                    .await
                    .map_err(|e| AgentArchiveError::Internal(e.to_string()))?;
                Ok(AgentArchiveAck::Requested { thread_id: target })
            }
        }
    }
}

#[cfg(test)]
#[path = "agent_archive_tests.rs"]
mod tests;
