mod driver;
mod integration;
mod landing;
mod lane_thread;
mod recovery;
mod remote;
mod snapshot;
mod state;

pub(crate) use driver::{settle_restored_lane, LaneMsg, Submission};
#[cfg(test)]
pub(crate) use landing::{publish_clean_pass, LandingInput};
pub(crate) use lane_thread::lane_batch_ended;
pub(crate) use snapshot::LaneSnapshot;
pub use state::LaneRemoval;
pub use {
    lane_thread::lane_batch_of_stale_session, lane_thread::lane_duty_for_run,
    lane_thread::unanswered_run_end, lane_thread::LaneDuty, lane_thread::LaneThreadEnd,
    lane_thread::LANE_THREAD_CLOSED,
};

use crate::engine::change_ops::ApplyKindContext;
use std::path::Path;
use uuid::Uuid;

pub use crate::engine::thread_events::landing_lane::{LaneHoldRelease, LaneMember};

pub fn takes_landing_lane(
    kind: &ApplyKindContext,
    change_repo_root: &Path,
    lane_repo_root: &Path,
) -> bool {
    kind.is_lucidos_source()
        && crate::engine::git_ops::canonical_repo_root(change_repo_root)
            == crate::engine::git_ops::canonical_repo_root(lane_repo_root)
}

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
            remote: None,
        })
        .collect())
}

/// INVARIANT: The changes among `change_ids` the lane holds: their latest lane event is
/// `ChangeQueued`, waiting or in the open batch, and they are still pending.
/// Read from events, so it answers before the lane restores after a restart.
pub(crate) async fn lane_held_changes(
    pool: &sqlx::PgPool,
    change_ids: &[Uuid],
) -> Result<std::collections::HashSet<Uuid>, sqlx::Error> {
    if change_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let ids: Vec<String> = change_ids.iter().map(Uuid::to_string).collect();
    let held: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT c.id FROM ( \
             SELECT DISTINCT ON (payload->>'change_id') \
                    payload->>'change_id' AS change_id, event_type \
             FROM events \
             WHERE event_type IN ('ChangeQueued', 'ChangeUnqueued') \
               AND payload->>'change_id' = ANY($1) \
             ORDER BY payload->>'change_id', sequence DESC \
         ) q \
         JOIN changes c ON c.id::text = q.change_id AND c.status = 'pending' \
         WHERE q.event_type = 'ChangeQueued'",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    Ok(held.into_iter().map(|(id,)| id).collect())
}

pub(crate) const HELD_BY_LANDER: &str = "This change is held by the lander.";

pub(crate) async fn handoff_started(pool: &sqlx::PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let latest: Option<String> = sqlx::query_scalar(
        "SELECT event_type FROM events
         WHERE event_type IN ('ChangeHandoffStarted', 'ChangeUnqueued', 'ChangeApplied')
           AND payload->>'change_id' = $1
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(latest.as_deref() == Some("ChangeHandoffStarted"))
}

pub async fn queued_snapshot(
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

/// INVARIANT: The thread `change_id`'s latest `ChangeQueued` went on, when it has one.
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
