//! Moving a child thread to top level: the parent stops waiting for it
//! (ADR 0278).
//!
//! The move is one event, `ChildThreadDetached`, on the **former parent**. Its
//! projection arm cuts the edge. Nothing is stopped: a child mid-turn finishes
//! that turn, keeps its work and proposes its change, and its result lands on
//! its own timeline only.
//!
//! Two callers, told apart by what the engine verified about them:
//!
//! - **An agent** holds to ADR 0043's ladder. It moves only its own direct
//!   children, and the caller is ambient, never a parameter.
//! - **The user** moves any thread that has a parent. The route trusts an
//!   untokened caller exactly as archive and discard do (ADR 0052).
//!
//! ## Why a typed error here
//!
//! For the reason `ChildFollowUpError` has one: the route maps each variant to
//! a status, and the LLM tool turns each into its own actionable sentence.

use uuid::Uuid;

use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{EventMeta, ThreadEvent};

/// Who asked for the move, as the engine verified it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachCaller {
    /// The user's own device, or the local API with no origin token.
    User,
    /// A thread-bound caller: an LLM tool's ambient thread, or a verified
    /// origin token. `None` is a verified subprocess with no thread of its own.
    Agent(Option<Uuid>),
}

/// Why a move to top level was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildDetachError {
    /// No `thread_summaries` row for the target id.
    UnknownThread(Uuid),
    /// The user asked to move a thread that is already top level.
    NotAChild(Uuid),
    /// An agent asked to move a thread that is not its own direct child. One
    /// variant for a top-level thread, a sibling and a grandchild alike, as
    /// `ChildFollowUpError::NotYourChild` is.
    NotYourChild(Uuid),
    /// The target was thrown away by the user.
    Discarded(Uuid),
    /// An agent addressed itself.
    SelfTarget(Uuid),
    /// A verified subprocess with no thread, so it has no children.
    NoCaller,
    /// The row read or the emit failed.
    Internal(String),
}

impl ChildDetachError {
    /// HTTP status for `POST /api/v1/threads/:thread_id/detach`.
    pub fn status_code(&self) -> u16 {
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

/// What the caller gets back once the move is on the former parent's timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachAck {
    pub child_thread_id: Uuid,
    pub child_title: String,
    pub former_parent_id: Uuid,
}

/// The target's row as the ladder reads it:
/// `(parent_thread_id, state, title, first_message)`.
type TargetRow = (Option<Uuid>, Option<String>, Option<String>, Option<String>);

impl crate::engine::LucidosEngine {
    /// Load the target's row and run the refusal ladder. Pure: reads one row
    /// and writes nothing, so it is testable without an engine.
    ///
    /// Returns the former parent and the child's label.
    pub(crate) async fn authorize_child_detach(
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

    /// Move `target` to top level on `caller`'s authority.
    ///
    /// `meta` carries the actor, so the event says who moved the thread.
    pub async fn detach_child_thread(
        &self,
        caller: DetachCaller,
        target: Uuid,
        meta: EventMeta,
    ) -> Result<DetachAck, ChildDetachError> {
        let (former_parent_id, child_title) =
            Self::authorize_child_detach(self.pool(), caller, target).await?;

        // Boxed: the emit reaches the fan-in, which can re-enter an agentic
        // loop whose tool dispatch calls back into this function.
        let emitted = Box::pin(self.event_bus.emit(BusEvent::Thread {
            thread_id: former_parent_id,
            event: ThreadEvent::ChildThreadDetached {
                child_thread_id: target,
                child_thread_title: Some(child_title.clone()),
            },
            meta,
        }))
        .await
        .map_err(|e| ChildDetachError::Internal(e.to_string()))?;
        // The bus drops a move whose edge a concurrent move already cut.
        if emitted.is_none() {
            return Err(ChildDetachError::NotAChild(target));
        }

        crate::log!(
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
