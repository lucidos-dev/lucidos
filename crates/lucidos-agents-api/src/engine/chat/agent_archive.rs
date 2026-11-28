use uuid::Uuid;

use crate::engine::archive_family::{
    archive_family, ArchiveOutcome, ArchiveRejection, PinnedMembers,
};
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{agent_thread_actor, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::thread_lifecycle::ThreadStatus;

#[derive(Debug)]
pub(crate) enum AgentArchiveError {
    UnknownThread(Uuid),
    Discarded(Uuid),
    Refused(ArchiveRejection),
    Internal(String),
}

impl AgentArchiveError {
    pub fn status_code(&self) -> u16 {
        match self {
            Self::UnknownThread(_) => 404,
            Self::Discarded(_) => 409,
            Self::Refused((status, _)) => status.as_u16(),
            Self::Internal(_) => 500,
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::UnknownThread(_) => "thread_not_found",
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
            Self::UnknownThread(id) => write!(f, "No thread {id} exists in this workspace."),
            Self::Discarded(id) => {
                write!(f, "Thread {id} was discarded, so it cannot be archived.")
            }
            Self::Refused(rejection) => write!(
                f,
                "{}",
                crate::engine::archive_family::rejection_text(rejection)
            ),
            Self::Internal(msg) => write!(f, "Archiving the thread failed: {msg}"),
        }
    }
}

impl std::error::Error for AgentArchiveError {}

pub(crate) enum AgentArchiveAck {
    Archived(ArchiveOutcome),
    Requested { thread_id: Uuid },
}

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
    crate::log_legacy!(
        "[AgentArchive] {} asked to be archived when it settles",
        thread_id
    );
    Ok(())
}

type TargetRow = (Option<String>, String, bool, bool, bool);

#[easy_ext::ext(AgentArchiveExt)]
pub(crate) impl crate::engine::LucidosEngine {
    async fn check_agent_archive_eligibility(
        pool: &sqlx::PgPool,
        target: Uuid,
        self_archive: bool,
    ) -> Result<(), AgentArchiveError> {
        let row: Option<TargetRow> = sqlx::query_as(
            "SELECT state, status, coding_agent_change_state = $2, is_saved, \
                    is_home \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(target)
        .bind(crate::engine::thread_lifecycle::ChangeStateKind::Proposed)
        .fetch_optional(pool)
        .await
        .map_err(|e| AgentArchiveError::Internal(e.to_string()))?;
        let Some((state, status, has_pending_changes, is_saved, is_home)) = row else {
            return Err(AgentArchiveError::UnknownThread(target));
        };
        if state.as_deref() == Some("discarded") {
            return Err(AgentArchiveError::Discarded(target));
        }
        if is_home {
            return Err(AgentArchiveError::Refused(
                crate::engine::archive_family::home_thread_rejection(target),
            ));
        }
        if is_saved {
            return Err(AgentArchiveError::Refused(
                crate::engine::archive_family::pinned_rejection(target),
            ));
        }
        if self_archive && ThreadStatus::parse(&status) == ThreadStatus::WaitingForUserAnswer {
            return Err(AgentArchiveError::Refused(
                crate::engine::archive_family::waiting_rejection(target, has_pending_changes),
            ));
        }
        Ok(())
    }

    async fn archive_as_agent(
        &self,
        caller: Option<Uuid>,
        target: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<AgentArchiveAck, AgentArchiveError> {
        let self_archive = caller == Some(target);
        Self::check_agent_archive_eligibility(self.pool(), target, self_archive).await?;
        if self_archive {
            Box::pin(record_archive_request(&self.event_bus, target))
                .await
                .map_err(|e| AgentArchiveError::Internal(e.to_string()))?;
            return Ok(AgentArchiveAck::Requested { thread_id: target });
        }
        let engine = self.clone_arc();
        Box::pin(archive_family(
            &engine,
            target,
            actor,
            PinnedMembers::LeaveOpen,
        ))
        .await
        .map(AgentArchiveAck::Archived)
        .map_err(AgentArchiveError::Refused)
    }
}

#[cfg(test)]
#[path = "agent_archive_tests.rs"]
mod tests;
