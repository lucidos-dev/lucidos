use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bounds<T> {
    pub low: T,
    pub high: T,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffortCost {
    pub effort: String,
    pub usable_secs: Bounds<u64>,
    pub complete_secs: Bounds<u64>,
    pub backfill_usd: Bounds<f64>,
    pub backfill_usd_central: f64,
    pub daily_usd: Bounds<f64>,
    pub daily_usd_central: f64,
    pub turn_usd: Bounds<f64>,
    pub turn_usd_central: f64,
    pub cap_usd: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BackfillProgress {
    pub done: usize,
    pub total: usize,
    pub done_milli: u64,
    pub nodes_done: u64,
    pub nodes_total: u64,
    pub waiting_for_model: bool,
    pub retrying: bool,
    #[serde(default)]
    pub failing_model: Option<String>,
    #[serde(default)]
    pub spend_cap_held_until: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub spend_cap_resets_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ready: bool,
    pub spent_usd: Option<f64>,
    pub cap_usd: Option<f64>,
    pub next_cap_usd: Option<f64>,
    pub held: bool,
    /// INVARIANT: The order of the frame that announced this count, rising across one
    /// engine's life. A snapshot carries the last frame's order, so a client
    /// drops a frame its snapshot already counted.
    pub seq: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartEstimate {
    pub model: String,
    pub effort: Option<String>,
    pub calls: Bounds<u64>,
    pub cost: Option<EffortCost>,
}
