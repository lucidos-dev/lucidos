//! How far a destructive verb reaches: a thread reaches itself and its own
//! descendants, and nothing else.
//!
//! Archive and cancel end another thread's work, and both took any thread id
//! in the workspace with no authorization ladder. So a subprocess on the
//! loopback API could archive or cancel a sibling, or its own parent. Plan:
//! `docs/plans/2026-08-17-archive-and-cancel-reach-self-and-descendants.md`.
//!
//! This is the ladder the messaging edge already had (ADR 0043,
//! `engine::chat::child_follow_up`). It widens from direct children to
//! descendants, because the archive route's cascade walks the whole family.
//! Three properties come with it:
//!
//! - **The caller is authenticated, never asserted.** It is the prefix of the
//!   thread-bound origin token, which is HMAC-covered (`api::actor`).
//! - **The relationship is read, never claimed.** The caller passes a target
//!   id and says nothing about how the two are related.
//! - **A refusal is typed, with its status beside the taxonomy**, because two
//!   handlers with different body shapes consume it structurally.

use axum::http::{HeaderMap, StatusCode};
use uuid::Uuid;

use crate::api::actor::SubprocessOrigin;

/// Which destructive verb was attempted. Carried so the refusal and the log
/// line name the verb instead of saying "this action": an agent reading "you
/// cannot cancel that thread" knows what it just tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadReachVerb {
    Archive,
    Cancel,
}

impl ThreadReachVerb {
    fn word(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Cancel => "cancel",
        }
    }
}

/// Why a destructive verb was refused. Every branch is a refusal: none falls
/// back to a narrower target, and none reports success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ThreadReachError {
    /// The target is neither the caller nor one of its descendants.
    /// Deliberately one variant for a sibling, a parent and an unknown thread,
    /// exactly as `ChildFollowUpError::NotYourChild` is: the caller learns the
    /// thread is out of reach, and nothing about whose it is.
    OutOfReach { target: Uuid, verb: ThreadReachVerb },
    /// A verified subprocess with no thread of its own, which is a scheduled
    /// script. There is no subtree to scope it to, so it is refused rather than
    /// handed every thread in the workspace.
    NoCallerThread(ThreadReachVerb),
    /// A thread-bound caller asked to cancel with no `thread_id`, which stops
    /// every thread in the workspace. Refused rather than silently reread as
    /// "cancel yourself": a caller that meant itself can say so, and one that
    /// meant everything must not get it.
    UnscopedCancel,
    /// The ancestor chain could not be read, so the call fails closed. That
    /// costs the user nothing: only a token-bearing caller runs that query, so
    /// a database hiccup can never reach the Archive or Stop button.
    Unverifiable(String),
}

impl ThreadReachError {
    /// HTTP status, beside the taxonomy so neither handler's mapping can drift
    /// from it.
    pub(crate) fn status_code(&self) -> StatusCode {
        match self {
            Self::OutOfReach { .. } | Self::NoCallerThread(_) | Self::UnscopedCancel => {
                StatusCode::FORBIDDEN
            }
            Self::Unverifiable(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Machine-readable slug for the archive route, whose rejection bodies are
    /// `{reason, ...}` rather than `{error}`.
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Self::OutOfReach { .. } => "out_of_reach",
            Self::NoCallerThread(_) => "no_caller_thread",
            Self::UnscopedCancel => "unscoped_cancel",
            Self::Unverifiable(_) => "reach_unverifiable",
        }
    }
}

impl std::fmt::Display for ThreadReachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfReach { target, verb } => write!(
                f,
                "Thread {target} is out of your reach, so you cannot {} it. A thread \
                 can only {} itself and the sub-threads below it. To reach a sibling \
                 or your own parent, say so to your parent thread and let it decide.",
                verb.word(),
                verb.word()
            ),
            Self::NoCallerThread(verb) => write!(
                f,
                "This request comes from a Lucidos subprocess with no thread of its \
                 own, so there is no subtree to {} within.",
                verb.word()
            ),
            Self::UnscopedCancel => write!(
                f,
                "Cancelling with no thread_id stops every thread in the workspace, \
                 which is the user's own Stop. Name a thread instead: your own id, or \
                 one of the sub-threads below it."
            ),
            Self::Unverifiable(msg) => write!(
                f,
                "Could not check whether that thread is within your reach, so the \
                 request was refused: {msg}"
            ),
        }
    }
}

impl std::error::Error for ThreadReachError {}

/// Is `target` the caller itself, or one of its descendants?
///
/// Self is answered with no query at all. Otherwise one recursive CTE walks
/// from the target UPWARD, so the cost is the depth of the tree rather than its
/// width: a parent of ten children pays what a parent of one pays.
///
/// Takes the pool rather than `&self`, for the reason
/// `authorize_child_follow_up` does: the whole check is one query and a ladder,
/// so it is testable without standing up an engine.
pub(crate) async fn authorize_thread_reach(
    pool: &sqlx::PgPool,
    caller_thread_id: Uuid,
    target_thread_id: Uuid,
    verb: ThreadReachVerb,
) -> Result<(), ThreadReachError> {
    if caller_thread_id == target_thread_id {
        return Ok(());
    }

    // Mirrors the `ancestors` CTE in
    // `engine::event_bus_projection_propagation`, including its lack of a depth
    // cap: `parent_thread_id` is stamped once at spawn from an already-existing
    // thread, so the graph is a forest and the walk terminates.
    let reachable: bool = sqlx::query_scalar(
        "WITH RECURSIVE ancestors AS ( \
            SELECT parent_thread_id AS thread_id \
            FROM thread_summaries \
            WHERE thread_id = $1 AND parent_thread_id IS NOT NULL \
            UNION ALL \
            SELECT t.parent_thread_id AS thread_id \
            FROM thread_summaries t \
            JOIN ancestors a ON t.thread_id = a.thread_id \
            WHERE t.parent_thread_id IS NOT NULL \
         ) \
         SELECT EXISTS (SELECT 1 FROM ancestors WHERE thread_id = $2)",
    )
    .bind(target_thread_id)
    .bind(caller_thread_id)
    .fetch_one(pool)
    .await
    .map_err(|e| {
        crate::log!(
            "[ThreadReach] Could not read the ancestors of {} for caller {}: {}",
            target_thread_id,
            caller_thread_id,
            e
        );
        ThreadReachError::Unverifiable(e.to_string())
    })?;

    if !reachable {
        crate::log!(
            "[ThreadReach] Refused: thread {} tried to {} thread {}, which is neither \
             itself nor a descendant",
            caller_thread_id,
            verb.word(),
            target_thread_id
        );
        return Err(ThreadReachError::OutOfReach {
            target: target_thread_id,
            verb,
        });
    }
    Ok(())
}

/// Refuse a destructive verb aimed outside the caller's own subtree.
///
/// `target` is optional because `POST /api/v1/chat/cancel` takes no thread id
/// when it means "everything". That form is the user's global Stop, so it is
/// left alone for an untokened caller and refused for a thread-bound one.
///
/// Runs BEFORE either handler writes anything, so a refusal leaves no
/// half-applied cascade and no cancel-stamped question card behind.
///
/// ## A caller presenting no token keeps its reach
///
/// That is the user's own device, the local API surface and the e2e suites,
/// and it is the answer `threads::actions::refuse_event_waits_for_another_thread`
/// already gives. It is also the residual this check does not close: `/api/v1`
/// carries no authentication (`docs/glossary.md` § unattributed caller), so a
/// subprocess that DROPS its token reads as an ordinary local caller. Every
/// route here has that property, and moving the boundary is owed its own ADR.
/// What this buys is that a caller presenting its credential is bound by it.
pub(in crate::api) async fn refuse_out_of_reach(
    pool: &sqlx::PgPool,
    headers: &HeaderMap,
    target: Option<Uuid>,
    verb: ThreadReachVerb,
) -> Result<(), ThreadReachError> {
    let caller_thread_id = match crate::api::actor::subprocess_origin(headers) {
        SubprocessOrigin::NotSubprocess => return Ok(()),
        SubprocessOrigin::Subprocess { source_thread_id } => source_thread_id,
    };
    let Some(caller_thread_id) = caller_thread_id else {
        crate::log!(
            "[ThreadReach] Refused: a threadless subprocess tried to {} {:?}",
            verb.word(),
            target
        );
        return Err(ThreadReachError::NoCallerThread(verb));
    };
    let Some(target) = target else {
        crate::log!(
            "[ThreadReach] Refused: thread {} tried to cancel every thread",
            caller_thread_id
        );
        return Err(ThreadReachError::UnscopedCancel);
    };
    authorize_thread_reach(pool, caller_thread_id, target, verb).await
}

#[cfg(test)]
#[path = "thread_reach_tests.rs"]
mod tests;
