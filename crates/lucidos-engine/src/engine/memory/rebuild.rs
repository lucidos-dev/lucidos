//! Batch / derived memory operations: LLM artifact summaries, the
//! full/incremental memory rebuild (events + artifact history + correction
//! replay), and the post-import hook. These drive the per-item indexing
//! helpers in `super::extract` in bulk.

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::{AuxCapture, LucidosEngine};
use crate::llm::provider::LlmProvider;
use crate::llm::{Message, MessageContent};
use crate::memory::{
    cosine_similarity, EmbeddingProvider, MemoryEntry, MemorySource, PgVectorIndex,
};
use std::sync::atomic::Ordering;
use uuid::Uuid;

use super::scoring::MEMORY_CORRECTION_THRESHOLD;

/// Summarise one artifact on an arbitrary provider, recording what it cost.
///
/// A free function so a stubbed provider drives it offline, which resolving
/// the engine's own provider would not allow. A file too small to be worth a
/// model call returns its one-line stand-in and spends nothing.
pub(crate) async fn summarize_on<P: LlmProvider + ?Sized>(
    provider: &P,
    path: &str,
    content: &str,
    capture: Option<&AuxCapture>,
) -> Option<String> {
    if content.len() < 50 {
        return Some(format!("Small file: {}", path));
    }

    // Truncate very large content for summarization
    let content_for_summary: String = if content.len() > 4000 {
        format!(
            "{}...\n[truncated, {} total chars]",
            content.chars().take(3500).collect::<String>(),
            content.len()
        )
    } else {
        content.to_string()
    };

    let prompt = format!(
        "Summarize this file in 1-2 sentences. Focus on what it contains and its purpose.\n\nFile: {}\n\nContent:\n{}",
        path, content_for_summary
    );
    let request_chars = prompt.chars().count();

    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text(prompt),
    }];

    match provider
        .chat(
            messages,
            vec![],
            crate::llm::ModelSelection::default(),
            None,
            None,
        )
        .await
    {
        Ok(response) => {
            if let Some(capture) = capture {
                capture
                    .record(provider.default_model(), request_chars, &response)
                    .await;
            }
            response.content
        }
        Err(e) => {
            log!("[Memory] Failed to generate summary for {}: {}", path, e);
            None
        }
    }
}

impl LucidosEngine {
    /// Generate a summary for an artifact using the LLM.
    ///
    /// `thread_id` anchors the capture. The only caller is the `import_file`
    /// tool, which runs inside a turn and holds one.
    pub(crate) async fn summarize_artifact(
        &self,
        path: &str,
        content: &str,
        thread_id: Uuid,
    ) -> Option<String> {
        let capture = AuxCapture::new(
            &self.event_bus,
            thread_id,
            crate::engine::ContextPurpose::ArtifactSummary,
        );
        summarize_on(
            self.current_provider().as_ref(),
            path,
            content,
            Some(&capture),
        )
        .await
    }

    /// Rebuild memory entries from event store and artifact history.
    /// When `force` is true, clears all entries first (full rebuild).
    /// When `force` is false, skips already-indexed items (resume/incremental).
    /// When `re_extract_stale` is true (and `force` is false), entries written
    /// by an older `EXTRACTOR_VERSION` are deleted before the incremental walk
    /// — the affected sources then look un-indexed to `sources_indexed` and
    /// the normal flow re-extracts them with the current extractor. Ignored
    /// when `force` is true (a full rebuild already starts from empty).
    pub async fn rebuild_memory(
        &self,
        force: bool,
        re_extract_stale: bool,
        event_bus: Option<EventBus>,
    ) {
        const CONCURRENCY: usize = 50;

        // Prevent concurrent rebuilds
        if self
            .rebuilding_memory
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            log!(@Memory, "Rebuild already in progress, skipping");
            return;
        }
        self.cancel_rebuild.store(false, Ordering::SeqCst);

        let Some(ref index) = self.memory_index else {
            log!(@Memory, "No memory index, skipping rebuild");
            self.rebuilding_memory.store(false, Ordering::SeqCst);
            return;
        };

        // The embedding model loads in the background (see `memory::EmbedderSlot`).
        // A rebuild DELETES entries before re-indexing (force → clear all;
        // re_extract_stale → delete stale-version rows), and every re-index would
        // error EMBEDDER_UNAVAILABLE while the model isn't ready — clearing memory
        // it cannot rebuild. Refuse until the model lands so a rebuild triggered
        // during the load window can't wipe memory (the HTTP handler pre-checks
        // this too; this is the fail-fast floor for every caller). The background
        // loader runs the reembed sweep itself once the model installs.
        if !self.embedder.is_ready() {
            log!(
                @Memory,
                "Embedding model not ready yet — refusing memory rebuild (would clear entries it can't re-index); retry once memory is active"
            );
            self.rebuilding_memory.store(false, Ordering::SeqCst);
            return;
        }

        let send_progress = |processed: usize, total: usize| {
            if let Some(ref bus) = event_bus {
                let pct = if total > 0 {
                    (processed * 100) / total
                } else {
                    0
                };
                let bus = bus.clone();
                tokio::spawn(async move {
                    bus.emit_or_log(
                        BusEvent::System(SystemEvent::MemoryRebuildProgress {
                            processed,
                            total,
                            percent: pct,
                        }),
                        "[Memory] MemoryRebuildProgress",
                    )
                    .await;
                });
            }
        };

        if force {
            log!(@Memory, "Starting FULL memory rebuild (force=true)...");
            match index.clear().await {
                Ok(deleted) => {
                    if deleted > 0 {
                        log!(@Memory, "Cleared {} existing memory entries", deleted);
                    }
                }
                Err(e) => {
                    log!(@Memory, "Failed to clear memory entries: {}", e);
                    self.rebuilding_memory.store(false, Ordering::SeqCst);
                    return;
                }
            }
        } else if re_extract_stale {
            log!(@Memory, "Starting INCREMENTAL memory rebuild (re_extract_stale=true)...");
            match index
                .delete_below_extractor_version(crate::memory::EXTRACTOR_VERSION)
                .await
            {
                Ok(deleted) if deleted > 0 => {
                    log!(
                        @Memory,
                        "Deleted {} stale entries (extractor_version < {}) — they will be re-extracted",
                        deleted,
                        crate::memory::EXTRACTOR_VERSION
                    );
                }
                Ok(_) => {
                    log!(@Memory, "No stale entries to re-extract");
                }
                Err(e) => {
                    log!(@Memory, "Failed to delete stale entries: {}", e);
                    self.rebuilding_memory.store(false, Ordering::SeqCst);
                    return;
                }
            }
        } else {
            log!(@Memory, "Starting INCREMENTAL memory rebuild (resume mode)...");
        }

        // Load all events and pre-filter to types that memory_content_for_event handles
        let events: Vec<_> = match self.event_store.get_all_events_chronological().await {
            Ok(all) => all
                .into_iter()
                .filter(|e| Self::memory_content_for_event(e).is_some())
                .collect(),
            Err(e) => {
                log!(@Memory, "Failed to load events: {}", e);
                self.rebuilding_memory.store(false, Ordering::SeqCst);
                return;
            }
        };

        // Walk git history for artifact changes
        let artifact_changes = match self.artifact_manager.walk_artifact_history() {
            Ok(changes) => changes,
            Err(e) => {
                log!(@Memory, "Failed to walk artifact history: {}, continuing without artifacts", e);
                Vec::new()
            }
        };

        let combined_total = events.len() + artifact_changes.len();

        // Load already-indexed sources upfront for resume mode
        let already_indexed = if !force {
            let mut all_sources: Vec<serde_json::Value> = Vec::with_capacity(combined_total);
            for event in &events {
                if let Ok(v) = serde_json::to_value(&MemorySource::Event { id: event.id }) {
                    all_sources.push(v);
                }
            }
            for change in &artifact_changes {
                if let Ok(v) = serde_json::to_value(&MemorySource::Artifact {
                    path: change.path.clone(),
                    commit: change.commit_hash.clone(),
                }) {
                    all_sources.push(v);
                }
            }
            match index.sources_indexed(&all_sources).await {
                Ok(set) => {
                    log!(@Memory, "Found {} already-indexed sources, will skip them", set.len());
                    set
                }
                Err(e) => {
                    log!(@Memory, "Failed to load indexed sources: {}, processing all", e);
                    std::collections::HashSet::new()
                }
            }
        } else {
            std::collections::HashSet::new()
        };

        log!(@Memory, "Rebuilding: {} events + {} artifact versions ({} total, {} already indexed)",
            events.len(), artifact_changes.len(), combined_total, already_indexed.len());

        // Build base extraction context (system + user, no conversation since this is a rebuild)
        let rebuild_ctx = self.extraction_context_base().await;

        let mut indexed = 0usize;
        let mut skipped = 0usize;
        let mut fallbacks = 0usize;
        let mut progress = 0usize;

        // Phase 1: Index events in parallel (chunks of CONCURRENCY)
        // Filter out already-indexed events first
        let events_to_process: Vec<_> = if already_indexed.is_empty() {
            events.iter().collect()
        } else {
            events
                .iter()
                .filter(|event| {
                    if let Ok(source_json) =
                        serde_json::to_value(&MemorySource::Event { id: event.id })
                    {
                        !already_indexed.contains(&source_json.to_string())
                    } else {
                        true
                    }
                })
                .collect()
        };
        let events_skipped = events.len() - events_to_process.len();
        skipped += events_skipped;
        progress += events_skipped;
        if events_skipped > 0 {
            send_progress(progress, combined_total);
            log!(@Memory, "Skipped {} already-indexed events", events_skipped);
        }

        let mut last_log = 0usize;
        let mut canceled = false;
        // Deferred deletes: collect IDs from concurrent futures, flush once after the loop.
        // This prevents 50 concurrent DELETE FROM memory_entries calls from deadlocking
        // on overlapping rows.
        let deferred_deletes = std::sync::Mutex::new(Vec::<Uuid>::new());
        {
            use futures::stream::StreamExt;
            // Launch up to CONCURRENCY futures at a time using a sliding window.
            // FuturesUnordered yields results as they complete, keeping the pipeline full.
            let mut in_flight = futures::stream::FuturesUnordered::new();
            let mut event_iter = events_to_process.iter();

            // Seed the initial batch
            for event in event_iter.by_ref().take(CONCURRENCY) {
                in_flight.push(self.index_event_deferred(event, &deferred_deletes));
            }

            while let Some(result) = in_flight.next().await {
                if self.cancel_rebuild.load(Ordering::SeqCst) {
                    canceled = true;
                    break;
                }
                // Refill: start a new future for each completed one
                if let Some(event) = event_iter.next() {
                    in_flight.push(self.index_event_deferred(event, &deferred_deletes));
                }
                match result {
                    Some(was_fallback) => {
                        indexed += 1;
                        if was_fallback {
                            fallbacks += 1;
                        }
                    }
                    None => {
                        skipped += 1;
                    }
                }
                progress += 1;
                send_progress(progress, combined_total);
                if progress - last_log >= 50 {
                    log!(@Memory, "Rebuild progress: {}/{} ({} indexed, {} skipped, {} fallbacks)",
                        progress, combined_total, indexed, skipped, fallbacks);
                    last_log = progress;
                }
            }
        }

        // Flush deferred deletes from Phase 1
        {
            let ids = std::mem::take(&mut *deferred_deletes.lock().unwrap());
            if !ids.is_empty() {
                log!(@Memory, "Flushing {} deferred deletes from event phase", ids.len());
                if let Err(e) = index.delete_many(&ids).await {
                    log!(@Memory, "Failed to flush deferred deletes: {}", e);
                }
            }
        }

        // Phase 2: Index artifact versions in parallel (chunks of CONCURRENCY)
        // Filter out already-indexed artifacts first
        let artifacts_to_process: Vec<_> = if already_indexed.is_empty() {
            artifact_changes.iter().collect()
        } else {
            artifact_changes
                .iter()
                .filter(|change| {
                    if let Ok(source_json) = serde_json::to_value(&MemorySource::Artifact {
                        path: change.path.clone(),
                        commit: change.commit_hash.clone(),
                    }) {
                        !already_indexed.contains(&source_json.to_string())
                    } else {
                        true
                    }
                })
                .collect()
        };
        let artifacts_skipped = artifact_changes.len() - artifacts_to_process.len();
        skipped += artifacts_skipped;
        progress += artifacts_skipped;
        if artifacts_skipped > 0 {
            send_progress(progress, combined_total);
            log!(@Memory, "Skipped {} already-indexed artifacts", artifacts_skipped);
        }

        last_log = progress;
        // Reset deferred deletes for Phase 2
        let deferred_deletes = std::sync::Mutex::new(Vec::<Uuid>::new());
        // A cancel in Phase 1 skips this phase outright. Seeding it would start
        // up to CONCURRENCY billed extraction calls before the loop's first
        // cancel check.
        if !canceled {
            use futures::stream::StreamExt;
            // Filter out binary/image/archive artifacts that can't contain prose
            let artifacts_after_ext_filter: Vec<_> = artifacts_to_process
                .iter()
                .filter(|change| !Self::should_skip_artifact_for_memory(&change.path))
                .collect();
            let ext_skipped = artifacts_to_process.len() - artifacts_after_ext_filter.len();
            if ext_skipped > 0 {
                log!(@Memory, "Skipped {} binary/image/archive artifacts", ext_skipped);
            }
            skipped += ext_skipped;
            progress += ext_skipped;

            // Pre-read artifact content (synchronous git reads), then index in parallel
            let artifact_items: Vec<_> = artifacts_after_ext_filter
                .iter()
                .filter_map(|change| {
                    let content = self
                        .artifact_manager
                        .read_artifact_at_commit_string(&change.path, &change.commit_hash)
                        .ok()?;
                    Some((change, content))
                })
                .collect();
            let read_skipped = artifacts_after_ext_filter.len() - artifact_items.len();
            skipped += read_skipped;
            progress += read_skipped;

            let mut in_flight = futures::stream::FuturesUnordered::new();
            let mut artifact_iter = artifact_items.iter();

            for (change, content) in artifact_iter.by_ref().take(CONCURRENCY) {
                in_flight.push(self.index_artifact_memory_deferred(
                    &change.path,
                    content,
                    &change.commit_hash,
                    change.timestamp,
                    Some(&rebuild_ctx),
                    &deferred_deletes,
                ));
            }

            while let Some(was_fallback) = in_flight.next().await {
                if self.cancel_rebuild.load(Ordering::SeqCst) {
                    canceled = true;
                    break;
                }
                if let Some((change, content)) = artifact_iter.next() {
                    in_flight.push(self.index_artifact_memory_deferred(
                        &change.path,
                        content,
                        &change.commit_hash,
                        change.timestamp,
                        Some(&rebuild_ctx),
                        &deferred_deletes,
                    ));
                }
                indexed += 1;
                if was_fallback {
                    fallbacks += 1;
                }
                progress += 1;
                send_progress(progress, combined_total);
                if progress - last_log >= 50 {
                    log!(@Memory, "Rebuild progress: {}/{} (artifact phase, {} indexed, {} skipped)",
                        progress, combined_total, indexed, skipped);
                    last_log = progress;
                }
            }
        }

        // Flush deferred deletes from Phase 2
        {
            let ids = std::mem::take(&mut *deferred_deletes.lock().unwrap());
            if !ids.is_empty() {
                log!(@Memory, "Flushing {} deferred deletes from artifact phase", ids.len());
                if let Err(e) = index.delete_many(&ids).await {
                    log!(@Memory, "Failed to flush deferred deletes: {}", e);
                }
            }
        }

        // Phase 3: Replay MemoryCorrected events to re-apply user corrections
        if !canceled {
            let correction_events: Vec<_> =
                match self.event_store.get_all_events_chronological().await {
                    Ok(all) => all
                        .into_iter()
                        .filter(|e| e.event_type == "MemoryCorrected")
                        .collect(),
                    Err(e) => {
                        log!(@Memory, "Failed to load correction events: {}", e);
                        Vec::new()
                    }
                };

            if !correction_events.is_empty() {
                log!(@Memory, "Phase 3: Replaying {} memory corrections", correction_events.len());
                for event in &correction_events {
                    // Prefer wrong_fact (new format) over deleted_summaries (legacy)
                    let wrong_fact = event
                        .payload
                        .get("wrong_fact")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if !wrong_fact.is_empty() {
                        // New format: use wrong_fact + keyword search + semantic filtering
                        let search_query = event
                            .payload
                            .get("search_query")
                            .and_then(|v| v.as_str())
                            .unwrap_or(wrong_fact);

                        let wrong_embedding = match self.embedder.embed(wrong_fact).await {
                            Ok(e) => e,
                            Err(err) => {
                                log!(@Memory, "Correction replay: failed to embed wrong_fact '{}': {}", &wrong_fact[..wrong_fact.floor_char_boundary(50)], err);
                                continue;
                            }
                        };

                        // Find candidates by keyword, filter by semantic similarity to wrong_fact
                        let candidates = correction_candidates(index, search_query).await;
                        if !candidates.is_empty() {
                            let candidate_texts: Vec<&str> =
                                candidates.iter().map(|e| e.summary.as_str()).collect();
                            let candidate_embeddings = match self
                                .embedder
                                .embed_batch(&candidate_texts)
                                .await
                            {
                                Ok(e) => e,
                                Err(err) => {
                                    log!(@Memory, "Correction replay: embed_batch failed: {}", err);
                                    continue;
                                }
                            };

                            let ids_to_delete: Vec<Uuid> = candidates
                                .iter()
                                .zip(candidate_embeddings.iter())
                                .filter(|(_, embedding)| {
                                    cosine_similarity(&wrong_embedding, embedding)
                                        >= MEMORY_CORRECTION_THRESHOLD
                                })
                                .map(|(entry, _)| entry.id)
                                .collect();

                            if !ids_to_delete.is_empty() {
                                let deleted = match index.delete_many(&ids_to_delete).await {
                                    Ok(n) => n,
                                    Err(e) => {
                                        log!(@Memory, "Correction replay: delete_many failed: {}", e);
                                        0
                                    }
                                };
                                log!(@Memory, "Correction replay: deleted {} of {} entries matching '{}' (similar to '{}')",
                                    deleted, candidates.len(), search_query, &wrong_fact[..wrong_fact.floor_char_boundary(60)]);
                            }
                        }
                    } else {
                        // Legacy format: use deleted_summaries for exact-ish matching
                        let deleted_summaries: Vec<String> = event
                            .payload
                            .get("deleted_summaries")
                            .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
                            .unwrap_or_default();

                        if deleted_summaries.is_empty() {
                            continue;
                        }

                        let mut ids_to_delete: Vec<uuid::Uuid> = Vec::new();
                        for summary in &deleted_summaries {
                            match self.embedder.embed(summary).await {
                                Ok(embedding) => {
                                    match index
                                        .find_similar(&embedding, 0.85, 5, self.embedder.model_id())
                                        .await
                                    {
                                        Ok(similar) => {
                                            for entry in &similar {
                                                ids_to_delete.push(entry.id);
                                            }
                                        }
                                        Err(e) => {
                                            log!(@Memory, "Correction replay: find_similar failed: {}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    log!(@Memory, "Correction replay: embed failed for '{}': {}", &summary[..summary.floor_char_boundary(50)], e);
                                }
                            }
                        }

                        ids_to_delete.sort();
                        ids_to_delete.dedup();
                        if !ids_to_delete.is_empty() {
                            let deleted = match index.delete_many(&ids_to_delete).await {
                                Ok(n) => n,
                                Err(e) => {
                                    log!(@Memory, "Correction replay (legacy): delete_many failed: {}", e);
                                    0
                                }
                            };
                            log!(@Memory, "Correction replay (legacy): deleted {} entries similar to {} wrong facts", deleted, deleted_summaries.len());
                        }
                    }

                    // Re-add corrected fact if present
                    if let Some(correction_text) =
                        event.payload.get("correction").and_then(|v| v.as_str())
                    {
                        if !correction_text.is_empty() {
                            match self.embedder.embed_batch(&[correction_text]).await {
                                Ok(embeddings) if !embeddings.is_empty() => {
                                    let (fact_id, source) = correction_entry_identity(event.id);
                                    if let Err(e) = index
                                        .index_entry(
                                            fact_id,
                                            &source,
                                            "Memory Correction",
                                            correction_text,
                                            0.8,
                                            &[],
                                            &embeddings[0],
                                            self.embedder.model_id(),
                                            event.created,
                                            crate::memory::EXTRACTOR_VERSION,
                                        )
                                        .await
                                    {
                                        log!(@Memory, "Failed to re-add correction: {}", e);
                                    }
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    log!(@Memory, "Failed to embed correction: {}", e);
                                }
                            }
                        }
                    }
                }
            }
        }

        let count = index.len().await.unwrap_or(0);
        if canceled {
            log!(@Memory, "Rebuild CANCELED at {}/{}. {} memory entries ({} indexed, {} skipped, {} fallbacks). Resume with force=false.",
                progress, combined_total, count, indexed, skipped, fallbacks);
        } else {
            send_progress(combined_total, combined_total);
            log!(@Memory, "Rebuild complete. {} memory entries from {} events + {} artifact versions ({} indexed, {} skipped, {} fallbacks).",
                count, events.len(), artifact_changes.len(), indexed, skipped, fallbacks);
        }
        self.rebuilding_memory.store(false, Ordering::SeqCst);
    }

    pub async fn post_import_index(&self, dest_relative: &str, _commit_sha: &str) {
        // Memory indexing for text artifacts happens via `memory_consumer` —
        // it subscribes to `ArtifactImported` and reads the blob at the
        // commit. Binary artifacts (PDFs, images, archives, …) are skipped
        // there. We previously used this hook to OCR / pdf-extract PDFs into
        // a `.txt` sidecar and index that out-of-band; that capability has
        // been removed, leaving nothing extra to do at upload time.
        log!(@import_bg, "Background processing complete for {}", dest_relative);
    }
}

/// One of the four built-in v5 namespaces, the same one the other derived-id
/// sites use (`core::image_described_backfill`, `core::aux_context_backfill`).
const CORRECTION_NAMESPACE: Uuid = Uuid::NAMESPACE_OID;

/// The id and source of the entry a `MemoryCorrected` replay writes.
///
/// Phase 3 replays every correction on every rebuild, so a fresh random id
/// added one more copy of the same fact each time. Every copy was retrievable
/// and every copy went into the pre-turn block, crowding other facts out. A v5
/// uuid over the event id makes the write an upsert instead.
///
/// The source is the event itself, which is what lets `sources_indexed` see
/// the row and "View source" resolve it.
fn correction_entry_identity(event_id: Uuid) -> (Uuid, MemorySource) {
    let key = format!("memory-correction:{event_id}");
    (
        Uuid::new_v5(&CORRECTION_NAMESPACE, key.as_bytes()),
        MemorySource::Event { id: event_id },
    )
}

/// Entries matching ANY WORD of a correction's `search_query`, deduped by id.
///
/// `search_by_keyword` matches `summary ILIKE '%needle%'`, so the whole phrase
/// asks for that exact substring and finds nothing. That left the delete half
/// of a replay unable to match anything it was meant to remove. One lookup per
/// word, through the tokenizer the rest of retrieval already shares.
async fn correction_candidates(index: &PgVectorIndex, search_query: &str) -> Vec<MemoryEntry> {
    let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    let mut candidates: Vec<MemoryEntry> = Vec::new();
    for keyword in super::keywords_for([search_query]) {
        match index.search_by_keyword(&keyword, 0.0, 100).await {
            Ok(results) => {
                for entry in results.entries {
                    if seen.insert(entry.id) {
                        candidates.push(entry);
                    }
                }
            }
            Err(e) => {
                log!(@Memory, "Correction replay: keyword search failed for '{}': {}", keyword, e);
            }
        }
    }
    candidates
}

#[cfg(test)]
mod summary_capture_tests {
    use super::*;
    use crate::engine::event_bus::EventBus;
    use crate::test_support::{aux_captures, setup_test_db, teardown_test_db, ScriptedProvider};

    const SUMMARY_MODEL: &str = "claude-opus-4-5";

    /// One import, one model call, one row. The call runs on the agent's own
    /// chat model, so an import of a long document is real money.
    #[tokio::test]
    async fn an_artifact_summary_records_what_it_cost() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let capture = AuxCapture::new(
            &bus,
            thread_id,
            crate::engine::ContextPurpose::ArtifactSummary,
        );

        let provider = ScriptedProvider::new(SUMMARY_MODEL, vec!["A quarterly sales report."]);
        let summary = summarize_on(
            &provider,
            "artifacts/projects/reports/q4.md",
            &"sales figures, one per region. ".repeat(20),
            Some(&capture),
        )
        .await;
        assert_eq!(summary.as_deref(), Some("A quarterly sales report."));

        let captures = aux_captures(&pool, thread_id, "artifact_summary").await;
        assert_eq!(captures.len(), 1, "one call, one row: {captures:?}");
        assert_eq!(captures[0]["producer"], "auxiliary");
        assert_eq!(captures[0]["model"], SUMMARY_MODEL);
        assert_eq!(captures[0]["usage"]["input_tokens"], 210);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A file under the size floor never reaches a provider, so it must leave
    /// no row. A row with no call behind it is as wrong as a call with no row.
    #[tokio::test]
    async fn a_file_too_small_to_summarise_records_nothing() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let capture = AuxCapture::new(
            &bus,
            thread_id,
            crate::engine::ContextPurpose::ArtifactSummary,
        );

        // No scripted reply: reaching the provider at all would fail the call.
        let provider = ScriptedProvider::new(SUMMARY_MODEL, vec![]);
        let summary = summarize_on(&provider, "notes.md", "too short", Some(&capture)).await;
        assert!(summary.is_some_and(|s| s.starts_with("Small file:")));
        assert!(aux_captures(&pool, thread_id, "artifact_summary")
            .await
            .is_empty());

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

#[cfg(test)]
mod correction_replay_tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use chrono::Utc;

    async fn seed(index: &PgVectorIndex, summary: &str) -> Uuid {
        let id = Uuid::new_v4();
        index
            .index_entry(
                id,
                &MemorySource::Event { id: Uuid::new_v4() },
                "General",
                summary,
                0.5,
                &[],
                &vec![0.1f32; 384],
                "test-model",
                Utc::now(),
                crate::memory::EXTRACTOR_VERSION,
            )
            .await
            .unwrap();
        id
    }

    /// One correction, any number of rebuilds, one row. The id is derived from
    /// the event, so Phase 3 upserts rather than inserting another copy.
    #[tokio::test]
    async fn replaying_a_correction_twice_leaves_one_entry() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();
        let event_id = Uuid::new_v4();

        for _ in 0..2 {
            let (fact_id, source) = correction_entry_identity(event_id);
            index
                .index_entry(
                    fact_id,
                    &source,
                    "Memory Correction",
                    "The dog is called Rex.",
                    0.8,
                    &[],
                    &vec![0.2f32; 384],
                    "test-model",
                    Utc::now(),
                    crate::memory::EXTRACTOR_VERSION,
                )
                .await
                .unwrap();
        }

        assert_eq!(
            index.len().await.unwrap(),
            1,
            "a second rebuild must upsert the correction, not duplicate it"
        );

        // And the row points back at the MemoryCorrected event, so
        // `sources_indexed` sees it and "View source" resolves.
        let source_json = serde_json::to_value(MemorySource::Event { id: event_id }).unwrap();
        assert_eq!(
            index.entries_for_source(&source_json).await.unwrap().len(),
            1,
            "the source must be the correction event, not a random uuid"
        );

        teardown_test_db(&db_name).await;
    }

    /// The delete half has to match. A whole phrase is not a keyword, so the
    /// raw query finds nothing and the wrong fact survives every replay.
    #[tokio::test]
    async fn correction_candidates_match_on_words_not_the_whole_phrase() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();
        let wrong = seed(&index, "The dog is called Bella.").await;

        let phrase = "what the dog is called";
        assert!(
            index
                .search_by_keyword(phrase, 0.0, 100)
                .await
                .unwrap()
                .entries
                .is_empty(),
            "the raw phrase is an ILIKE substring, which is why it never matched"
        );

        let found = correction_candidates(&index, phrase).await;
        assert_eq!(
            found.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![wrong],
            "tokenizing the query is what lets the replay delete the wrong fact"
        );

        teardown_test_db(&db_name).await;
    }
}
