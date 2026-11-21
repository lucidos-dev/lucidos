use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MemorySource {
    #[serde(rename = "artifact")]
    Artifact { path: String, commit: String },
    #[serde(rename = "event")]
    Event { id: Uuid },
}

/// A memory entry a user correction removed: its text, its entities, and the
/// source it was extracted from. A rebuild re-extracts under a new id but the
/// same source, which is what lets a replay find the entry again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectedMemory {
    pub summary: String,
    #[serde(default)]
    pub entities: Vec<String>,
    pub source: MemorySource,
}

impl From<&MemoryEntry> for CorrectedMemory {
    fn from(entry: &MemoryEntry) -> Self {
        Self {
            summary: entry.summary.clone(),
            entities: entry.entities.clone(),
            source: entry.source.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: Uuid,
    pub source: MemorySource,
    pub topic: String,
    pub summary: String,
    pub importance: f32,
    pub entities: Vec<String>,
    /// Timestamp from the source event or artifact (when the content was originally created)
    pub src_created_at: DateTime<Utc>,
    /// Timestamp when this memory entry row was inserted/updated
    pub created_at: DateTime<Utc>,
}

/// Where the background loader has got to with the embedding model. Lives on
/// the slot rather than beside it so "the embedder is installed" and "the
/// loader says it is still downloading" cannot disagree: `EmbedderSlot::install`
/// is the only way to fill the slot and it sets [`Ready`](Self::Ready) itself.
///
/// In-memory only. A restart simply re-runs the load and re-derives this, so it
/// is runtime status, not durable state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EmbeddingModelLoadState {
    /// Cache is cold: bytes are being fetched from the model hub. `total_bytes`
    /// is what is known so far (see `model_download::DownloadFrame`).
    Downloading {
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    /// The ONNX session is being built, or the loader is checking the cache.
    /// This also covers another engine filling the shared cache.
    /// No failure backoff applies here (see `embedder_retry::delay_before_pass`).
    Loading,
    /// Installed. Memory search, extraction and semantic thread search are live.
    Ready,
    /// A fetch-class failure (offline, hub blocked). The loader is backing off
    /// and will try again; `attempt` counts the tries so far.
    Waiting { attempt: u32 },
    /// Terminal. The loader has stopped trying, and only a fix plus a restart
    /// will change that.
    Failed { message: String },
}

impl EmbeddingModelLoadState {
    /// Whether the loader has stopped for good. Callers use this to tell a
    /// temporary degradation ("not yet") from a permanent one ("not going to").
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// The embedding model's current status, as both the REST snapshot
/// (`GET /api/v1/memory/embedding-model-status`) and the
/// `EmbeddingModelStatusChanged` SSE payload. One shape for both so a client
/// that loads mid-download and one that watches the stream read the same thing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingModelStatus {
    /// Which model this is about, so the reading is unambiguous after a config
    /// change.
    pub model_id: String,
    pub load_state: EmbeddingModelLoadState,
}
