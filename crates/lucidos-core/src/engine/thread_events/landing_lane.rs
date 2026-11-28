use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneMember {
    pub change_id: Uuid,
    pub snapshot_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteLaneSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteLaneSource {
    pub oid: String,
    pub title: String,
    pub summary: String,
    pub branch: String,
    pub gateway: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneHoldRelease {
    BatchLanded,
    BatchFailed,
    CapReached,
}

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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneSnapshot {
    #[serde(default)]
    pub setting: LandingSetting,
    pub open_batch: Option<OpenBatchSnapshot>,
    pub waiting: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_members: Vec<RemoteLaneEntry>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multiple_landers: bool,
    pub gathering: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrival_hold: Option<ArrivalHoldSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteLaneEntry {
    pub change_id: Uuid,
    pub title: String,
    pub gateway: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrivalHoldSnapshot {
    pub batch_id: Uuid,
    /// INVARIANT: The held changes, in arrival order. Each also appears in `waiting`.
    pub arrivals: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenBatchSnapshot {
    pub batch_id: Uuid,
    pub lane_thread_id: Uuid,
    pub members: Vec<Uuid>,
    pub step: LaneStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneStep {
    Resolving,
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

pub async fn lane_batch_ended<'e>(
    pool: impl sqlx::PgExecutor<'e>,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    Ok(lane_batch_of_thread(pool, thread_id)
        .await?
        .is_some_and(|(batch, _, ended)| ended && Uuid::parse_str(&batch).is_ok()))
}
