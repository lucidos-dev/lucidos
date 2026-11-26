use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetachCaller {
    User,
    Agent(Option<Uuid>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChildDetachError {
    UnknownThread(Uuid),
    NotAChild(Uuid),
    NotYourChild(Uuid),
    Discarded(Uuid),
    SelfTarget(Uuid),
    NoCaller,
    Internal(String),
}

impl ChildDetachError {
    pub(crate) fn status_code(&self) -> u16 {
        match self {
            Self::UnknownThread(_) => 404,
            Self::NotAChild(_) | Self::Discarded(_) => 409,
            Self::NotYourChild(_) | Self::NoCaller => 403,
            Self::SelfTarget(_) => 400,
            Self::Internal(_) => 500,
        }
    }
}

impl std::fmt::Display for ChildDetachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownThread(id) => write!(f, "No thread {id} exists in this workspace."),
            Self::NotAChild(id) => write!(f, "Thread {id} is already at top level."),
            Self::NotYourChild(id) => write!(
                f,
                "Thread {id} is not one of your child threads. A thread can only move \
                 its own direct children to top level."
            ),
            Self::Discarded(id) => write!(f, "Thread {id} was discarded, so it cannot be moved."),
            Self::SelfTarget(_) => write!(
                f,
                "A thread cannot move itself to top level. Address one of its child threads."
            ),
            Self::NoCaller => write!(
                f,
                "Moving a child to top level needs a caller thread, and this request has none."
            ),
            Self::Internal(msg) => write!(f, "Moving the thread to top level failed: {msg}"),
        }
    }
}

impl std::error::Error for ChildDetachError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetachAck {
    pub child_thread_id: Uuid,
    pub child_title: String,
    pub former_parent_id: Uuid,
}

type TargetRow = (Option<Uuid>, Option<String>, Option<String>, Option<String>);

#[easy_ext::ext(ChildDetachExt)]
pub(crate) impl crate::engine::LucidosEngine {
    async fn authorize_child_detach(
        pool: &sqlx::PgPool,
        caller: DetachCaller,
        target: Uuid,
    ) -> Result<(Uuid, String), ChildDetachError> {
        if let DetachCaller::Agent(caller_thread) = caller {
            match caller_thread {
                None => return Err(ChildDetachError::NoCaller),
                Some(id) if id == target => return Err(ChildDetachError::SelfTarget(target)),
                Some(_) => {}
            }
        }

        let row: Option<TargetRow> = sqlx::query_as(
            "SELECT parent_thread_id, state, title, first_message \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(target)
        .fetch_optional(pool)
        .await
        .map_err(|e| ChildDetachError::Internal(e.to_string()))?;
        let Some((parent, state, title, first_message)) = row else {
            return Err(ChildDetachError::UnknownThread(target));
        };

        let parent = match (caller, parent) {
            (DetachCaller::Agent(caller_thread), parent) if parent != caller_thread => {
                return Err(ChildDetachError::NotYourChild(target));
            }
            (_, None) => return Err(ChildDetachError::NotAChild(target)),
            (_, Some(parent)) => parent,
        };
        if state.as_deref() == Some("discarded") {
            return Err(ChildDetachError::Discarded(target));
        }

        Ok((
            parent,
            super::child_follow_up::child_label(title, first_message.as_deref()),
        ))
    }

    async fn detach_child_thread(
        &self,
        caller: DetachCaller,
        target: Uuid,
        meta: EventMeta,
    ) -> Result<DetachAck, ChildDetachError> {
        Self::move_child_to_top_level(&self.event_bus, caller, target, meta).await
    }

    async fn move_child_to_top_level(
        bus: &EventBus,
        caller: DetachCaller,
        target: Uuid,
        meta: EventMeta,
    ) -> Result<DetachAck, ChildDetachError> {
        let (former_parent_id, child_title) =
            Self::authorize_child_detach(bus.pool(), caller, target).await?;

        let emitted = Box::pin(bus.emit(BusEvent::Thread {
            thread_id: former_parent_id,
            event: ThreadEvent::ChildThreadDetached {
                child_thread_id: target,
                child_thread_title: Some(child_title.clone()),
            },
            meta,
        }))
        .await
        .map_err(|e| ChildDetachError::Internal(e.to_string()))?;
        if emitted.is_none() {
            return Err(ChildDetachError::NotAChild(target));
        }

        crate::log_legacy!(
            "[ChildDetach] Moved {} out of {} to top level",
            target,
            former_parent_id
        );
        Ok(DetachAck {
            child_thread_id: target,
            child_title,
            former_parent_id,
        })
    }
}

#[cfg(test)]
#[path = "child_detach_tests.rs"]
mod tests;
