//! Per-item memory extraction & indexing: build the extraction context for an
//! event, turn a stored event / raw text / artifact into facts, dedup against
//! existing entries, and persist. The heavy `index_memory_inner_impl` (extract
//! → embed → dedup → supersede → insert) lives here; the batch rebuild that
//! drives it in bulk lives in `super::rebuild`.

use crate::core::EventRow;
use crate::engine::LucidosEngine;
use crate::memory::{EmbeddingProvider, ExtractedFact, MemorySource};
use chrono::{DateTime, Utc};
use std::sync::atomic::Ordering;
use uuid::Uuid;

use super::scoring::{MEMORY_DEDUP_CANDIDATES, MEMORY_DEDUP_THRESHOLD, MEMORY_SUPERSEDE_THRESHOLD};

impl LucidosEngine {
    pub(crate) async fn extraction_context_base(&self) -> String {
        let user_summary: String = self
            .user_profile
            .snapshot()
            .await
            .chars()
            .take(300)
            .collect();

        let user_language = self.user_language.read().await.clone();

        let mut ctx = if user_summary.is_empty() {
            String::new()
        } else {
            format!("Background:\n- The user's own profile (extract ONLY facts about THIS person, not about other people mentioned in conversations): {}", user_summary)
        };

        if !user_language.is_empty() {
            if ctx.is_empty() {
                ctx = format!(
                    "Background:\n- Language: Write all extracted facts in {}",
                    user_language
                );
            } else {
                ctx.push_str(&format!(
                    "\n- Language: Write all extracted facts in {}",
                    user_language
                ));
            }
        }

        ctx
    }

    /// Build extraction context including conversation summary for chat indexing.
    pub(crate) fn extraction_context_with_conversation(
        base_context: &str,
        recent_messages: &str,
    ) -> String {
        if recent_messages.is_empty() {
            base_context.to_string()
        } else {
            format!(
                "{}\n- Current conversation: {}",
                base_context, recent_messages
            )
        }
    }

    /// Build the full extraction context for a specific event: base profile/language
    /// PLUS the most recent prior messages from the same thread, so Gemini can resolve
    /// pronouns and inherit entities (e.g. "the order" → "the customer's order").
    ///
    /// `current_event_id` is excluded so the event being extracted doesn't appear in
    /// its own prompt. Returns `None` only when profile, language, AND thread context
    /// are all empty.
    pub(crate) async fn build_extraction_context_for_event(
        &self,
        thread_id: Option<uuid::Uuid>,
        current_event_id: Option<uuid::Uuid>,
    ) -> Option<String> {
        let base = self.extraction_context_base().await;
        let Some(tid) = thread_id else {
            return (!base.is_empty()).then_some(base);
        };

        // 6 messages is enough to resolve coreferences without ballooning the prompt.
        let recent = match self
            .event_store
            .recent_thread_messages_for_extraction(tid, 6, current_event_id)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                log!(@Memory, "Failed to fetch thread context for extraction: {}", e);
                return (!base.is_empty()).then_some(base);
            }
        };

        let combined = Self::extraction_context_with_conversation(&base, &recent);
        (!combined.is_empty()).then_some(combined)
    }

    /// Extract the content to index into memory from a stored event.
    /// Deserializes to ThreadEvent and delegates to `indexable_text()` —
    /// single source of truth shared with the live memory consumer.
    /// Returns None for event types that should not be indexed or for
    /// trigger threads.
    pub(super) fn memory_content_for_event(event: &EventRow) -> Option<String> {
        // Skip trigger-driven threads (scheduled or event-fired)
        if event.payload.get("channel").and_then(|v| v.as_str()) == Some("trigger") {
            return None;
        }
        let thread_event = crate::engine::thread_events::ThreadEvent::from_stored(
            &event.event_type,
            event.payload.clone(),
        )
        .ok()?;
        thread_event.indexable_text().map(ToString::to_string)
    }

    /// Index raw text content into memory without an EventRow wrapper.
    /// Used for live chat/response indexing where we already have the text.
    /// `event_id` must be a real persisted event ID so "View source" can look it up.
    pub(crate) async fn index_text(
        &self,
        content: &str,
        context: Option<&str>,
        event_id: Uuid,
    ) -> Option<bool> {
        if content.trim().is_empty() {
            return None;
        }
        let source = MemorySource::Event { id: event_id };
        Some(
            self.index_memory_inner_impl(source, content, Utc::now(), context, None)
                .await,
        )
    }

    /// Index an event into memory using the shared content extraction logic (for rebuild).
    /// Returns `Some(was_fallback)` if content was indexed, `None` if skipped.
    /// Defers deletes to a shared buffer for batching.
    pub(crate) async fn index_event_deferred(
        &self,
        event: &EventRow,
        deferred_deletes: &std::sync::Mutex<Vec<Uuid>>,
    ) -> Option<bool> {
        if let Some(content) = Self::memory_content_for_event(event) {
            if !content.trim().is_empty() {
                let source = MemorySource::Event { id: event.id };
                let context = self
                    .build_extraction_context_for_event(event.thread_id, Some(event.id))
                    .await;
                return Some(
                    self.index_memory_inner_impl(
                        source,
                        &content,
                        event.created,
                        context.as_deref(),
                        Some(deferred_deletes),
                    )
                    .await,
                );
            }
        }
        None
    }

    /// Index arbitrary content into memory (for artifacts and other non-event content).
    pub(crate) async fn index_memory(
        &self,
        source: MemorySource,
        content: &str,
        src_created_at: DateTime<Utc>,
        context: Option<&str>,
    ) -> bool {
        self.index_memory_inner_impl(source, content, src_created_at, context, None)
            .await
    }

    /// File extensions that should be skipped during memory extraction.
    /// Only truly unextractable formats — binary, images, archives.
    /// Text-based formats (json, csv, yaml, etc.) are kept because they may
    /// contain meaningful user content (e.g., skill data.json files).
    const SKIP_ARTIFACT_EXTENSIONS: &[&str] = &[
        // Images
        "png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp", "tiff", // Archives
        "zip", "tar", "gz", "bz2", "xz", "7z", "rar", // Binary
        "wasm", "bin", "dat", "exe", "dll", "so", "dylib",
    ];

    /// Artifact paths that should never be indexed into memory.
    /// user_profile.md is always loaded into extraction context already —
    /// indexing it creates a feedback loop where wrong facts in the profile
    /// get re-extracted as memory entries during rebuild.
    const SKIP_ARTIFACT_PATHS: &[&str] = &["user_profile.md"];

    pub(crate) fn should_skip_artifact_for_memory(path: &str) -> bool {
        // Skip by path suffix
        if Self::SKIP_ARTIFACT_PATHS.iter().any(|p| path.ends_with(p)) {
            return true;
        }
        // Skip by extension (binary/image/archive)
        if let Some(ext) = path.rsplit('.').next() {
            Self::SKIP_ARTIFACT_EXTENSIONS.contains(&ext.to_lowercase().as_str())
        } else {
            false
        }
    }

    /// Normalize an artifact path coming off the EventBus into the form
    /// `MemorySource::Artifact { path, .. }` expects: relative to
    /// `data/artifacts/`.
    ///
    /// Sources of inconsistency this hides:
    /// - `tools/files.rs`, `tools/import.rs`, `tools/email.rs` already strip the
    ///   `artifacts/` prefix before emitting → path arrives bare ("notes.md").
    /// - `tools/python.rs` walks staged files under `data/` and emits the
    ///   data-relative path → for files under `data/artifacts/` the path arrives
    ///   prefixed ("artifacts/output.csv"). For files under other data subdirs
    ///   (apps/, knowhow/) the prefix is different and they are not artifacts at
    ///   all. The caller's subsequent read fails and the consumer
    ///   silently skips, matching `walk_artifact_history` which only sees
    ///   `data/artifacts/` paths.
    pub(crate) fn canonicalize_artifact_path(path: &str) -> &str {
        path.strip_prefix("artifacts/").unwrap_or(path)
    }

    /// Index an artifact's content into memory. Truncates to 4000 chars.
    /// Used by both live artifact operations and memory rebuild.
    pub(crate) async fn index_artifact_memory(
        &self,
        path: &str,
        content: &str,
        commit: &str,
        src_created_at: DateTime<Utc>,
        context: Option<&str>,
    ) -> bool {
        self.index_artifact_memory_impl(path, content, commit, src_created_at, context, None)
            .await
    }

    /// Like index_artifact_memory but defers deletes to a shared buffer (for use during rebuild).
    pub(crate) async fn index_artifact_memory_deferred(
        &self,
        path: &str,
        content: &str,
        commit: &str,
        src_created_at: DateTime<Utc>,
        context: Option<&str>,
        deferred_deletes: &std::sync::Mutex<Vec<Uuid>>,
    ) -> bool {
        self.index_artifact_memory_impl(
            path,
            content,
            commit,
            src_created_at,
            context,
            Some(deferred_deletes),
        )
        .await
    }

    async fn index_artifact_memory_impl(
        &self,
        path: &str,
        content: &str,
        commit: &str,
        src_created_at: DateTime<Utc>,
        context: Option<&str>,
        deferred_deletes: Option<&std::sync::Mutex<Vec<Uuid>>>,
    ) -> bool {
        if Self::should_skip_artifact_for_memory(path) {
            return false;
        }
        let truncated: String = content.chars().take(4000).collect();
        let formatted = format!("File: {}\n\n{}", path, truncated);
        let source = MemorySource::Artifact {
            path: path.to_string(),
            commit: commit.to_string(),
        };
        self.index_memory_inner_impl(
            source,
            &formatted,
            src_created_at,
            context,
            deferred_deletes,
        )
        .await
    }

    /// The thread an indexing job's LLM cost belongs to.
    ///
    /// An event's own thread, or the thread that wrote an artifact at that
    /// commit while it still exists. `None` for an event row with no
    /// `thread_id`, and for an artifact no surviving thread wrote. Either
    /// lookup sits next to an LLM round trip, so its cost does not register.
    async fn source_thread_id(&self, source: &MemorySource) -> Option<Uuid> {
        let found = match source {
            MemorySource::Event { id } => {
                sqlx::query_scalar::<_, Option<Uuid>>("SELECT thread_id FROM events WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await
            }
            MemorySource::Artifact { path, commit } => {
                artifact_writer_thread(&self.pool, path, commit)
                    .await
                    .map(Some)
            }
        };
        found
            .unwrap_or_else(|e| {
                log!(@Memory, "Could not resolve the thread for {:?}: {}", source, e);
                None
            })
            .flatten()
    }

    async fn index_memory_inner_impl(
        &self,
        source: MemorySource,
        content: &str,
        src_created_at: DateTime<Utc>,
        context: Option<&str>,
        deferred_deletes: Option<&std::sync::Mutex<Vec<Uuid>>>,
    ) -> bool {
        let Some(ref index) = self.memory_index else {
            return false;
        };

        let verbose = !self.rebuilding_memory.load(Ordering::SeqCst);

        // The embedding model loads in the background (see `memory::EmbedderSlot`).
        // While it isn't ready, embedding these facts would fail and the item
        // would be dropped at the `embed_batch` step below anyway — so skip the
        // costly LLM fact-extraction entirely rather than burn a call whose
        // result we'd discard. Items created during this window are NOT
        // auto-indexed once the model lands (the post-install `reembed_stale`
        // sweep only re-embeds EXISTING rows); a manual memory rebuild recovers
        // them. See docs/known-gaps.md § "Memory created during the model-load
        // window isn't auto-indexed".
        if !self.embedder.is_ready() {
            if verbose {
                log!(
                    @Memory,
                    "Embedding model not ready yet — skipping memory indexing (recovered by a rebuild)"
                );
            }
            return false;
        }

        // An artifact never takes the raw-text fallback: a CSV export or a data
        // file stored verbatim is noise, not a fact.
        let is_artifact = matches!(source, MemorySource::Artifact { .. });
        let mut used_fallback = false;
        let memory_call = self.aux_call(crate::engine::ContextPurpose::Memory).await;
        // Cloned, and hoisted out of the retry loop, so no `RwLock` guard is held
        // across the `extract_facts` await below. `user_language` is a
        // write-preferring `tokio::sync::RwLock`, so a read guard parked on a
        // multi-minute LLM round trip makes a `preferences(set, language)` write
        // queue behind it, and every later reader (including the setup path of
        // every chat turn, `chat/process/run.rs`) then queues behind that pending
        // writer. Every other read site already clones.
        let language = self.user_language.read().await.clone();
        // Extraction is billed to the thread the source event belongs to, so
        // its cost lands where the conversation that caused it lives. A
        // source with no thread records on the home thread.
        let capture = crate::engine::AuxCapture::for_thread_or_home(
            &self.event_bus,
            self.source_thread_id(&source).await,
            crate::engine::ContextPurpose::Memory,
        );
        let lang_ref = (!language.is_empty()).then_some(language.as_str());
        // The purpose's deadline bounds the WHOLE resample, not each call
        // inside it. Three attempts, each carrying the provider's own
        // retries, is exactly how a caller escapes a per-call bound.
        let resample = resample_extraction(verbose, || {
            crate::memory::extract_facts(content, context, lang_ref, &memory_call, &capture)
        });
        let outcome = match tokio::time::timeout(memory_call.deadline(), resample).await {
            Ok(outcome) => outcome,
            Err(_) => {
                log!(@Memory, "Extraction timed out ({:?}), falling back", memory_call.deadline());
                Extraction::Failed
            }
        };
        let facts: Vec<ExtractedFact> = match outcome {
            Extraction::Facts(f) => f,
            Extraction::Nothing => return false,
            Extraction::Failed if is_artifact => {
                if verbose {
                    log!(@Memory, "No facts extracted from artifact, skipping fallback");
                }
                return false;
            }
            Extraction::Failed => match crate::memory::fallback_fact(content, "General") {
                Some(f) => {
                    used_fallback = true;
                    vec![f]
                }
                None => {
                    if verbose {
                        log!(@Memory, "Extraction failed; fallback content not storable, skipping");
                    }
                    return false;
                }
            },
        };

        // Batch-embed all fact summaries
        let texts: Vec<&str> = facts.iter().map(|f| f.fact.as_str()).collect();
        let embeddings = match self.embedder.embed_batch(&texts).await {
            Ok(e) => e,
            Err(e) => {
                if verbose {
                    log!(@Memory, "Failed to generate embeddings: {}", e);
                }
                return used_fallback;
            }
        };

        enum DedupAction {
            Skip,
            Supersede(Vec<Uuid>),
            Insert,
        }

        // Run similarity searches in chunks to avoid exhausting the DB pool
        const DB_CONCURRENCY: usize = 10;
        let mut similarity_results = Vec::with_capacity(embeddings.len());
        let embedding_model = self.embedder.model_id();
        for chunk in embeddings.chunks(DB_CONCURRENCY) {
            let chunk_futures: Vec<_> = chunk
                .iter()
                .map(|embedding| {
                    index.find_similar(
                        embedding,
                        MEMORY_SUPERSEDE_THRESHOLD,
                        MEMORY_DEDUP_CANDIDATES,
                        embedding_model,
                    )
                })
                .collect();
            similarity_results.extend(futures::future::join_all(chunk_futures).await);
        }

        // Determine dedup actions from similarity results
        let actions: Vec<DedupAction> = facts
            .iter()
            .zip(similarity_results.into_iter())
            .map(|(fact, result)| match result {
                Ok(similar) if !similar.is_empty() => {
                    if similar
                        .iter()
                        .any(|s| s.similarity >= MEMORY_DEDUP_THRESHOLD)
                    {
                        DedupAction::Skip
                    } else {
                        let fact_entities: std::collections::HashSet<&str> =
                            fact.entities.iter().map(|e| e.as_str()).collect();
                        let to_supersede: Vec<Uuid> = similar
                            .iter()
                            .filter(|s| {
                                s.entities
                                    .iter()
                                    .any(|e| fact_entities.contains(e.as_str()))
                            })
                            .map(|s| s.id)
                            .collect();
                        if to_supersede.is_empty() {
                            DedupAction::Insert
                        } else {
                            DedupAction::Supersede(to_supersede)
                        }
                    }
                }
                Ok(_) => DedupAction::Insert,
                Err(e) => {
                    if verbose {
                        log!(@Memory, "Dedup search failed, inserting anyway: {}", e);
                    }
                    DedupAction::Insert
                }
            })
            .collect();

        // Collect all IDs to supersede and all entries to insert
        let mut all_delete_ids: Vec<Uuid> = Vec::new();
        let mut to_insert: Vec<(usize, Uuid)> = Vec::new(); // (fact index, new id)
        let mut skipped = 0u32;
        let mut superseded = 0u32;

        for (i, action) in actions.into_iter().enumerate() {
            match action {
                DedupAction::Skip => {
                    skipped += 1;
                }
                DedupAction::Supersede(old_ids) => {
                    superseded += old_ids.len() as u32;
                    all_delete_ids.extend(old_ids);
                    to_insert.push((i, Uuid::new_v4()));
                }
                DedupAction::Insert => {
                    to_insert.push((i, Uuid::new_v4()));
                }
            }
        }

        // Batch-delete superseded entries (or defer during rebuild to avoid deadlocks)
        if !all_delete_ids.is_empty() {
            if let Some(deferred) = deferred_deletes {
                deferred.lock().unwrap().extend(all_delete_ids);
            } else if let Err(e) = index.delete_many(&all_delete_ids).await {
                if verbose {
                    log!(@Memory, "Failed to delete superseded entries: {}", e);
                }
            }
        }

        // Insert new entries in chunks
        let mut insert_results = Vec::with_capacity(to_insert.len());
        for chunk in to_insert.chunks(DB_CONCURRENCY) {
            let chunk_futures: Vec<_> = chunk
                .iter()
                .map(|(i, fact_id)| {
                    let fact = &facts[*i];
                    let embedding = &embeddings[*i];
                    index.index_entry(
                        *fact_id,
                        &source,
                        &fact.topic,
                        &fact.fact,
                        fact.importance,
                        &fact.entities,
                        embedding,
                        self.embedder.model_id(),
                        src_created_at,
                        crate::memory::EXTRACTOR_VERSION,
                    )
                })
                .collect();
            insert_results.extend(futures::future::join_all(chunk_futures).await);
        }

        let mut inserted = 0u32;
        for result in insert_results {
            match result {
                Ok(()) => {
                    inserted += 1;
                }
                Err(e) => {
                    if verbose {
                        log!(@Memory, "Failed to index fact: {}", e);
                    }
                }
            }
        }

        if skipped > 0 || superseded > 0 {
            log!(@Memory, "Dedup: {} skipped, {} superseded, {} inserted", skipped, superseded, inserted);
        }

        used_fallback
    }
}

/// What extracting one item yielded.
#[derive(Debug)]
enum Extraction {
    Facts(Vec<ExtractedFact>),
    /// The model read the content and found nothing worth remembering.
    Nothing,
    /// No attempt produced a readable answer.
    Failed,
}

/// Up to three extraction attempts, retrying only a failed call or an
/// unreadable reply. An empty list is an answer, so it ends the resample:
/// asking again only shopped for a different one. Each attempt carries the
/// purpose's attempt cap on its requests, so a stalled one cannot eat the rest.
async fn resample_extraction<F, Fut>(verbose: bool, mut attempt_call: F) -> Extraction
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<
        Output = Result<Vec<ExtractedFact>, Box<dyn std::error::Error + Send + Sync>>,
    >,
{
    for attempt in 1..=3u32 {
        match attempt_call().await {
            Ok(facts) if facts.is_empty() => return Extraction::Nothing,
            Ok(facts) => return Extraction::Facts(facts),
            Err(e) => {
                if verbose {
                    log!(@Memory, "Extraction failed (attempt {}/3): {}", attempt, e);
                }
                // Exponential backoff before the next attempt, for a transient
                // error only. The provider already retried internally; this
                // delay lets the rate-limit window reset. An unreadable reply
                // resamples at once.
                if attempt < 3 && crate::llm::is_transient_error(&e.to_string()) {
                    tokio::time::sleep(crate::llm::retry_delay(attempt, 2)).await;
                }
            }
        }
    }
    Extraction::Failed
}

/// The surviving thread that wrote `path` (relative to `data/artifacts/`)
/// at `commit`, read from the artifact event that announced the write.
pub(crate) async fn artifact_writer_thread(
    pool: &sqlx::PgPool,
    path: &str,
    commit: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let artifact_writes = crate::engine::summary_tree::workspace_log::sql_list(
        crate::engine::summary_tree::workspace_log::ARTIFACT_WRITE_EVENT_TYPES,
    );
    sqlx::query_scalar(&format!(
        "SELECT w.thread_id FROM events e \
         JOIN thread_summaries w ON w.thread_id = \
           (COALESCE(e.payload->'data', e.payload)->>'writer_thread_id')::uuid \
         WHERE e.event_type IN ({artifact_writes}) \
           AND COALESCE(e.payload->'data', e.payload)->>'artifact_path' \
               IN ($1, 'artifacts/' || $1) \
           AND COALESCE(COALESCE(e.payload->'data', e.payload)->>'commit', \
                        COALESCE(e.payload->'data', e.payload)->>'commit_hash') = $2 \
         ORDER BY e.sequence DESC LIMIT 1"
    ))
    .bind(path)
    .bind(commit)
    .fetch_optional(pool)
    .await
}

#[cfg(test)]
#[path = "../memory_tests/source.rs"]
mod memory_source_tests;

#[cfg(test)]
#[path = "../memory_tests/resample.rs"]
mod resample_tests;

#[cfg(test)]
#[path = "../memory_tests/artifact_writer.rs"]
mod artifact_writer_tests;
