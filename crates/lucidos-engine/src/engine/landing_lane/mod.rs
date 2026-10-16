//! The *landing lane*: the one path that moves `main` for a Lucidos-source
//! change (ADR 0463).
//!
//! Apply queues the change as its exact branch commit, the *snapshot*. The lane
//! lands what merges cleanly at once, one *landing merge* per change. What
//! would conflict is set aside into one resolving *lane batch*, which its
//! *lane thread* resolves on an *integration branch* and lands whole. Later
//! arrivals that merge cleanly keep landing. While an Apply All press is open
//! the lane opens no batch, so the press's conflicts resolve in one (ADR
//! 0480). App changes keep the Apply tiers, and external-repo threads propose
//! nothing.
//!
//! - `state`: the pure queue, batch and press rules.
//! - `landing`: landing merges, built without a worktree.
//! - `integration`: a batch's integration branch, its check and its landing.
//! - `lane_thread`: the coding-agent thread that resolves a batch.
//! - `recovery`: the lane rebuilt from events after a restart.
//! - `snapshot`: the lane as clients see it.
//! - `driver`: the engine task that runs it all, one message at a time.

mod driver;
mod integration;
mod landing;
mod lane_thread;
mod recovery;
mod snapshot;
mod state;

pub(crate) use driver::{LaneMsg, Submission};
#[cfg(test)]
pub(crate) use landing::{publish_clean_pass, LandingInput};
pub(crate) use lane_thread::{
    lane_batch_of_stale_session, lane_duty_for_run, unanswered_run_end, LaneDuty, LaneThreadEnd,
    LANE_THREAD_CLOSED,
};
pub(crate) use snapshot::LaneSnapshot;
pub(crate) use state::LaneRemoval;

use crate::engine::change_ops::ApplyKindContext;
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

/// A change in the lane: exactly the commit the lane lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneMember {
    pub change_id: Uuid,
    pub snapshot_sha: String,
}

/// Whether a change lands through the lane. Only Lucidos source in the
/// engine's own repository does: the lane serves one repository.
pub(crate) fn takes_landing_lane(
    kind: &ApplyKindContext,
    change_repo_root: &Path,
    lane_repo_root: &Path,
) -> bool {
    kind.is_lucidos_source()
        && crate::engine::git_ops::canonical_repo_root(change_repo_root)
            == crate::engine::git_ops::canonical_repo_root(lane_repo_root)
}

/// The pending changes on `thread_id` whose latest lane event queued them,
/// with the snapshot each was queued at.
pub(crate) async fn queued_changes(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<Vec<LaneMember>, sqlx::Error> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT c.id, q.snapshot_sha FROM ( \
             SELECT DISTINCT ON (payload->>'change_id') \
                    payload->>'change_id' AS change_id, event_type, \
                    payload->>'snapshot_sha' AS snapshot_sha \
             FROM events \
             WHERE thread_id = $1 AND event_type IN ('ChangeQueued', 'ChangeUnqueued') \
             ORDER BY payload->>'change_id', sequence DESC \
         ) q \
         JOIN changes c ON c.id::text = q.change_id AND c.status = 'pending' \
         WHERE q.event_type = 'ChangeQueued'",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(change_id, snapshot_sha)| LaneMember {
            change_id,
            snapshot_sha,
        })
        .collect())
}

/// The snapshot `change_id` waits at, when it is queued.
pub(crate) async fn queued_snapshot(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    change_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    Ok(queued_changes(pool, thread_id)
        .await?
        .into_iter()
        .find(|m| m.change_id == change_id)
        .map(|m| m.snapshot_sha))
}

/// The thread `change_id`'s latest `ChangeQueued` went on, when it has one.
/// Its `ChangeUnqueued` goes on the same thread, or the change reads queued.
async fn queued_thread(pool: &sqlx::PgPool, change_id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT thread_id FROM events \
         WHERE event_type = 'ChangeQueued' AND payload->>'change_id' = $1 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(change_id.to_string())
    .fetch_optional(pool)
    .await
    .map(Option::flatten)
}

/// The landing merge `ChangeApplied` recorded for `change_id`, when the lane
/// landed it. `None` for an app change and for every apply before the lane.
pub(crate) async fn landing_merge_of(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    change_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT payload->>'landing_merge_sha' FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeApplied' \
           AND payload->>'change_id' = $2 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(change_id.to_string())
    .fetch_optional(pool)
    .await
    .map(Option::flatten)
}

#[cfg(test)]
#[path = "lane_tests.rs"]
mod tests;
