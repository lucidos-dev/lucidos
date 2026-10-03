//! The payload of a *thread triage* proposal (ADR 0349).

use serde::{Deserialize, Serialize};

/// One thread in a triage proposal: what was proposed, and why. `action` is a
/// `TriageAction` wire name, kept as a string so old rows always decode.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TriageProposalEntry {
    pub thread_id: uuid::Uuid,
    pub action: String,
    pub reason: String,
}
