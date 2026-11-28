use super::integration::LaneWorktree;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::{EngineReason, MessageOrigin, ThreadEvent};
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneDuty {
    pub batch_id: Uuid,
    pub worktree: LaneWorktree,
    pub(crate) base_sha: String,
}

impl LaneDuty {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneThreadEnd {
    Completed,
    Stopped,
    Failed(String),
}

pub const LANE_THREAD_CLOSED: &str = "The lane thread was archived, deleted or discarded.";

/// INVARIANT: The persisted events that close a thread for good: an archive and a
/// delete. [`lane_thread_closures`] reads the same two.
pub(super) const LANE_THREAD_CLOSING_EVENT_TYPES: [&str; 2] = ["ThreadArchived", "ThreadsDeleted"];

/// INVARIANT: The threads `event` closes, each with who closed it. The lane fails the
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

/// INVARIANT: Whether `thread_id` was archived or deleted. A lane thread's id is fresh
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

/// INVARIANT: What a lane-thread run that never reached its completion tells the lane.
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum LaneThreadRole {
    NotLane,
    Resolving(LaneDuty),
    Ended {
        batch_id: Uuid,
        worktree: LaneWorktree,
    },
}

pub(crate) const LANE_THREAD_ENDED: &str = "This landing batch has ended. \
    Apply any pending source changes again to start a new landing batch.";

pub(crate) use crate::engine::thread_events::landing_lane::lane_batch_ended;
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

pub(super) fn lane_turn_reason() -> EngineReason {
    EngineReason::MergeConflict
}

pub(crate) const LANE_TURN_OUTLIVED_BATCH: &str = "The lane batch ended before its turn started.";

/// INVARIANT: Whether `request_event_id` is a turn the lane sent. A lane thread is
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

/// INVARIANT: The lane duty a run on `thread_id` carries, if the thread is the lane
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
            .or_unknown(true);
            if still_there {
                super::integration::retire_lane_worktree(pool, repo_root, &worktree).await;
            }
            Err(LANE_THREAD_ENDED.into())
        }
    }
}

/// INVARIANT: Stale cleanup never proposes a historical lane conversation's work.
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

pub(super) async fn lane_thread_state(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    lane_branch: &str,
) -> Result<super::recovery::LaneThreadState, sqlx::Error> {
    use super::recovery::LaneThreadState;
    // INVARIANT: First: a delete leaves no event on the thread, so it reads unopened.
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
