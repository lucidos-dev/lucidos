use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseEstimate {
    pub typical_secs: u64,
    pub runs: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyEstimates {
    pub hardening: Option<PhaseEstimate>,
    pub resolving_conflict: Option<PhaseEstimate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApplyFailure {
    pub change_id: Uuid,
    pub error: String,
}
