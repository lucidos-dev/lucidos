//! Apply estimates and failure details carried by system events.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The typical duration of one phase, and how many runs it rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseEstimate {
    pub typical_secs: u64,
    pub runs: usize,
}

/// One estimate per slow phase. `None` where too few runs exist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyEstimates {
    pub hardening: Option<PhaseEstimate>,
    pub resolving_conflict: Option<PhaseEstimate>,
}

/// Per-change failure reason in an Apply All batch. Named-struct wire
/// format keeps the `failed` field of `ApplyAllBatchCompleted` self-
/// describing (tuple serialization would emit a positional JSON array
/// downstream consumers couldn't introspect).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApplyFailure {
    pub change_id: Uuid,
    pub error: String,
}
