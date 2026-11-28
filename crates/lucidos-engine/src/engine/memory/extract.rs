use crate::core::EventRow;
use crate::engine::LucidosEngine;
use crate::memory::{EmbeddingProvider, ExtractedFact, MemorySource};
use chrono::{DateTime, Utc};
use std::sync::atomic::Ordering;
use uuid::Uuid;

use super::scoring::{MEMORY_DEDUP_CANDIDATES, MEMORY_DEDUP_THRESHOLD, MEMORY_SUPERSEDE_THRESHOLD};

impl LucidosEngine {
    pub async fn extraction_context_base(&self) -> String {
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

    pub fn extraction_context_with_conversation(
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

    pub(crate) async fn build_extraction_context_for_event(
        &self,
        thread_id: Option<uuid::Uuid>,
        current_event_id: Option<uuid::Uuid>,
    ) -> Option<String> {
        let base = self.extraction_context_base().await;
        let Some(tid) = thread_id else {
            return (!base.is_empty()).then_some(base);
        };

        let recent = match self
            .event_store
            .recent_thread_messages_for_extraction(tid, 6, current_event_id)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                log_legacy!(@Memory, "Failed to fetch thread context for extraction: {}", e);
                return (!base.is_empty()).then_some(base);
            }
        };

        let combined = Self::extraction_context_with_conversation(&base, &recent);
        (!combined.is_empty()).then_some(combined)
    }

    pub(in crate::engine) fn memory_content_for_event(event: &EventRow) -> Option<String> {
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

    /// INVARIANT: Index raw text content into memory without an EventRow wrapper.
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

    pub async fn index_memory(
        &self,
        source: MemorySource,
        content: &str,
        src_created_at: DateTime<Utc>,
        context: Option<&str>,
    ) -> bool {
        self.index_memory_inner_impl(source, content, src_created_at, context, None)
            .await
    }

    const SKIP_ARTIFACT_EXTENSIONS: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp", "tiff", "zip", "tar", "gz",
        "bz2", "xz", "7z", "rar", "wasm", "bin", "dat", "exe", "dll", "so", "dylib",
    ];

    /// INVARIANT: Artifact paths that should never be indexed into memory.
    /// user_profile.md is always loaded into extraction context already —
    /// indexing it creates a feedback loop where wrong facts in the profile
    /// get re-extracted as memory entries during rebuild.
    const SKIP_ARTIFACT_PATHS: &[&str] = &["user_profile.md"];

    pub(crate) fn should_skip_artifact_for_memory(path: &str) -> bool {
        if Self::SKIP_ARTIFACT_PATHS.iter().any(|p| path.ends_with(p)) {
            return true;
        }
        if let Some(ext) = path.rsplit('.').next() {
            Self::SKIP_ARTIFACT_EXTENSIONS.contains(&ext.to_lowercase().as_str())
        } else {
            false
        }
    }

    /// INVARIANT: Normalize an artifact path coming off the EventBus into the form
    /// `MemorySource::Artifact { path, .. }` expects: relative to
    /// `data/artifacts/`.
    ///
    /// Every writer emits the bare form now ("notes.md"). Event rows that
    /// `tools/python.rs` stored before it did carry the data-relative form
    /// ("artifacts/output.csv"), and a rebuild still reads them. Such a row
    /// can also name a path outside `artifacts/` ("apps/x/state.json"). Its
    /// read then fails and the consumer skips it, matching
    /// `walk_artifact_history`, which only sees `data/artifacts/` paths.
    pub(crate) fn canonicalize_artifact_path(path: &str) -> &str {
        path.strip_prefix("artifacts/").unwrap_or(path)
    }

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
                log_legacy!(@Memory, "Could not resolve the thread for {:?}: {}", source, e);
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

        if !self.embedder.is_ready() {
            if verbose {
                log_legacy!(
                    @Memory,
                    "Embedding model not ready yet — skipping memory indexing (recovered by a rebuild)"
                );
            }
            return false;
        }

        let is_artifact = matches!(source, MemorySource::Artifact { .. });
        let mut used_fallback = false;
        let memory_call = self.aux_call(crate::engine::ContextPurpose::Memory).await;
        // INVARIANT: Cloned, and hoisted out of the retry loop, so no `RwLock` guard is held
        // across the `extract_facts` await below. `user_language` is a
        // write-preferring `tokio::sync::RwLock`, so a read guard parked on a
        // multi-minute LLM round trip makes a `preferences(set, language)` write
        // queue behind it, and every later reader (including the setup path of
        // every chat turn, `chat/process/run.rs`) then queues behind that pending
        // writer. Every other read site already clones.
        let language = self.user_language.read().await.clone();
        let capture = crate::engine::AuxCapture::for_thread_or_home(
            &self.event_bus,
            self.source_thread_id(&source).await,
            crate::engine::ContextPurpose::Memory,
        );
        let lang_ref = (!language.is_empty()).then_some(language.as_str());
        let resample = resample_extraction(verbose, || {
            crate::memory::extract_facts(content, context, lang_ref, &memory_call, &capture)
        });
        let outcome = report_extraction(
            &self.event_bus,
            &source,
            memory_call.running_model(),
            memory_call.deadline(),
            resample,
        )
        .await;
        let facts: Vec<ExtractedFact> = match outcome {
            Extraction::Facts(f) => f,
            Extraction::Nothing => return false,
            Extraction::Failed(_) if is_artifact => {
                if verbose {
                    log_legacy!(@Memory, "No facts extracted from artifact, skipping fallback");
                }
                return false;
            }
            Extraction::Failed(_) => match crate::memory::fallback_fact(content, "General") {
                Some(f) => {
                    used_fallback = true;
                    vec![f]
                }
                None => {
                    if verbose {
                        log_legacy!(@Memory, "Extraction failed; fallback content not storable, skipping");
                    }
                    return false;
                }
            },
        };

        let texts: Vec<&str> = facts.iter().map(|f| f.fact.as_str()).collect();
        let embeddings = match self.embedder.embed_batch(&texts).await {
            Ok(e) => e,
            Err(e) => {
                if verbose {
                    log_legacy!(@Memory, "Failed to generate embeddings: {}", e);
                }
                return used_fallback;
            }
        };

        enum DedupAction {
            Skip,
            Supersede(Vec<Uuid>),
            Insert,
        }

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
                        log_legacy!(@Memory, "Dedup search failed, inserting anyway: {}", e);
                    }
                    DedupAction::Insert
                }
            })
            .collect();

        let mut all_delete_ids: Vec<Uuid> = Vec::new();
        let mut to_insert: Vec<(usize, Uuid)> = Vec::new();
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

        // INVARIANT: Batch-delete superseded entries (or defer during rebuild to avoid deadlocks)
        if !all_delete_ids.is_empty() {
            if let Some(deferred) = deferred_deletes {
                deferred.lock().unwrap().extend(all_delete_ids);
            } else if let Err(e) = index.delete_many(&all_delete_ids).await {
                if verbose {
                    log_legacy!(@Memory, "Failed to delete superseded entries: {}", e);
                }
            }
        }

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
                        log_legacy!(@Memory, "Failed to index fact: {}", e);
                    }
                }
            }
        }

        if skipped > 0 || superseded > 0 {
            log_legacy!(@Memory, "Dedup: {} skipped, {} superseded, {} inserted", skipped, superseded, inserted);
        }

        used_fallback
    }
}

#[derive(Debug)]
enum Extraction {
    Facts(Vec<ExtractedFact>),
    Nothing,
    Failed(&'static str),
}

async fn report_extraction(
    bus: &crate::engine::event_bus::EventBus,
    source: &MemorySource,
    model: &str,
    deadline: std::time::Duration,
    resample: impl std::future::Future<Output = Extraction>,
) -> Extraction {
    let outcome = match tokio::time::timeout(deadline, resample).await {
        Ok(outcome) => outcome,
        Err(_) => {
            log_legacy!(@Memory, "Extraction timed out ({:?}), falling back", deadline);
            Extraction::Failed("timed_out")
        }
    };
    if let Extraction::Failed(reason) = &outcome {
        use crate::engine::event_bus::{BusEvent, SystemEvent};
        if let Err(error) = bus
            .emit(BusEvent::System(SystemEvent::MemoryExtractionFailed {
                source: source.clone(),
                model: model.to_string(),
                reason: (*reason).to_string(),
            }))
            .await
        {
            log_legacy!(@Memory, "Failed to record memory extraction failure: {}", error);
        }
    }
    outcome
}

async fn resample_extraction<F, Fut>(verbose: bool, mut attempt_call: F) -> Extraction
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<
        Output = Result<Vec<ExtractedFact>, Box<dyn std::error::Error + Send + Sync>>,
    >,
{
    let mut reason = "call_failed";
    for attempt in 1..=3u32 {
        match attempt_call().await {
            Ok(facts) if facts.is_empty() => return Extraction::Nothing,
            Ok(facts) => return Extraction::Facts(facts),
            Err(e) => {
                reason = if e
                    .to_string()
                    .starts_with("[Memory] Failed to parse extraction JSON")
                    || e.to_string()
                        .starts_with("[Memory] Failed to deserialize extraction facts")
                {
                    "unreadable_reply"
                } else {
                    "call_failed"
                };
                if e.is::<crate::llm::spend_cap::SpendCapReached>() {
                    return Extraction::Failed(reason);
                }

                if verbose {
                    log_legacy!(@Memory, "Extraction failed (attempt {}/3): {}", attempt, e);
                }
                if attempt < 3 && crate::llm::is_transient_error(&e.to_string()) {
                    tokio::time::sleep(crate::llm::retry_delay(attempt, 2)).await;
                }
            }
        }
    }
    Extraction::Failed(reason)
}

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

#[cfg(test)]
mod spend_cap_tests {
    #[tokio::test]
    async fn a_spend_cap_is_never_resampled() {
        let mut calls = 0;
        let result = super::resample_extraction(false, || {
            calls += 1;
            let cap = crate::llm::spend_cap::SpendCapReached {
                provider: crate::llm::ProviderKind::OpenAi,
                resets_at: None,
                message: "spent".into(),
            };
            std::future::ready(Err(
                Box::new(cap) as Box<dyn std::error::Error + Send + Sync>
            ))
        })
        .await;
        assert!(matches!(result, super::Extraction::Failed("call_failed")));
        assert_eq!(calls, 1);
    }
}
