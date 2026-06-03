//! Batch / derived memory operations: LLM artifact summaries, user-profile
//! generation, the full/incremental memory rebuild (events + artifact
//! history + correction replay), and the post-import hook. These drive
//! the per-item indexing helpers in `super::extract` in bulk.

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::LucidosEngine;
use crate::llm::{Message, MessageContent};
use crate::memory::{cosine_similarity, EmbeddingProvider, MemorySource};
use std::sync::atomic::Ordering;
use uuid::Uuid;

use super::scoring::MEMORY_CORRECTION_THRESHOLD;

impl LucidosEngine {
    /// Generate a summary for an artifact using the LLM
    pub(crate) async fn summarize_artifact(&self, path: &str, content: &str) -> Option<String> {
        // Skip very small files
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

        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(prompt),
        }];

        match self
            .llm
            .chat(messages, vec![], None, None, None, None)
            .await
        {
            Ok(response) => response.content,
            Err(e) => {
                log!("[Memory] Failed to generate summary for {}: {}", path, e);
                None
            }
        }
    }

    /// Generate or update the user profile based on memory contents
    pub async fn update_user_profile(&self) {
        // Sample from different memory categories to build profile
        let Some(ref index) = self.memory_index else {
            return;
        };

        // Get a diverse sample of memories using multiple query strategies
        let sample_queries = [
            // General topics
            "projects and work I'm doing",
            "meetings with colleagues and team",
            "fitness workouts and health tracking",
            "personal tasks reminders appointments",
            "travel plans and trips",
            // People-focused queries
            "met with discussed with talked to",
            "Sarah Alex Jamie meeting sync",
            // Activity types
            "completed finished deployed shipped",
            "morning brief daily summary",
            "session summary what was discussed",
        ];

        let mut context_parts = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for query in &sample_queries {
            if let Ok(embedding) = self.embedder.embed(query).await {
                if let Ok(results) = index.search(&embedding, 0.0, 20).await {
                    for entry in results.entries.iter().take(8) {
                        // Deduplicate by first 50 chars
                        let key = entry.summary.chars().take(50).collect::<String>();
                        if seen.contains(&key) {
                            continue;
                        }
                        seen.insert(key);
                        context_parts.push(format!("- {}", entry.summary));
                    }
                }
            }
        }

        // Also sample by source type to ensure coverage
        for topic_query in &["artifacts files", "session summary", "conversations events"] {
            if let Ok(embedding) = self.embedder.embed(topic_query).await {
                if let Ok(results) = index.search(&embedding, 0.0, 15).await {
                    for entry in results.entries.iter().take(5) {
                        let key = entry.summary.chars().take(50).collect::<String>();
                        if seen.contains(&key) {
                            continue;
                        }
                        seen.insert(key);
                        let source_label = match &entry.source {
                            MemorySource::Artifact { .. } => "artifact",
                            MemorySource::Event { .. } => "event",
                        };
                        context_parts.push(format!("- [{}] {}", source_label, entry.summary));
                    }
                }
            }
        }

        if context_parts.is_empty() {
            return;
        }

        // Limit to ~100 entries to avoid token overflow
        let sample_context: String = context_parts
            .into_iter()
            .take(100)
            .collect::<Vec<_>>()
            .join("\n");

        let existing_profile = self.user_profile.read().await.clone();
        let user_language = self.user_language.read().await.clone();

        let language_instruction = if user_language.is_empty() {
            String::new()
        } else {
            format!(
                "IMPORTANT: Write the ENTIRE profile in {}. All section headings, descriptions, and details must be in {}.",
                user_language, user_language
            )
        };

        let prompt = if existing_profile.is_empty() {
            // First-time creation — LLM generates the full profile
            format!(
                r#"Based on these memories about a user, create a comprehensive profile. Include:

1. **Projects & Work**: All projects mentioned, their status, and key activities
2. **People**: Everyone mentioned by name - colleagues, friends, family, contacts
3. **Health & Fitness**: Any workout routines, health tracking, fitness goals
4. **Personal Life**: Travel, hobbies, appointments, personal tasks
5. **Preferences**: Communication style, reminder preferences, how they like to work

Be thorough - extract ALL names and projects mentioned. Use bullet points. Include specific details like dates when available.

{language_instruction}

Memories:
{sample_context}

Write the profile now:"#,
                language_instruction = language_instruction,
                sample_context = sample_context
            )
        } else {
            // Incremental update — LLM outputs ONLY additions, never the full profile
            format!(
                r#"Here is the user's existing profile:

---
{existing_profile}
---

Here are recent memories:

{sample_context}

Your job: identify NEW facts from the memories that are NOT already in the profile.

Rules:
- ONLY output new information not already covered in the profile
- Skip memories that repeat what's already in the profile
- Skip trivial interactions (greetings, acknowledgments, routine tool calls)
- If a fact updates an existing one (e.g., new status), output it as an update

Output format — use EXACTLY this structure:

## Section Name
- New bullet point fact
- Another new fact

You may use any section name from the existing profile, or create a new one.

If there is genuinely nothing new to add, respond with exactly: NO_CHANGES

{language_instruction}"#,
                existing_profile = existing_profile,
                sample_context = sample_context,
                language_instruction = language_instruction
            )
        };

        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(prompt),
        }];

        match self
            .llm
            .chat(messages, vec![], None, None, None, None)
            .await
        {
            Ok(response) => {
                if let Some(llm_output) = response.content {
                    let trimmed = llm_output.trim();

                    if existing_profile.is_empty() {
                        // First-time creation — use full LLM output
                        let full_profile = format!("# User Profile\n\n{}", trimmed);
                        match self
                            .artifact_manager
                            .write_and_commit(
                                "user_profile.md",
                                &full_profile,
                                "Create user profile",
                            )
                            .await
                        {
                            Ok(_) => {
                                log!(
                                    "[Memory] User profile created ({} chars)",
                                    full_profile.len()
                                );
                                *self.user_profile.write().await = full_profile;
                            }
                            Err(e) => {
                                log!("[Memory] Failed to save user profile: {}", e);
                            }
                        }
                    } else if trimmed == "NO_CHANGES" || trimmed.is_empty() {
                        log!("[Memory] User profile: no new information to add");
                    } else {
                        // Append additions to existing profile
                        let updated = format!("{}\n\n{}", existing_profile.trim_end(), trimmed);
                        match self
                            .artifact_manager
                            .write_and_commit("user_profile.md", &updated, "Update user profile")
                            .await
                        {
                            Ok(_) => {
                                log!(
                                    "[Memory] User profile updated: appended {} chars (total {} chars)",
                                    trimmed.len(),
                                    updated.len()
                                );
                                *self.user_profile.write().await = updated;
                            }
                            Err(e) => {
                                log!("[Memory] Failed to save user profile: {}", e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                log!("[Memory] Failed to generate user profile: {}", e);
            }
        }
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
        {
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
                        match index.search_by_keyword(search_query, 0.0, 100).await {
                            Ok(results) => {
                                let candidate_texts: Vec<&str> =
                                    results.entries.iter().map(|e| e.summary.as_str()).collect();
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

                                let mut ids_to_delete: Vec<uuid::Uuid> = Vec::new();
                                for (i, entry) in results.entries.iter().enumerate() {
                                    if i < candidate_embeddings.len() {
                                        let similarity = cosine_similarity(
                                            &wrong_embedding,
                                            &candidate_embeddings[i],
                                        );
                                        if similarity >= MEMORY_CORRECTION_THRESHOLD {
                                            ids_to_delete.push(entry.id);
                                        }
                                    }
                                }

                                if !ids_to_delete.is_empty() {
                                    let deleted =
                                        index.delete_many(&ids_to_delete).await.unwrap_or(0);
                                    log!(@Memory, "Correction replay: deleted {} of {} entries matching '{}' (similar to '{}')",
                                        deleted, results.entries.len(), search_query, &wrong_fact[..wrong_fact.floor_char_boundary(60)]);
                                }
                            }
                            Err(e) => {
                                log!(@Memory, "Correction replay: keyword search failed: {}", e);
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
                                    match index.find_similar(&embedding, 0.85, 5).await {
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
                            let deleted = index.delete_many(&ids_to_delete).await.unwrap_or(0);
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
                                    let fact_id = uuid::Uuid::new_v4();
                                    let source = MemorySource::Event { id: fact_id };
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
