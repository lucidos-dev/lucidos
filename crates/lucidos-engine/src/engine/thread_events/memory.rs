use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::memory::{MemoryEntry, MemorySource};

/// One long-term memory the pre-turn recall injected into the turn's context,
/// as the model saw it.
///
/// The summary is copied, not referenced: a memory rebuild re-extracts every
/// entry under a new `id`, while `source` survives it. So the step detail can
/// always show what was recalled and link to where it came from.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecalledMemory {
    pub id: Uuid,
    pub topic: String,
    pub summary: String,
    /// When the source content was created, which is the date the model saw.
    pub src_created_at: DateTime<Utc>,
    pub source: MemorySource,
}

impl From<&MemoryEntry> for RecalledMemory {
    fn from(entry: &MemoryEntry) -> Self {
        Self {
            id: entry.id,
            topic: entry.topic.clone(),
            summary: entry.summary.clone(),
            src_created_at: entry.src_created_at,
            source: entry.source.clone(),
        }
    }
}
