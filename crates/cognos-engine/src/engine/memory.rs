use crate::core::EventRow;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::llm::{Message, MessageContent};
use crate::memory::{
    cosine_similarity, EmbeddingProvider, ExtractedFact, MemoryExtractor, MemorySource,
};
use chrono::{DateTime, Utc};
use std::sync::atomic::Ordering;
use uuid::Uuid;

use super::CognosEngine;

/// Cosine similarity threshold for matching wrong_fact against memory candidates
/// in correct_memory. Entries must exceed this to be considered for deletion.
pub(crate) const MEMORY_CORRECTION_THRESHOLD: f32 = 0.65;

/// Jaccard similarity between two strings based on word sets.
/// Returns a value between 0.0 (no overlap) and 1.0 (identical word sets).
pub(super) fn jaccard_similarity(a: &str, b: &str) -> f32 {
    let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();
    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }
    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f32 / union as f32
}

impl CognosEngine {
    pub(crate) async fn extraction_context_base(&self) -> String {
        let profile = self.user_profile.read().await;
        let user_summary: String = profile.chars().take(300).collect();
        drop(profile);

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
    /// pronouns and inherit entities (e.g. "the eye operation" → "pappa's eye operation").
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
    fn memory_content_for_event(event: &EventRow) -> Option<String> {
        // Skip trigger-driven threads (scheduled or event-fired)
        if event.payload.get("channel").and_then(|v| v.as_str()) == Some("trigger") {
            return None;
        }
        // Reconstruct the ThreadEvent from stored payload + event_type
        let mut payload = event.payload.clone();
        payload.as_object_mut()?.insert(
            "type".into(),
            serde_json::Value::String(event.event_type.clone()),
        );
        let thread_event: crate::engine::thread_events::ThreadEvent =
            serde_json::from_value(payload).ok()?;
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
        self.index_memory_inner(source, content, src_created_at, context)
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

    fn should_skip_artifact_for_memory(path: &str) -> bool {
        // Skip by exact path
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
        if Self::should_skip_artifact_for_memory(path) {
            return false;
        }
        let truncated: String = content.chars().take(4000).collect();
        let formatted = format!("File: {}\n\n{}", path, truncated);
        let source = MemorySource::Artifact {
            path: path.to_string(),
            commit: commit.to_string(),
        };
        self.index_memory_inner(source, &formatted, src_created_at, context)
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
        if Self::should_skip_artifact_for_memory(path) {
            return false;
        }
        let truncated: String = content.chars().take(4000).collect();
        let formatted = format!("File: {}\n\n{}", path, truncated);
        let source = MemorySource::Artifact {
            path: path.to_string(),
            commit: commit.to_string(),
        };
        self.index_memory_inner_deferred(
            source,
            &formatted,
            src_created_at,
            context,
            deferred_deletes,
        )
        .await
    }

    pub(crate) async fn index_memory_inner(
        &self,
        source: MemorySource,
        content: &str,
        src_created_at: DateTime<Utc>,
        context: Option<&str>,
    ) -> bool {
        self.index_memory_inner_impl(source, content, src_created_at, context, None)
            .await
    }

    pub(crate) async fn index_memory_inner_deferred(
        &self,
        source: MemorySource,
        content: &str,
        src_created_at: DateTime<Utc>,
        context: Option<&str>,
        deferred_deletes: &std::sync::Mutex<Vec<Uuid>>,
    ) -> bool {
        self.index_memory_inner_impl(
            source,
            content,
            src_created_at,
            context,
            Some(deferred_deletes),
        )
        .await
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

        // Extract facts via Gemini Flash with retry, falling back to a single raw-content fact.
        // For artifacts, skip fallback — if the LLM couldn't extract facts, the content
        // probably doesn't contain any (e.g., CSV exports, data files).
        let is_artifact = matches!(source, MemorySource::Artifact { .. });
        let mut used_fallback = false;
        let memory_model =
            crate::core::PreferenceStore::get(&self.pool, crate::core::PREF_MODEL_MEMORY)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
        let facts: Vec<ExtractedFact> = if let Some(ref extractor) = self.extractor {
            let mut facts = None;
            for attempt in 1..=3u32 {
                let language = self.user_language.read().await;
                let lang_ref = if language.is_empty() {
                    None
                } else {
                    Some(language.as_str())
                };
                match extractor
                    .extract_facts(content, context, lang_ref, Some(&memory_model))
                    .await
                {
                    Ok(f) if !f.is_empty() => {
                        facts = Some(f);
                        break;
                    }
                    Ok(_) => {
                        if verbose {
                            log!(@Memory, "Extraction returned no facts (attempt {}/3)", attempt);
                        }
                    }
                    Err(e) => {
                        if verbose {
                            log!(@Memory, "Extraction failed (attempt {}/3): {}", attempt, e);
                        }
                        // Exponential backoff before next attempt (provider already retried
                        // internally — this delay lets the rate limit window reset)
                        if attempt < 3 {
                            let delay = crate::llm::retry_delay(attempt, 2);
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            }
            match facts {
                Some(f) => f,
                None if is_artifact => {
                    if verbose {
                        log!(@Memory, "No facts extracted from artifact, skipping fallback");
                    }
                    return false;
                }
                None => {
                    used_fallback = true;
                    vec![MemoryExtractor::fallback_fact(content, "General")]
                }
            }
        } else if is_artifact {
            return false;
        } else {
            used_fallback = true;
            vec![MemoryExtractor::fallback_fact(content, "General")]
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

        // Dedup thresholds
        const MEMORY_DEDUP_THRESHOLD: f64 = 0.95; // Skip (near-duplicate)
        const MEMORY_SUPERSEDE_THRESHOLD: f32 = 0.85; // Replace old with new (passed to SQL)

        enum DedupAction {
            Skip,
            Supersede(Vec<Uuid>),
            Insert,
        }

        // Run similarity searches in chunks to avoid exhausting the DB pool
        const DB_CONCURRENCY: usize = 10;
        let mut similarity_results = Vec::with_capacity(embeddings.len());
        for chunk in embeddings.chunks(DB_CONCURRENCY) {
            let chunk_futures: Vec<_> = chunk
                .iter()
                .map(|embedding| index.find_similar(embedding, MEMORY_SUPERSEDE_THRESHOLD, 5))
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
                log!("Warning: Failed to generate summary for {}: {}", path, e);
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
                                log!("User profile created ({} chars)", full_profile.len());
                                *self.user_profile.write().await = full_profile;
                            }
                            Err(e) => {
                                log!("Warning: Failed to save user profile: {}", e);
                            }
                        }
                    } else if trimmed == "NO_CHANGES" || trimmed.is_empty() {
                        log!("User profile: no new information to add");
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
                                    "User profile updated: appended {} chars (total {} chars)",
                                    trimmed.len(),
                                    updated.len()
                                );
                                *self.user_profile.write().await = updated;
                            }
                            Err(e) => {
                                log!("Warning: Failed to save user profile: {}", e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                log!("Warning: Failed to generate user profile: {}", e);
            }
        }
    }

    /// Rebuild memory entries from event store and artifact history.
    /// When `force` is true, clears all entries first (full rebuild).
    /// When `force` is false, skips already-indexed items (resume/incremental).
    pub async fn rebuild_memory(&self, force: bool, event_bus: Option<EventBus>) {
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
                    let _ = bus
                        .emit(BusEvent::System(SystemEvent::MemoryRebuildProgress {
                            processed,
                            total,
                            percent: pct,
                        }))
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

    pub async fn post_import_index(&self, dest_relative: &str, commit_sha: &str) {
        let extension = std::path::Path::new(dest_relative)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let is_binary = crate::core::is_binary_extension(&extension);

        // PDF: extract text, write sidecar, commit sidecar
        let extracted_text = if extension == "pdf" {
            let artifact_path = self.artifact_manager.artifact_path(dest_relative);
            match super::document::safe_extract_pdf_text(&artifact_path) {
                Ok(text) if !text.trim().is_empty() => {
                    log!(@import_bg, "pdf_extract succeeded, {} chars", text.len());
                    Some(text)
                }
                Ok(_) => {
                    log!(@import_bg, "pdf_extract empty, trying OCR");
                    super::document::extract_text_with_ocr(&artifact_path).ok()
                }
                Err(e) => {
                    log!(@import_bg, "pdf_extract failed: {}, trying OCR", e);
                    super::document::extract_text_with_ocr(&artifact_path).ok()
                }
            }
        } else {
            None
        };

        if let Some(text) = &extracted_text {
            let text_path = format!("{}.txt", dest_relative);
            if let Err(e) = self
                .artifact_manager
                .write_and_commit(
                    &text_path,
                    text,
                    &format!("Extract text from {}", dest_relative),
                )
                .await
            {
                log!(@import_bg, "Failed to write/commit sidecar: {}", e);
            }
        }

        // Summarize
        let summary = if let Some(text) = &extracted_text {
            self.summarize_artifact(dest_relative, text)
                .await
                .unwrap_or_else(|| "PDF document".to_string())
        } else if !is_binary {
            if let Ok(content) = self.artifact_manager.read_artifact(dest_relative) {
                self.summarize_artifact(dest_relative, &content)
                    .await
                    .unwrap_or_else(|| "Text file".to_string())
            } else {
                format!("{} file", extension.to_uppercase())
            }
        } else {
            format!("{} file", extension.to_uppercase())
        };

        // Index in memory
        let import_ctx = self.extraction_context_base().await;
        let indexed = if extension == "pdf" {
            let sidecar_path = format!("{}.txt", dest_relative);
            if let Ok(content) = self.artifact_manager.read_artifact(&sidecar_path) {
                self.index_artifact_memory(
                    dest_relative,
                    &content,
                    commit_sha,
                    Utc::now(),
                    Some(&import_ctx),
                )
                .await;
                true
            } else {
                false
            }
        } else if !is_binary {
            if let Ok(content) = self.artifact_manager.read_artifact(dest_relative) {
                self.index_artifact_memory(
                    dest_relative,
                    &content,
                    commit_sha,
                    Utc::now(),
                    Some(&import_ctx),
                )
                .await;
                true
            } else {
                false
            }
        } else {
            false
        };
        if !indexed {
            // Fallback: index summary for binary/unreadable files
            let fallback = format!("Imported file: {}\n{}", dest_relative, summary);
            self.index_memory(
                MemorySource::Artifact {
                    path: dest_relative.to_string(),
                    commit: commit_sha.to_string(),
                },
                &fallback,
                Utc::now(),
                Some(&import_ctx),
            )
            .await;
        }
        log!(@import_bg, "Background processing complete for {}", dest_relative);
    }
}

#[cfg(test)]
mod memory_source_tests {
    use super::CognosEngine;
    use crate::core::EventRow;

    #[test]
    fn trigger_completed_is_not_indexed() {
        let event = EventRow::new(
            "TriggerCompleted",
            serde_json::json!({
                "trigger_id": "t-1",
                "trigger_name": "Calendar sync",
                "result_summary": "Synced 47 events from Google Calendar",
            }),
        );
        let content = CognosEngine::memory_content_for_event(&event);
        assert!(
            content.is_none(),
            "Trigger events should not be indexed into memory"
        );
    }

    #[test]
    fn message_received_produces_indexable_content() {
        let msg = EventRow::new(
            "MessageReceived",
            serde_json::json!({"text": "Hello world"}),
        );
        let content = CognosEngine::memory_content_for_event(&msg);
        assert_eq!(content.unwrap(), "Hello world");
    }

    #[test]
    fn trigger_channel_events_not_indexed() {
        // Events with channel="trigger" should be skipped regardless of type
        let msg = EventRow::new(
            "MessageReceived",
            serde_json::json!({
                "text": "Check weather",
                "channel": "trigger",
            }),
        );
        assert!(
            CognosEngine::memory_content_for_event(&msg).is_none(),
            "MessageReceived in trigger channel should not be indexed"
        );

        // Regular chat messages should still be indexed
        let regular = EventRow::new("MessageReceived", serde_json::json!({"text": "Hello"}));
        assert!(CognosEngine::memory_content_for_event(&regular).is_some());
    }
}

const HALF_LIFE_DAYS: f64 = 365.0;

/// Days elapsed since a timestamp, clamped to non-negative (handles clock skew).
pub(crate) fn age_in_days(
    now: chrono::DateTime<chrono::Utc>,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> f64 {
    (now - timestamp).num_seconds().max(0) as f64 / 86400.0
}

/// Compute combined relevance score for a memory entry.
/// Blends cosine similarity, importance, and time decay into a single score.
///
/// Time decay is gentle: halves the time factor after `HALF_LIFE_DAYS` (1 year).
/// `time_factor = 1.0 / (1.0 + age_days / 365.0)`
pub(crate) fn relevance_score(similarity: f64, importance: f32, age_days: f64) -> f64 {
    let clamped_age = age_days.max(0.0);
    let time_factor = 1.0 / (1.0 + clamped_age / HALF_LIFE_DAYS);
    similarity * (importance as f64) * time_factor
}

#[cfg(test)]
mod relevance_score_tests {
    use super::relevance_score;

    #[test]
    fn perfect_match_today_high_importance() {
        let score = relevance_score(1.0, 1.0, 0.0);
        assert!(
            (score - 1.0).abs() < f64::EPSILON,
            "perfect match today should be 1.0, got {}",
            score
        );
    }

    #[test]
    fn zero_similarity_means_zero_relevance() {
        let score = relevance_score(0.0, 1.0, 0.0);
        assert!(
            score.abs() < f64::EPSILON,
            "zero similarity should give zero relevance, got {}",
            score
        );
    }

    #[test]
    fn zero_importance_means_zero_relevance() {
        let score = relevance_score(0.8, 0.0, 0.0);
        assert!(
            score.abs() < f64::EPSILON,
            "zero importance should give zero relevance, got {}",
            score
        );
    }

    #[test]
    fn one_year_old_halves_time_factor() {
        let today = relevance_score(1.0, 1.0, 0.0);
        let one_year = relevance_score(1.0, 1.0, 365.0);
        // time_factor at 365 days = 1/(1+1) = 0.5
        assert!(
            (one_year - today * 0.5).abs() < 1e-10,
            "1 year old should be half of today's score: today={}, 1yr={}",
            today,
            one_year
        );
    }

    #[test]
    fn recent_high_importance_beats_old_low_importance() {
        // Recent (1 day), high importance (0.9), moderate similarity (0.7)
        let recent = relevance_score(0.7, 0.9, 1.0);
        // Old (300 days), low importance (0.35), same similarity
        let old = relevance_score(0.7, 0.35, 300.0);
        assert!(
            recent > old,
            "recent high-importance ({}) should beat old low-importance ({})",
            recent,
            old
        );
    }

    #[test]
    fn very_relevant_old_fact_can_still_rank_high() {
        // Old (2 years) but perfect similarity and critical importance
        let old_critical = relevance_score(1.0, 1.0, 730.0);
        // Recent (1 day) but moderate similarity and medium importance
        let recent_meh = relevance_score(0.5, 0.5, 1.0);
        // time_factor at 730 days = 1/3 ≈ 0.33, so old_critical ≈ 0.33
        // recent_meh ≈ 0.5 * 0.5 * ~1.0 = 0.25
        assert!(
            old_critical > recent_meh,
            "old critical fact ({}) should still beat recent mediocre ({})",
            old_critical,
            recent_meh
        );
    }

    #[test]
    fn importance_scales_linearly() {
        let low = relevance_score(0.8, 0.3, 10.0);
        let high = relevance_score(0.8, 0.9, 10.0);
        // Same similarity and age, 3x importance → 3x score (f32→f64 tolerance)
        assert!(
            (high / low - 3.0).abs() < 1e-6,
            "3x importance should give 3x score, got ratio {}",
            high / low
        );
    }

    #[test]
    fn negative_age_treated_as_zero() {
        // Edge case: clock skew could give negative age
        let score = relevance_score(1.0, 1.0, -5.0);
        // Should clamp to at most 1.0 (no boost from future dates)
        assert!(
            score <= 1.0 + f64::EPSILON,
            "negative age should not boost score above 1.0, got {}",
            score
        );
    }
}

#[cfg(test)]
mod age_in_days_tests {
    use super::age_in_days;
    use chrono::{Duration, Utc};

    #[test]
    fn same_timestamp_is_zero() {
        let now = Utc::now();
        assert!(age_in_days(now, now).abs() < f64::EPSILON);
    }

    #[test]
    fn one_day_ago() {
        let now = Utc::now();
        let yesterday = now - Duration::days(1);
        let age = age_in_days(now, yesterday);
        assert!((age - 1.0).abs() < 0.001, "expected ~1.0, got {}", age);
    }

    #[test]
    fn future_timestamp_clamped_to_zero() {
        let now = Utc::now();
        let future = now + Duration::hours(5);
        let age = age_in_days(now, future);
        assert!(
            age.abs() < f64::EPSILON,
            "future timestamp should give 0.0 age, got {}",
            age
        );
    }

    #[test]
    fn fractional_days() {
        let now = Utc::now();
        let twelve_hours_ago = now - Duration::hours(12);
        let age = age_in_days(now, twelve_hours_ago);
        assert!(
            (age - 0.5).abs() < 0.001,
            "12 hours should be ~0.5 days, got {}",
            age
        );
    }

    #[test]
    fn large_age() {
        let now = Utc::now();
        let five_years_ago = now - Duration::days(5 * 365);
        let age = age_in_days(now, five_years_ago);
        assert!(
            (age - 1825.0).abs() < 1.0,
            "5 years should be ~1825 days, got {}",
            age
        );
    }
}

#[cfg(test)]
mod jaccard_tests {
    use super::jaccard_similarity;

    #[test]
    fn identical_strings() {
        assert!((jaccard_similarity("hello world", "hello world") - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn no_overlap() {
        assert!(jaccard_similarity("hello world", "foo bar").abs() < f32::EPSILON);
    }

    #[test]
    fn partial_overlap() {
        // "hello world foo" vs "hello world bar" → intersection={"hello","world"}, union={"hello","world","foo","bar"}
        let sim = jaccard_similarity("hello world foo", "hello world bar");
        assert!((sim - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn empty_strings() {
        assert!((jaccard_similarity("", "") - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn high_overlap_above_threshold() {
        // 4 out of 5 words shared → 0.8
        let sim = jaccard_similarity(
            "the user works on projects daily",
            "the user works on projects weekly",
        );
        assert!(sim > 0.7);
    }

    // --- Tests for the user's concern: distinct same-topic facts must NOT be deduped ---

    #[test]
    fn started_vs_completed_same_project_survives_dedup() {
        // "Started X" vs "Completed X" — distinct lifecycle events
        let sim = jaccard_similarity(
            "Started the CognOS migration to PostgreSQL",
            "Completed the CognOS migration to PostgreSQL",
        );
        assert!(
            sim <= 0.8,
            "started vs completed should survive 0.8 dedup threshold, got {:.3}",
            sim
        );
    }

    #[test]
    fn same_person_different_meetings_survives_dedup() {
        let sim = jaccard_similarity(
            "Meeting with Sarah about the Q2 roadmap",
            "Meeting with Sarah about the Q3 roadmap",
        );
        assert!(
            sim <= 0.8,
            "different meetings should survive dedup, got {:.3}",
            sim
        );
    }

    #[test]
    fn same_project_different_versions_survives_dedup() {
        let sim = jaccard_similarity(
            "Deployed CognOS v2.1 to production",
            "Deployed CognOS v2.2 to production",
        );
        assert!(
            sim <= 0.8,
            "different versions should survive dedup, got {:.3}",
            sim
        );
    }

    #[test]
    fn same_system_different_tech_survives_dedup() {
        let sim = jaccard_similarity(
            "Uses Rust for the CognOS engine backend",
            "Uses TypeScript for the CognOS frontend",
        );
        assert!(
            sim <= 0.8,
            "different tech choices should survive dedup, got {:.3}",
            sim
        );
    }

    #[test]
    fn same_topic_different_preferences_survives_dedup() {
        let sim = jaccard_similarity(
            "Prefers dark mode in the CognOS interface",
            "Prefers vim keybindings in the CognOS interface",
        );
        assert!(
            sim <= 0.8,
            "different preferences should survive dedup, got {:.3}",
            sim
        );
    }

    #[test]
    fn near_duplicate_with_few_extra_words_below_threshold() {
        // 7 shared out of 9 total = 0.778 — below 0.8 threshold.
        // Adding 2+ words to a 7-word fact drops below dedup.
        // This means the 0.8 threshold is strict, which is conservative (fewer false dedup).
        let sim = jaccard_similarity(
            "Works at Finn as a software engineer",
            "Works at Finn as a software engineer in Oslo",
        );
        assert!(
            sim > 0.7 && sim < 0.8,
            "expected ~0.778 (7/9), got {:.3}",
            sim
        );
    }

    #[test]
    fn verbatim_duplicate_gets_deduped() {
        let sim = jaccard_similarity(
            "Started the habit tracker skill project",
            "Started the habit tracker skill project",
        );
        assert!(
            (sim - 1.0).abs() < f32::EPSILON,
            "identical facts should have Jaccard 1.0, got {:.3}",
            sim
        );
    }

    #[test]
    fn single_word_swap_in_short_fact_survives_dedup() {
        // 5 shared out of 7 union = 5/7 ≈ 0.714 — below 0.8 threshold.
        // This shows the 0.8 threshold is conservative: even a single word swap
        // in a 6-word fact drops below it. Only near-verbatim duplicates are removed.
        let sim = jaccard_similarity(
            "Started the habit tracker skill project",
            "Started the habit tracker skill development",
        );
        assert!(
            sim < 0.8,
            "single word swap should survive 0.8 threshold, got {:.3}",
            sim
        );
    }

    #[test]
    fn long_fact_single_word_swap_gets_deduped() {
        // With longer facts, a single word swap is a smaller portion of the total,
        // so the Jaccard stays above 0.8.
        // 11 words, 1 different: 10/12 ≈ 0.833
        let sim = jaccard_similarity(
            "Kenneth completed the migration of the CognOS backend to PostgreSQL successfully",
            "Kenneth completed the migration of the CognOS backend to PostgreSQL yesterday",
        );
        assert!(
            sim > 0.8,
            "single word swap in 11-word fact should be deduped, got {:.3}",
            sim
        );
    }

    #[test]
    fn completely_different_facts_same_topic_survives() {
        // Both about "CognOS" topic but completely different facts
        let sim = jaccard_similarity(
            "CognOS uses event sourcing with PostgreSQL",
            "CognOS desktop app built with Tauri framework",
        );
        assert!(
            sim <= 0.8,
            "different facts about same project should survive dedup, got {:.3}",
            sim
        );
    }

    #[test]
    fn similar_structure_different_entities_survives() {
        // Same sentence structure, different entities
        let sim = jaccard_similarity(
            "Had a meeting with Kenneth about the migration plan",
            "Had a meeting with Sarah about the deployment strategy",
        );
        assert!(
            sim <= 0.8,
            "same structure but different entities should survive dedup, got {:.3}",
            sim
        );
    }
}
