//! May this caller press this button? Two questions, and both are asked here.
//!
//! **Which threads may it aim at?** A thread reaches itself and its own
//! descendants, on its own authority, and nothing else (ADR 0168 clause 3).
//! This is the ladder the messaging edge already had (ADR 0043,
//! `engine::chat::child_follow_up`), widened from direct children to
//! descendants because the archive cascade walks the whole family.
//!
//! **On whose behalf does it act?** Anything wider is the *workspace owner*'s
//! button, and a thread may press one only while carrying their standing
//! instruction (clauses 4 and 5). `api::standing_instruction` owns that answer,
//! and this module refuses when neither half covers the act.
//!
//! The gate is per VERB, never per route. Three verbs arrive by more than one
//! path, and gating the first path of each is how the ungated set grew.
//!
//! **Two things here are still open, and both are recorded rather than left to
//! be rediscovered** in `docs/plans/2026-08-30-a-thread-acts-in-its-own-subtree.md`.
//! Resolving a permission card is the one clause-4 verb with no gate, in
//! `command_permission` and `mcp_permission`. Taking an owner's standing apply
//! back is the other, and clause 4 has no verb for it: it classifies pressing
//! an owner button, not revoking one.
//!
//! - **The caller is authenticated, never asserted.** It is the prefix of the
//!   thread-bound origin token, which is HMAC-covered (`api::actor`).
//! - **The relationship is read, never claimed.** The caller passes a target
//!   id and says nothing about how the two are related.
//! - **A refusal is typed, with its status beside the taxonomy**, because
//!   handlers with different body shapes consume it structurally.

use axum::http::{HeaderMap, StatusCode};
use uuid::Uuid;

use crate::api::actor::SubprocessOrigin;
use crate::api::standing_instruction::carries_standing_instruction;

/// Which clause-4 verb was attempted. Carried so the refusal and the log line
/// name the verb instead of saying "this action": an agent reading "you cannot
/// cancel that thread" knows what it just tried.
///
/// One variant per VERB, never per route. `Apply` covers the single change, the
/// batch and a coding-agent thread's own apply alike. The authority question is
/// the same one, and the caller reads the same sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadReachVerb {
    Archive,
    Cancel,
    Apply,
    Discard,
    AnswerQuestion,
    Continue,
    /// Creating a thread with no parent, which aims at the workspace root. The
    /// root is not a thread and gets no row (ADR 0168 clause 1), so no place in
    /// the tree reaches it.
    CreateTopThread,
}

impl ThreadReachVerb {
    /// The verb as an infinitive taking a thread, for "you cannot {} it".
    ///
    /// Every refusal that names a target uses this one form in both of its
    /// slots. An earlier pass wrote "a thread {}s itself" and got "applys",
    /// which is why the frames below never inflect it.
    fn word(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Cancel => "cancel",
            Self::Apply => "apply a change from",
            Self::Discard => "discard a change from",
            Self::AnswerQuestion => "answer a question card on",
            Self::Continue => "restart the turn on",
            Self::CreateTopThread => "create a top-thread beside",
        }
    }

    /// The act as a noun phrase, for a refusal with no thread to point at.
    ///
    /// [`Self::word`] needs a target to follow it, and two refusals have none:
    /// a verb aimed at the workspace root, and a caller with no subtree at all.
    fn act(self) -> &'static str {
        match self {
            Self::Archive => "archiving a thread",
            Self::Cancel => "cancelling a turn",
            Self::Apply => "applying a change",
            Self::Discard => "discarding a change",
            Self::AnswerQuestion => "answering a question card",
            Self::Continue => "restarting a turn",
            Self::CreateTopThread => "creating a top-thread",
        }
    }
}

/// Why a clause-4 verb was refused. Every branch is a refusal: none falls
/// back to a narrower target, and none reports success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ThreadReachError {
    /// The target is neither the caller nor one of its descendants, and the
    /// caller carries no standing instruction from the workspace owner.
    /// Deliberately one variant for a sibling, a parent and an unknown thread,
    /// exactly as `ChildFollowUpError::NotYourChild` is: the caller learns the
    /// thread is out of reach, and nothing about whose it is.
    OutOfReach { target: Uuid, verb: ThreadReachVerb },
    /// A verb aimed at the workspace root rather than at a thread, by a caller
    /// carrying no standing instruction. Creating a top-thread is the case:
    /// the root is nobody's subtree, so only the owner reaches it.
    NoStandingInstruction(ThreadReachVerb),
    /// A verified subprocess with no thread of its own, and not a trigger's
    /// fire either. It has no subtree to scope to and no turn to carry an
    /// instruction, so it is refused rather than handed the whole workspace.
    NoCallerThread(ThreadReachVerb),
    /// A thread-bound caller asked to cancel with no `thread_id`, which stops
    /// every thread in the workspace. Refused rather than silently reread as
    /// "cancel yourself": a caller that meant itself can say so, and one that
    /// meant everything must not get it without the owner behind it.
    UnscopedCancel,
    /// The ancestor chain could not be read, so the call fails closed. That
    /// costs the user nothing: only a token-bearing caller runs that query, so
    /// a database hiccup can never reach the Archive or Stop button.
    Unverifiable(String),
}

/// What a thread is told about the authority it does not have. Appended to
/// every refusal below, so no site can spell the remedy its own way.
///
/// It names the *workspace owner* and not a parent thread. Most threads in a
/// workspace have none, so the old sentence sent the majority of callers to
/// something with no row (ADR 0168 clause 1).
const OWNERS_TO_PRESS: &str = "Anything wider than your own subtree is the workspace owner's \
     to press, and a thread may press it only while carrying their standing \
     instruction: a turn they opened, or a trigger firing they authorized. \
     Report what you found and let the owner decide.";

impl ThreadReachError {
    /// HTTP status, beside the taxonomy so neither handler's mapping can drift
    /// from it.
    pub(crate) fn status_code(&self) -> StatusCode {
        match self {
            Self::OutOfReach { .. }
            | Self::NoStandingInstruction(_)
            | Self::NoCallerThread(_)
            | Self::UnscopedCancel => StatusCode::FORBIDDEN,
            Self::Unverifiable(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Machine-readable slug for the archive route, whose rejection bodies are
    /// `{reason, ...}` rather than `{error}`.
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Self::OutOfReach { .. } => "out_of_reach",
            Self::NoStandingInstruction(_) => "no_standing_instruction",
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
                "Thread {target} is out of your reach, so you cannot {} it. On its own \
                 authority a thread can only {} itself and the sub-threads below it. \
                 {OWNERS_TO_PRESS}",
                verb.word(),
                verb.word()
            ),
            Self::NoStandingInstruction(verb) => write!(
                f,
                "This request is {}, which aims at the workspace root rather than at a \
                 thread. The root is nobody's subtree. {OWNERS_TO_PRESS}",
                verb.act()
            ),
            Self::NoCallerThread(verb) => write!(
                f,
                "This request comes from a Lucidos subprocess with no thread of its \
                 own, so {} has no subtree to stay inside. {OWNERS_TO_PRESS}",
                verb.act()
            ),
            Self::UnscopedCancel => write!(
                f,
                "Cancelling with no thread_id stops every thread in the workspace, \
                 which is the user's own Stop. Name a thread instead: your own id, or \
                 one of the sub-threads below it. {OWNERS_TO_PRESS}"
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

/// The refusal as the `{"error": msg}` body an `ApiError` handler returns, so a
/// gated route writes `?` rather than re-deriving the status and the text.
///
/// Spelling that mapping per site is one status lookup away from a route
/// answering 500 where its neighbours answer 403. The archive route keeps its
/// own mapping: its rejection bodies are `{reason, ...}`, which is what
/// [`ThreadReachError::reason`] is for.
impl From<ThreadReachError> for super::error::ApiError {
    fn from(e: ThreadReachError) -> Self {
        Self::new(e.status_code(), e.to_string())
    }
}

/// The ladder, and only the ladder: is `target` the caller itself, or one of
/// its descendants? Clause 3, saying nothing about clause 5.
///
/// Self is answered with no query at all. Otherwise one recursive CTE walks
/// from the target UPWARD, so the cost is the depth of the tree rather than its
/// width: a parent of ten children pays what a parent of one pays.
///
/// A bool rather than a refusal, because [`refuse_without_authority`] asks a
/// second question after a `false` and must not read it as an answer yet. An
/// `Err` stays an `Err`: a ladder nobody could read must not fall through to
/// the wider path.
///
/// Takes the pool rather than `&self`, for the reason
/// `authorize_child_follow_up` does: the whole check is one query and a ladder,
/// so it is testable without standing up an engine.
async fn authorize_thread_reach(
    pool: &sqlx::PgPool,
    caller_thread_id: Uuid,
    target_thread_id: Uuid,
) -> Result<bool, ThreadReachError> {
    if caller_thread_id == target_thread_id {
        return Ok(true);
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

    Ok(reachable)
}

/// Refuse a clause-4 verb this caller has no authority for.
///
/// Two questions in order, and the cheap one first. Inside its own subtree a
/// thread acts on its own authority, which costs one query at most (clause 3).
/// Anything wider is the owner's button, and only a standing instruction lets
/// the caller press it (clause 5).
///
/// `target` is optional because two verbs aim at no single thread: `POST
/// /api/v1/chat/cancel` with no thread id means "everything", and creating a
/// top-thread means "at the root". Both are wider than any subtree, so both
/// take the standing-instruction path.
///
/// Runs BEFORE the handler writes anything, so a refusal leaves no half-applied
/// cascade, no merged branch and no cancel-stamped question card behind.
///
/// ## A caller presenting no token keeps its reach
///
/// That is the user's own device, the local API surface and the e2e suites,
/// and it is the answer `threads::actions::refuse_event_waits_for_another_thread`
/// already gives. What this buys is that a caller presenting its credential is
/// bound by it.
///
/// The residual it leaves is narrower than it was. A subprocess dropping its
/// token used to read as an ordinary local caller. `api::mutating_gate` now
/// refuses one presenting NO credential at all (ADR 0169), and every clause-4
/// verb is a mutating method, so that layer answers first. What is left is a
/// caller holding one of the other three credentials. Each names a device or
/// the machine rather than a thread, so none has a reach to weigh.
pub(in crate::api) async fn refuse_without_authority(
    pool: &sqlx::PgPool,
    headers: &HeaderMap,
    target: Option<Uuid>,
    verb: ThreadReachVerb,
) -> Result<(), ThreadReachError> {
    let SubprocessOrigin::Subprocess {
        source_thread_id,
        emitting_trigger_id,
        ..
    } = crate::api::actor::subprocess_origin(headers)
    else {
        return Ok(());
    };
    weigh_authority(
        pool,
        source_thread_id,
        emitting_trigger_id.as_deref(),
        target,
        verb,
    )
    .await
}

/// [`refuse_without_authority`], for a caller that is already IN the thread.
///
/// An LLM tool runs in-process and carries no headers, so it names its own
/// thread instead of presenting a token for one. The rule below is the same
/// one, which is the point: Apply reached by a tool and Apply reached by a
/// route must not answer differently.
///
/// No trigger id, because that field exists to name a fire that has no thread
/// of its own. A tool call always has one, and a trigger thread's own turn
/// carries the firing already.
pub(crate) async fn refuse_thread_without_authority(
    pool: &sqlx::PgPool,
    caller_thread_id: Uuid,
    target: Option<Uuid>,
    verb: ThreadReachVerb,
) -> Result<(), ThreadReachError> {
    weigh_authority(pool, Some(caller_thread_id), None, target, verb).await
}

/// The rule itself, over the two authenticated facts about the caller.
async fn weigh_authority(
    pool: &sqlx::PgPool,
    source_thread_id: Option<Uuid>,
    emitting_trigger_id: Option<&str>,
    target: Option<Uuid>,
    verb: ThreadReachVerb,
) -> Result<(), ThreadReachError> {
    // Clause 3. A threadless caller has no subtree, so it skips straight to the
    // owner's question below.
    if let (Some(caller), Some(target)) = (source_thread_id, target) {
        if authorize_thread_reach(pool, caller, target).await? {
            return Ok(());
        }
    }

    // Clause 5. The act is wider than the caller's own authority, so the only
    // thing that can carry it is the owner's standing instruction.
    if carries_standing_instruction(pool, source_thread_id, emitting_trigger_id).await {
        crate::log!(
            "[ThreadReach] Allowed on the owner's standing instruction: caller {:?} may {} {:?}",
            source_thread_id,
            verb.word(),
            target
        );
        return Ok(());
    }

    crate::log!(
        "[ThreadReach] Refused: caller {:?} tried to {} {:?} with no standing instruction",
        source_thread_id,
        verb.word(),
        target
    );
    Err(match (source_thread_id, target, verb) {
        (None, _, verb) => ThreadReachError::NoCallerThread(verb),
        (Some(_), Some(target), verb) => ThreadReachError::OutOfReach { target, verb },
        (Some(_), None, ThreadReachVerb::Cancel) => ThreadReachError::UnscopedCancel,
        (Some(_), None, verb) => ThreadReachError::NoStandingInstruction(verb),
    })
}

#[cfg(test)]
#[path = "thread_reach_tests.rs"]
mod tests;
