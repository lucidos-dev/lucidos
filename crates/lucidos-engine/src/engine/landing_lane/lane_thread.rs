//! The *lane thread*: the coding-agent thread a lane batch resolves in.
//!
//! The lane opens it with a fresh thread id it records on `LaneBatchStarted`.
//! Any run on that thread while the batch is open carries the batch: the
//! first turn, a follow-up the user types, a catch-up turn, a Continue and a
//! switch resume alike. Its completion reports to the lane, which lands or
//! fails the batch whole.

use super::integration::LaneWorktree;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::{EngineReason, MessageOrigin, ThreadEvent};
use std::path::Path;
use uuid::Uuid;

/// What a run on a lane thread carries: its batch, and where it resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneDuty {
    pub batch_id: Uuid,
    pub worktree: LaneWorktree,
    /// Where the integration branch starts.
    pub(crate) base_sha: String,
}

impl LaneDuty {
    /// The lane worktree, made again if something removed it, so a run never
    /// starts in a missing directory.
    pub async fn ensure_worktree(&self, repo_root: &Path, workspace: &Path) -> Result<(), String> {
        super::integration::ensure_lane_worktree(
            repo_root,
            workspace,
            &self.worktree,
            &self.base_sha,
        )
        .await
    }
}

/// How a lane thread's run ended. Every run that carries a batch gives the
/// lane exactly one of these, however it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneThreadEnd {
    /// The turn ended cleanly with nothing left unmerged. The lane checks the
    /// integration branch and lands it.
    Completed,
    /// The user stopped the turn. The batch stays open with its worktree,
    /// and the thread's next turn (a follow-up or a Continue) carries it on.
    Stopped,
    /// A failed turn, or a run that could not start or prepare its
    /// worktree. The batch fails whole.
    Failed(String),
}

/// Why a batch failed when its lane thread was archived, deleted or
/// discarded.
pub const LANE_THREAD_CLOSED: &str = "The lane thread was archived, deleted or discarded.";

/// The persisted events that close a thread for good: an archive and a
/// delete. [`lane_thread_closures`] reads the same two.
pub(super) const LANE_THREAD_CLOSING_EVENT_TYPES: [&str; 2] = ["ThreadArchived", "ThreadsDeleted"];

/// The threads `event` closes, each with who closed it. The lane fails the
/// open batch when one of them is its lane thread, whether or not a run is
/// live there.
pub(crate) fn lane_thread_closures(event: &BusEvent) -> Vec<(Uuid, Option<MessageOrigin>)> {
    match event {
        BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ThreadArchived,
            meta,
        } => vec![(*thread_id, meta.actor.clone())],
        BusEvent::System(SystemEvent::ThreadsDeleted {
            thread_ids, actor, ..
        }) => thread_ids.iter().map(|t| (*t, actor.clone())).collect(),
        _ => Vec::new(),
    }
}

/// Whether `thread_id` was archived or deleted. A lane thread's id is fresh
/// for its batch, so any such event came after the batch opened.
async fn lane_thread_closed(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events \
                        WHERE thread_id = $1 AND event_type = 'ThreadArchived') \
             OR EXISTS (SELECT 1 FROM events WHERE event_type = 'ThreadsDeleted' \
                        AND payload->'data'->'thread_ids' @> jsonb_build_array($1::text))",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
}

/// What a lane-thread run that never reached its completion tells the lane.
/// The completion answers for every run that got that far.
///
/// - A shutdown answers nothing: the restart rebuilds the batch, and agent
///   recovery owns the thread.
/// - A stale resume answers nothing: its caller retries at once on the same
///   claim, and the retry answers.
/// - An `Ok` with no completion is a Stop during startup, or a session that
///   ended between turns. The batch waits, as after any Stop.
/// - Any other error failed to start or prepare the run.
pub fn unanswered_run_end(outcome: Result<(), &str>, shutting_down: bool) -> Option<LaneThreadEnd> {
    if shutting_down {
        return None;
    }
    match outcome {
        Ok(()) => Some(LaneThreadEnd::Stopped),
        Err(e) if e == crate::engine::continuation_reason::STALE_RESUME_ERROR => None,
        Err(e) => Some(LaneThreadEnd::Failed(format!(
            "The lane thread could not start: {e}"
        ))),
    }
}

/// What a thread is to the lane.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LaneThreadRole {
    NotLane,
    Resolving(LaneDuty),
    /// Its batch landed or failed.
    Ended {
        batch_id: Uuid,
        worktree: LaneWorktree,
    },
}

/// Closed lane conversations retain their batch identity (ADR 0662).
pub(crate) const LANE_THREAD_ENDED: &str = "This landing batch has ended. \
    Apply any pending source changes again to start a new landing batch.";

pub(crate) use crate::engine::thread_events::landing_lane::lane_batch_ended;
/// The newest batch `thread_id` is the lane thread of: its id, its base sha,
/// and whether it landed or failed.
use crate::engine::thread_events::landing_lane::lane_batch_of_thread;

async fn lane_thread_role(
    pool: &sqlx::PgPool,
    workspace: &Path,
    thread_id: Uuid,
) -> Result<LaneThreadRole, sqlx::Error> {
    let Some((batch, base_sha, ended)) = lane_batch_of_thread(pool, thread_id).await? else {
        return Ok(LaneThreadRole::NotLane);
    };
    let Ok(batch_id) = Uuid::parse_str(&batch) else {
        return Ok(LaneThreadRole::NotLane);
    };
    let worktree = LaneWorktree::of(workspace, batch_id);
    Ok(if ended {
        LaneThreadRole::Ended { batch_id, worktree }
    } else {
        LaneThreadRole::Resolving(LaneDuty {
            batch_id,
            worktree,
            base_sha,
        })
    })
}

/// The reason every turn the lane sends its lane thread carries.
pub(super) fn lane_turn_reason() -> EngineReason {
    EngineReason::MergeConflict
}

/// Why a turn the lane sent does not run: its batch ended first.
pub(crate) const LANE_TURN_OUTLIVED_BATCH: &str = "The lane batch ended before its turn started.";

/// Whether `request_event_id` is a turn the lane sent. A lane thread is
/// fresh for its batch, so such a turn was sent for that batch alone.
async fn is_lane_turn(pool: &sqlx::PgPool, request_event_id: Uuid) -> Result<bool, sqlx::Error> {
    let origin: Option<Option<serde_json::Value>> = sqlx::query_scalar(
        "SELECT payload->'origin' FROM events \
         WHERE id = $1 AND event_type = 'MessageReceived'",
    )
    .bind(request_event_id)
    .fetch_optional(pool)
    .await?;
    Ok(origin
        .flatten()
        .and_then(|origin| serde_json::from_value::<MessageOrigin>(origin).ok())
        == Some(MessageOrigin::engine(lane_turn_reason())))
}

/// The lane duty a run on `thread_id` carries, if the thread is the lane
/// thread of an open batch. `request_event_id` is the input the run serves.
///
/// A turn the lane sent never outlives its batch: a Cancel can end the batch
/// while the turn waits for its worktree. Run anyway, it would merge every
/// member as an ordinary thread and propose them again.
///
/// A closed lane thread never becomes an ordinary worker. The run holds the
/// thread claim, so it can retire the integration branch before refusing.
pub async fn lane_duty_for_run(
    pool: &sqlx::PgPool,
    workspace: &Path,
    repo_root: &Path,
    thread_id: Uuid,
    request_event_id: Uuid,
) -> Result<Option<LaneDuty>, Box<dyn std::error::Error + Send + Sync>> {
    match lane_thread_role(pool, workspace, thread_id).await? {
        LaneThreadRole::NotLane => Ok(None),
        LaneThreadRole::Resolving(duty) => Ok(Some(duty)),
        LaneThreadRole::Ended { worktree, .. } => {
            if is_lane_turn(pool, request_event_id).await? {
                return Err(LANE_TURN_OUTLIVED_BATCH.into());
            }
            let refname = format!("refs/heads/{}", worktree.branch);
            let still_there = crate::engine::git_ops::git_answer(
                &["rev-parse", "--verify", "--quiet", &refname],
                repo_root,
            )
            .await
            // An unknown tries the removal, which refuses on its own unknown.
            .or_unknown(true);
            if still_there {
                super::integration::retire_lane_worktree(pool, repo_root, &worktree).await;
            }
            Err(LANE_THREAD_ENDED.into())
        }
    }
}

/// Stale cleanup never proposes a historical lane conversation's work.
/// Explicit Discard may still clean up a historical ordinary branch.
pub async fn lane_batch_of_stale_session(
    pool: &sqlx::PgPool,
    workspace: &Path,
    thread_id: Uuid,
    session_branch: Option<&str>,
    discard: bool,
) -> Result<Option<Uuid>, sqlx::Error> {
    Ok(match lane_thread_role(pool, workspace, thread_id).await? {
        LaneThreadRole::NotLane => None,
        LaneThreadRole::Resolving(duty) => Some(duty.batch_id),
        LaneThreadRole::Ended { batch_id, worktree } => {
            (!discard || session_branch == Some(worktree.branch.as_str())).then_some(batch_id)
        }
    })
}

/// The lane thread's state at startup, from its events. See
/// [`super::recovery::LaneThreadState`].
pub(super) async fn lane_thread_state(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    lane_branch: &str,
) -> Result<super::recovery::LaneThreadState, sqlx::Error> {
    use super::recovery::LaneThreadState;
    // First: a delete leaves no event on the thread, so it reads unopened.
    if lane_thread_closed(pool, thread_id).await? {
        return Ok(LaneThreadState::Closed);
    }
    let opened: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM events WHERE thread_id = $1)")
            .bind(thread_id)
            .fetch_one(pool)
            .await?;
    if !opened {
        return Ok(LaneThreadState::NotOpened);
    }
    let in_flight =
        crate::engine::branch_recovery_queries::branch_turn_in_flight(pool, lane_branch).await?;
    Ok(
        if in_flight == Some(false)
            && crate::engine::coding_turn_end::last_turn_ended_cleanly(pool, thread_id).await
        {
            LaneThreadState::Finished
        } else {
            LaneThreadState::Unfinished
        },
    )
}

/// The lane thread's title: what it lands.
pub(crate) fn lane_thread_title(member_titles: &[String]) -> String {
    match member_titles {
        [only] => format!("Landing: {only}"),
        all => format!("Landing {} changes", all.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lane_thread_is_titled_for_what_it_lands() {
        assert_eq!(
            lane_thread_title(&["Fix the parser".into()]),
            "Landing: Fix the parser"
        );
        assert_eq!(
            lane_thread_title(&["a".into(), "b".into(), "c".into()]),
            "Landing 3 changes"
        );
    }
}
