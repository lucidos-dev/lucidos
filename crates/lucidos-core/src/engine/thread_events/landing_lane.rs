//! Landing lane state carried by system events.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A change in the lane: exactly the commit the lane lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneMember {
    pub change_id: Uuid,
    pub snapshot_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteLaneSource>,
}

/// The advertised tag and the source's brief, saved with open batch members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteLaneSource {
    pub oid: String,
    pub title: String,
    pub summary: String,
    pub branch: String,
    pub gateway: Option<String>,
}

/// What ended a hold on clean arrivals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneHoldRelease {
    BatchLanded,
    BatchFailed,
    /// The hold reached its cap with the batch still open.
    CapReached,
}

/// Which host lands changes for this repository.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LandingSetting {
    #[default]
    Here,
    AnotherHost,
}

impl LandingSetting {
    pub const VALUES: [&'static str; 2] = ["here", "another-host"];

    pub fn resolve(value: &str) -> Self {
        match value {
            "another-host" => Self::AnotherHost,
            _ => Self::Here,
        }
    }
}

/// What the landing lane holds right now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneSnapshot {
    #[serde(default)]
    pub setting: LandingSetting,
    /// The lane batch resolving now, if any. At most one is open.
    pub open_batch: Option<OpenBatchSnapshot>,
    /// Changes waiting for the next batch, in landing order.
    pub waiting: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_members: Vec<RemoteLaneEntry>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multiple_landers: bool,
    /// The queue waits for an Apply All press to hand over the rest of its
    /// changes, with no batch open (ADR 0480).
    pub gathering: bool,
    /// Why the repository's `origin` could not take the last
    /// landing. The lane holds what it could not push and retries (ADR 0589).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for_origin: Option<String>,
    /// The batch that holds clean arrivals off `main` while it hardens again
    /// after a catch-up, and the arrivals it holds (ADR 0694).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrival_hold: Option<ArrivalHoldSnapshot>,
}

/// Origin metadata displayed for a remote member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteLaneEntry {
    pub change_id: Uuid,
    pub title: String,
    pub gateway: Option<String>,
}

/// The open hold on clean arrivals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrivalHoldSnapshot {
    pub batch_id: Uuid,
    /// The held changes, in arrival order. Each also appears in `waiting`.
    pub arrivals: Vec<Uuid>,
}

/// The open lane batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenBatchSnapshot {
    pub batch_id: Uuid,
    /// The coding-agent thread that resolves the batch.
    pub lane_thread_id: Uuid,
    /// The members still to land, in batch order.
    pub members: Vec<Uuid>,
    pub step: LaneStep,
}

/// What the open batch is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneStep {
    /// Its lane thread works on the merge conflicts, or has yet to finish.
    Resolving,
    /// Its lane thread finished cleanly, and the lane is moving `main`.
    Landing,
}

pub async fn lane_batch_of_thread<'e>(
    pool: impl sqlx::PgExecutor<'e>,
    thread_id: Uuid,
) -> Result<Option<(String, String, bool)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT s.aggregate_id, s.payload->'data'->>'base_sha', EXISTS ( \
             SELECT 1 FROM events e \
             WHERE e.aggregate = 'landing_lane' AND e.aggregate_id = s.aggregate_id \
               AND e.event_type IN ('LaneBatchLanded', 'LaneBatchFailed')) \
         FROM events s \
         WHERE s.event_type = 'LaneBatchStarted' \
           AND s.payload->'data'->>'lane_thread_id' = $1 \
         ORDER BY s.sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .fetch_optional(pool)
    .await
}

/// Whether `thread_id` is the lane thread of a batch that landed or failed:
/// a closed lane thread, which runs no session again (ADR 0662).
pub async fn lane_batch_ended<'e>(
    pool: impl sqlx::PgExecutor<'e>,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    Ok(lane_batch_of_thread(pool, thread_id)
        .await?
        .is_some_and(|(batch, _, ended)| ended && Uuid::parse_str(&batch).is_ok()))
}
