//! The *expected wake*: when a waiting thread expects to wake next, and who
//! said so. `WakeExpected` carries it, and the keep-alive plans its pings
//! from the newest one (ADR 0603).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Who stated an expected wake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeSource {
    /// The thread's own agent, through `expect_wake` or `lucidos expect-wake`.
    Agent,
    /// The *expected wake decision*, a background model call at turn end.
    Decision,
}

/// One time bucket of an expected wake's distribution: the odds the thread
/// wakes by `by`, and after the bucket before it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WakeBucket {
    pub by: DateTime<Utc>,
    pub probability: f64,
}

/// The expected wake a keep-alive run plans from, as its status shows it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedWake {
    pub at: DateTime<Utc>,
    pub source: WakeSource,
}
