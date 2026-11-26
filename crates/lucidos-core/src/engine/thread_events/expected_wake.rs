use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeSource {
    Agent,
    Decision,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WakeBucket {
    pub by: DateTime<Utc>,
    pub probability: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedWake {
    pub at: DateTime<Utc>,
    pub source: WakeSource,
}
