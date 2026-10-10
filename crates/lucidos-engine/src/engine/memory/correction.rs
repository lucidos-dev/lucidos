//! User corrections to long-term memory: the `MemoryCorrected` event that
//! records one, and the rebuild replay that re-applies it.
//!
//! The event is the authority. A rebuild re-extracts facts from the same
//! source events, so without it every corrected fact comes back.

use crate::core::EventRow;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::memory::{
    cosine_similarity, CorrectedMemory, EmbeddingProvider, MemoryEntry, MemorySource, PgVectorIndex,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use super::scoring::{MEMORY_DEDUP_CANDIDATES, MEMORY_DEDUP_THRESHOLD, MEMORY_SUPERSEDE_THRESHOLD};

pub(crate) const MEMORY_CORRECTED: &str = "MemoryCorrected";

/// How close a re-extracted entry must sit to the one the user removed. High,
/// because the replay has no LLM to confirm a match: it must only ever catch
/// the same fact again, never a neighbour of it.
const REPLAY_SIMILARITY: f32 = 0.85;

const CORRECTION_TOPIC: &str = "Memory Correction";

/// One of the four built-in v5 namespaces, the same one the other derived-id
/// sites use (`core::image_described_backfill`, `core::aux_context_backfill`).
const CORRECTION_NAMESPACE: Uuid = Uuid::NAMESPACE_OID;

/// Record a correction before anything is deleted, and return its event id.
///
/// Recording first makes the event the checkpoint: if a delete then fails, the
/// next rebuild still applies the correction the user asked for.
pub(crate) async fn record_correction(
    bus: &EventBus,
    search_query: &str,
    wrong_fact: &str,
    removed: Vec<CorrectedMemory>,
    correction: Option<&str>,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let emitted = bus
        .emit(BusEvent::System(SystemEvent::MemoryCorrected {
            search_query: search_query.to_string(),
            wrong_fact: wrong_fact.to_string(),
            removed,
            correction: correction.map(str::to_string),
            recorded_at: Utc::now(),
        }))
        .await?;
    emitted
        .map(|r| r.event_id)
        .ok_or_else(|| "MemoryCorrected was not persisted".into())
}

/// Write the corrected fact under the identity its replay will reuse, so a
/// rebuild upserts this row instead of adding a second copy.
pub(crate) async fn index_correction(
    index: &PgVectorIndex,
    embedder: &dyn EmbeddingProvider,
    event_id: Uuid,
    correction_text: &str,
    src_created_at: DateTime<Utc>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let embedding = embedder.embed(correction_text).await?;
    let (fact_id, source) = correction_entry_identity(event_id);
    index
        .index_entry(
            fact_id,
            &source,
            CORRECTION_TOPIC,
            correction_text,
            0.8,
            &[],
            &embedding,
            embedder.model_id(),
            src_created_at,
            crate::memory::EXTRACTOR_VERSION,
        )
        .await
}

/// What the replay reads from a `MemoryCorrected` payload. Current rows wrap
/// it in the `SystemEvent` `data` envelope and carry `removed`. Rows from the
/// retired factory are flat and carry only `deleted_summaries`.
#[derive(Deserialize)]
struct RecordedCorrection {
    #[serde(default)]
    removed: Vec<CorrectedMemory>,
    #[serde(default)]
    deleted_summaries: Vec<String>,
    #[serde(default)]
    correction: Option<String>,
    #[serde(default)]
    recorded_at: Option<DateTime<Utc>>,
}

impl RecordedCorrection {
    fn from_row(event: &EventRow) -> Option<Self> {
        let payload = event.payload.get("data").unwrap_or(&event.payload);
        serde_json::from_value(payload.clone()).ok()
    }
}

/// When a replay may remove an entry. Only one that came back: written after
/// the correction, from a source that predates it. Anything already there when
/// the user corrected was judged then, and a fact stated again afterwards is
/// new information, not the wrong fact.
#[derive(Clone, Copy)]
struct ReplayWindow {
    /// The event's `created`, on the database clock that stamps a source event.
    corrected_at: DateTime<Utc>,
    /// The engine clock that stamps `memory_entries.created_at`.
    recorded_at: DateTime<Utc>,
}

impl ReplayWindow {
    fn admits(&self, entry: &MemoryEntry) -> bool {
        entry.src_created_at < self.corrected_at && entry.created_at > self.recorded_at
    }
}

/// Rebuild Phase 3: re-apply every correction, oldest first.
pub(crate) async fn replay_corrections(
    index: &PgVectorIndex,
    embedder: &dyn EmbeddingProvider,
    corrections: &[EventRow],
) {
    if corrections.is_empty() {
        return;
    }
    log!(@Memory, "Phase 3: Replaying {} memory corrections", corrections.len());
    for event in corrections {
        let Some(recorded) = RecordedCorrection::from_row(event) else {
            log!(@Memory, "Correction replay: unreadable payload on event {}", event.id);
            continue;
        };
        let window = ReplayWindow {
            corrected_at: event.created,
            recorded_at: recorded.recorded_at.unwrap_or(event.created),
        };
        let matches = if recorded.removed.is_empty() {
            legacy_matches(index, embedder, &recorded.deleted_summaries).await
        } else {
            re_extracted_matches(index, embedder, &recorded.removed, window).await
        };
        delete_returned(index, matches, window).await;

        if let Some(correction_text) = recorded.correction.as_deref().filter(|c| !c.is_empty()) {
            if let Err(e) =
                index_correction(index, embedder, event.id, correction_text, event.created).await
            {
                log!(@Memory, "Failed to re-add correction: {}", e);
            }
        }
    }
}

/// Entries that say what a removed one said, found two ways. The same source
/// catches a re-extraction, since a rebuild keeps the source and not the id.
/// The folding rule catches copies from other sources that live extraction had
/// merged into the removed entry, but a concurrent rebuild wrote separately.
async fn re_extracted_matches(
    index: &PgVectorIndex,
    embedder: &dyn EmbeddingProvider,
    removed: &[CorrectedMemory],
    window: ReplayWindow,
) -> Vec<MemoryEntry> {
    let mut matches = Vec::new();
    for item in removed {
        let removed_embedding = match embedder.embed(&item.summary).await {
            Ok(e) => e,
            Err(e) => {
                log!(@Memory, "Correction replay: embedding failed: {}", e);
                continue;
            }
        };
        matches.extend(same_source_match(index, embedder, item, &removed_embedding, window).await);
        matches.extend(folded_matches(index, embedder, item, &removed_embedding).await);
    }
    matches
}

/// Entries live extraction would have folded into the removed one: a near
/// duplicate it skips, or a close entry sharing an entity that it supersedes.
///
/// Extraction weighs a batch only against what was stored before it, so the
/// removed entry's own source is left to `same_source_match`. A correction's
/// replacement never went through extraction, so it is never folded.
async fn folded_matches(
    index: &PgVectorIndex,
    embedder: &dyn EmbeddingProvider,
    item: &CorrectedMemory,
    removed_embedding: &[f32],
) -> Vec<MemoryEntry> {
    let similar = match index
        .find_similar(
            removed_embedding,
            MEMORY_SUPERSEDE_THRESHOLD,
            MEMORY_DEDUP_CANDIDATES,
            embedder.model_id(),
        )
        .await
    {
        Ok(similar) => similar,
        Err(e) => {
            log!(@Memory, "Correction replay: similarity lookup failed: {}", e);
            return Vec::new();
        }
    };
    let folded = similar.into_iter().filter(|hit| {
        hit.similarity >= MEMORY_DEDUP_THRESHOLD
            || hit.entities.iter().any(|e| item.entities.contains(e))
    });
    entries_by_id(index, folded.map(|hit| hit.id))
        .await
        .into_iter()
        .filter(|entry| entry.source != item.source && entry.topic != CORRECTION_TOPIC)
        .collect()
}

/// The removed fact as its own source re-extracted it: the closest entry from
/// that source that came back, if it sits within `REPLAY_SIMILARITY`. Only the
/// closest, since one extraction also yields sibling facts that stood beside it.
async fn same_source_match(
    index: &PgVectorIndex,
    embedder: &dyn EmbeddingProvider,
    item: &CorrectedMemory,
    removed_embedding: &[f32],
    window: ReplayWindow,
) -> Option<MemoryEntry> {
    let candidates = match serde_json::to_value(&item.source) {
        Ok(source) => index.entries_for_source(&source).await,
        Err(e) => Err(e.into()),
    };
    let candidates: Vec<MemoryEntry> = match candidates {
        Ok(c) => c.into_iter().filter(|entry| window.admits(entry)).collect(),
        Err(e) => {
            log!(@Memory, "Correction replay: source lookup failed: {}", e);
            return None;
        }
    };
    if candidates.is_empty() {
        return None;
    }
    let texts: Vec<&str> = candidates.iter().map(|e| e.summary.as_str()).collect();
    let embeddings = match embedder.embed_batch(&texts).await {
        Ok(e) => e,
        Err(e) => {
            log!(@Memory, "Correction replay: embedding failed: {}", e);
            return None;
        }
    };
    candidates
        .into_iter()
        .zip(embeddings.iter())
        .map(|(entry, embedding)| (entry, cosine_similarity(removed_embedding, embedding)))
        .filter(|(_, similarity)| *similarity >= REPLAY_SIMILARITY)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(entry, _)| entry)
}

async fn entries_by_id(index: &PgVectorIndex, ids: impl Iterator<Item = Uuid>) -> Vec<MemoryEntry> {
    let mut entries = Vec::new();
    for id in ids {
        match index.get_by_id(id).await {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => {}
            Err(e) => log!(@Memory, "Correction replay: get_by_id failed: {}", e),
        }
    }
    entries
}

/// Rows from the retired factory name no source: match on the removed text
/// alone.
async fn legacy_matches(
    index: &PgVectorIndex,
    embedder: &dyn EmbeddingProvider,
    deleted_summaries: &[String],
) -> Vec<MemoryEntry> {
    let mut matches = Vec::new();
    for summary in deleted_summaries {
        let similar = match embedder.embed(summary).await {
            Ok(embedding) => {
                index
                    .find_similar(&embedding, REPLAY_SIMILARITY, 20, embedder.model_id())
                    .await
            }
            Err(e) => Err(e),
        };
        match similar {
            Ok(similar) => {
                matches.extend(entries_by_id(index, similar.into_iter().map(|hit| hit.id)).await)
            }
            Err(e) => {
                log!(@Memory, "Correction replay (legacy): lookup failed for '{}': {}", &summary[..summary.floor_char_boundary(50)], e);
            }
        }
    }
    matches
}

async fn delete_returned(index: &PgVectorIndex, matches: Vec<MemoryEntry>, window: ReplayWindow) {
    let mut ids: Vec<Uuid> = matches
        .into_iter()
        .filter(|entry| window.admits(entry))
        .map(|entry| entry.id)
        .collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return;
    }
    match index.delete_many(&ids).await {
        Ok(n) => log!(@Memory, "Correction replay: deleted {} re-extracted entries", n),
        Err(e) => log!(@Memory, "Correction replay: delete_many failed: {}", e),
    }
}

/// The id and source of the entry a correction writes. The id derives from
/// the event, so every replay upserts one row rather than adding a copy. The
/// source is the event itself, so `sources_indexed` sees the row and "View
/// source" resolves it.
fn correction_entry_identity(event_id: Uuid) -> (Uuid, MemorySource) {
    let key = format!("memory-correction:{event_id}");
    (
        Uuid::new_v5(&CORRECTION_NAMESPACE, key.as_bytes()),
        MemorySource::Event { id: event_id },
    )
}

#[cfg(test)]
#[path = "correction_tests.rs"]
mod tests;
