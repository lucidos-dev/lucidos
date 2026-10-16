use super::super::LucidosEngine;
use crate::engine::event_bus::EventBus;
use crate::engine::memory::correction::{index_correction, record_correction};
use crate::engine::memory::MEMORY_CORRECTION_THRESHOLD;
use crate::engine::AuxCapture;
use crate::llm::provider::LlmProvider;
use crate::llm::{Message, MessageContent};
use crate::memory::{cosine_similarity, CorrectedMemory, EmbeddingProvider};

/// Which candidate entries express the wrong fact, 0-indexed, or `None` when
/// the call failed. A failure aborts the correction: deleting a memory on a
/// guess is the one outcome worse than deleting nothing.
///
/// A free function so a stubbed provider drives it offline. The handler around
/// it needs an index, an embedder and a live engine.
///
/// `capture` makes the call and records it before the answer is read, so a
/// verdict nobody could parse is still accounted.
pub(crate) async fn verify_entries<P: LlmProvider + ?Sized>(
    provider: &P,
    prompt: String,
    candidates: usize,
    capture: &AuxCapture,
) -> Option<Vec<usize>> {
    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text(prompt),
    }];
    let response = match capture
        .chat(
            provider,
            messages,
            vec![],
            crate::llm::ModelSelection::default(),
            None,
            None,
        )
        .await
    {
        Ok(response) => response,
        Err(e) => {
            log!(@Memory, "[correct_memory] LLM batch verification failed: {}. Aborting to be safe", e);
            return None;
        }
    };
    let answer = response
        .content
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    log!(@Memory, "[correct_memory] LLM batch verdict: '{}'", &answer[..answer.floor_char_boundary(100)]);
    if answer.starts_with("none") || answer.is_empty() {
        return Some(vec![]);
    }
    // Comma-separated and 1-indexed, as the prompt asked for.
    Some(
        answer
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter_map(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n >= 1 && n <= candidates)
            .map(|n| n - 1)
            .collect(),
    )
}

impl LucidosEngine {
    /// `thread_id` anchors the capture for the verdict call this makes. The
    /// tool dispatcher holds it, so the spend is never filed against nothing.
    pub(crate) async fn execute_memory_tool(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let search_query = args["search_query"].as_str().unwrap_or("");
        let wrong_fact = args
            .get("wrong_fact")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if search_query.is_empty() {
            return Ok("Error: search_query is required".to_string());
        }
        if wrong_fact.is_empty() {
            return Ok(
                "Error: wrong_fact is required — describe the specific wrong claim".to_string(),
            );
        }

        let Some(ref index) = self.memory_index else {
            return Ok("Error: memory system not available".to_string());
        };

        // Search for candidate memories by keyword
        let results = index
            .search_by_keyword(search_query, 0.0, 100)
            .await
            .map_err(|e| format!("Search failed: {}", e))?;

        if results.entries.is_empty() {
            return Ok(format!("No memories found matching '{}'", search_query));
        }

        // Embed the wrong_fact and all candidates for similarity filtering
        let wrong_embedding = self
            .embedder
            .embed(wrong_fact)
            .await
            .map_err(|e| format!("Failed to embed wrong_fact: {}", e))?;

        let candidate_embeddings = self
            .embedder
            .embed_batch(
                &results
                    .entries
                    .iter()
                    .map(|e| e.summary.as_str())
                    .collect::<Vec<_>>(),
            )
            .await
            .map_err(|e| format!("Failed to embed candidates: {}", e))?;

        // Collect candidates that pass the similarity threshold
        let mut candidates: Vec<(usize, f32)> = Vec::new();
        let mut kept_count: usize = 0;

        log!(@Memory, "[correct_memory] Found {} keyword candidates for '{}'", results.entries.len(), search_query);

        for (i, entry) in results.entries.iter().enumerate() {
            if i < candidate_embeddings.len() {
                let similarity = cosine_similarity(&wrong_embedding, &candidate_embeddings[i]);
                let truncated = &entry.summary[..entry.summary.floor_char_boundary(80)];
                log!(@Memory, "[correct_memory]   score={:.3} '{}'", similarity, truncated);
                if similarity >= MEMORY_CORRECTION_THRESHOLD {
                    candidates.push((i, similarity));
                } else {
                    kept_count += 1;
                }
            }
        }

        log!(@Memory, "[correct_memory] {} of {} candidates passed similarity filter (threshold={})",
            candidates.len(), results.entries.len(), MEMORY_CORRECTION_THRESHOLD);

        if candidates.is_empty() {
            return Ok(format!(
                "Found {} memories matching '{}', but none are semantically similar to the wrong fact '{}'. No changes made.",
                results.entries.len(), search_query, wrong_fact
            ));
        }

        // Safety cap
        const MAX_DELETIONS: usize = 10;
        if candidates.len() > MAX_DELETIONS {
            log!(@Memory, "[correct_memory] BLOCKED: {} matches exceeds safety cap of {}", candidates.len(), MAX_DELETIONS);
            return Ok(format!(
                "Too many matches ({} entries). The wrong_fact may be too broad. Please be more specific.",
                candidates.len()
            ));
        }

        // Sort by similarity descending
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Build numbered list for batch LLM verification
        let mut verify_list = String::new();
        for (list_idx, &(entry_idx, _)) in candidates.iter().enumerate() {
            verify_list.push_str(&format!(
                "{}. {}\n",
                list_idx + 1,
                results.entries[entry_idx].summary
            ));
        }

        let verify_prompt = format!(
            r#"The user says this fact is WRONG and should be removed from memory:
"{wrong_fact}"

Below are memory entries that might contain this wrong fact. For each entry, answer "yes" if it contains or directly expresses the specific wrong claim, or "no" if it merely mentions the same person/place but is about something else (contact info, finances, other dates, etc.).

{verify_list}
Reply with ONLY the numbers of entries that should be deleted, comma-separated. Example: "1, 3, 5"
If NONE should be deleted, reply with "none"."#,
            wrong_fact = wrong_fact,
            verify_list = verify_list,
        );

        let capture = crate::engine::AuxCapture::new(
            &self.event_bus,
            thread_id,
            crate::engine::ContextPurpose::MemoryCorrection,
        );
        let Some(verified_indices) = verify_entries(
            self.current_provider().as_ref(),
            verify_prompt,
            candidates.len(),
            &capture,
        )
        .await
        else {
            return Ok("Memory correction aborted: could not verify which entries to delete. Please try again.".to_string());
        };

        if verified_indices.is_empty() {
            return Ok(format!(
                "Found {} candidate memories matching '{}', but LLM verification determined none actually express the wrong fact '{}'. No changes made.",
                candidates.len(), search_query, wrong_fact
            ));
        }

        let correction = args
            .get("correction")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        let removed: Vec<CorrectedMemory> = candidates
            .iter()
            .enumerate()
            .filter(|(list_idx, _)| verified_indices.contains(list_idx))
            .map(|(_, &(entry_idx, _))| CorrectedMemory::from(&results.entries[entry_idx]))
            .collect();
        let event_id = match record_correction(
            &self.event_bus,
            search_query,
            wrong_fact,
            removed,
            correction,
        )
        .await
        {
            Ok(id) => id,
            Err(e) => return Ok(unrecorded_correction(e)),
        };

        // Delete one at a time — only the LLM-verified entries
        let mut deleted_summaries: Vec<String> = Vec::new();
        let mut skipped_summaries: Vec<String> = Vec::new();
        let mut failed_summaries: Vec<String> = Vec::new();

        for (list_idx, &(entry_idx, _)) in candidates.iter().enumerate() {
            let entry = &results.entries[entry_idx];
            if verified_indices.contains(&list_idx) {
                match index.delete(entry.id).await {
                    Ok(true) => {
                        deleted_summaries.push(entry.summary.clone());
                        log!(@Memory, "[correct_memory]   DELETED id={}", entry.id);
                    }
                    Ok(false) => {
                        log!(@Memory, "[correct_memory]   Entry {} already gone", entry.id);
                    }
                    Err(e) => {
                        log!(@Memory, "[correct_memory]   Failed to delete {}: {}", entry.id, e);
                        failed_summaries.push(format!("{} ({})", entry.summary, e));
                    }
                }
            } else {
                skipped_summaries.push(entry.summary.clone());
            }
        }

        if let Some(correction_text) = correction {
            index_correction(
                index,
                self.embedder.as_ref(),
                event_id,
                correction_text,
                chrono::Utc::now(),
            )
            .await
            .map_err(|e| format!("Insert correction failed: {}", e))?;
        }

        let total = deleted_summaries.len() + skipped_summaries.len() + kept_count;
        let mut response = format!(
            "Deleted {} of {} memories matching '{}' (verified by LLM against '{}'):\n",
            deleted_summaries.len(),
            total,
            search_query,
            wrong_fact
        );
        for summary in &deleted_summaries {
            response.push_str(&format!("  - {}\n", summary));
        }
        if !skipped_summaries.is_empty() {
            response.push_str(&format!(
                "\nSkipped {} entries (LLM determined they don't express the wrong fact):\n",
                skipped_summaries.len()
            ));
            for summary in &skipped_summaries {
                response.push_str(&format!("  - {}\n", summary));
            }
        }
        if !failed_summaries.is_empty() {
            response.push_str(&format!(
                "\nFailed to delete {} entries:\n",
                failed_summaries.len()
            ));
            for summary in &failed_summaries {
                response.push_str(&format!("  - {}\n", summary));
            }
        }
        if kept_count > 0 {
            response.push_str(&format!(
                "\nKept {} unrelated memories (below similarity threshold).\n",
                kept_count
            ));
        }
        if let Some(c) = correction {
            response.push_str(&format!("\nAdded corrected fact: {}", c));
        }

        Ok(response)
    }

    /// Delete (and optionally replace) one memory entry by its exact id — the
    /// precise lane the `[Long-term Memory]` block's `[id: <uuid>]` enables. The
    /// fuzzy keyword+semantic path (`correct_memory` → `execute_memory_tool`)
    /// is deliberately left untouched.
    pub(crate) async fn execute_correct_memory_by_id(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let Some(ref index) = self.memory_index else {
            return Ok("Error: memory system not available".to_string());
        };
        correct_memory_by_id_impl(index, self.embedder.as_ref(), &self.event_bus, args).await
    }

    /// `memory` action `search`: the agent's own query against long-term
    /// memory, for when the pre-turn injection missed something it needs.
    ///
    /// Ranked by the SAME formula as that injection
    /// (`engine::memory::relevance_score`), so a search cannot come back in a
    /// different order from the facts already in context. Two orderings over
    /// one corpus is a contradiction the agent would have to resolve with
    /// nothing to resolve it by.
    pub(crate) async fn execute_search_memory(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let q = args.get("q").and_then(|v| v.as_str()).unwrap_or("").trim();
        if q.is_empty() {
            return Ok("Error: q is required. Ask what you want to know.".to_string());
        }
        let limit = args.get("limit").and_then(|v| v.as_i64());
        let found = self.search_memory_ranked(q, limit).await?;
        Ok(serde_json::to_string(&found)?)
    }

    /// `memory` action `source`: walk one memory back to the event it came
    /// from, which carries the thread that produced it.
    pub(crate) async fn execute_memory_source(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
        if id.is_empty() {
            return Ok(
                "Error: id is required. Copy the `[id: <uuid>]` from the memory.".to_string(),
            );
        }
        let Ok(uuid) = uuid::Uuid::parse_str(id) else {
            return Ok(format!("Error: '{id}' is not a uuid."));
        };
        match self.memory_source(uuid).await {
            Ok(found) => Ok(serde_json::to_string(&found)?),
            // Surfaced rather than returned as an Err so the agent reads the
            // instruction and re-asks, instead of the loop reporting a failure
            // it cannot act on. The artifact case is the one that matters: it
            // is not a missing memory, it is a memory with no conversation.
            Err(e) => Ok(format!("Error: {e}")),
        }
    }

    /// `threads` action `search`: find past threads by what was said in them.
    ///
    /// The capability the UI's search box already had and the agent did not,
    /// which is why "we talked about this" was unanswerable: `list` filters by
    /// status and channel and can never find a topic.
    pub(crate) async fn execute_search_threads(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let q = args.get("q").and_then(|v| v.as_str()).unwrap_or("").trim();
        if q.is_empty() {
            return Ok("Error: q is required. Say what was discussed.".to_string());
        }
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(20)
            .clamp(1, 50);
        let found = crate::engine::thread_search::combined_thread_search(self, q, limit).await?;
        // PROJECTED, never the raw `ThreadSearchResult`. That flattens ~30
        // bookkeeping fields the model has no use for. It also carries the
        // whole draft, and search may match a thread on its draft alone. So a
        // row says only whether the thread holds one; the `drafts` action
        // reads it. Each row carries its thread link, so the model can point
        // the user at a hit.
        let workspace = self.workspace_name();
        let projected: Vec<serde_json::Value> = found
            .iter()
            // The schema states 1-50, and the merge bounds each ARM rather than
            // the total, so the promise is kept here.
            .take(limit.max(0) as usize)
            .map(|r| {
                serde_json::json!({
                    // The id the model passes on: to the `events` tool's
                    // 'query' to read the thread, or to 'drafts' for its draft.
                    "thread_id": r.info.thread_id,
                    "title": r.info.title,
                    "last_activity": r.info.last_activity,
                    "message_count": r.info.message_count,
                    "channel": r.info.channel,
                    "has_draft": crate::core::store::has_draft(
                        &r.info.compose_text,
                        &r.info.compose_images,
                    ),
                    "link": crate::core::store::thread_link(&workspace, &r.info.thread_id),
                })
            })
            .collect();
        Ok(serde_json::to_string(&projected)?)
    }
}

/// The tool reply when the correction event could not be written. Nothing is
/// deleted then: a correction that is not recorded comes back on a rebuild.
fn unrecorded_correction(e: Box<dyn std::error::Error + Send + Sync>) -> String {
    format!(
        "Memory correction aborted: could not record it ({e}). No changes made. Please try again."
    )
}

/// Parse the `id` arg for `correct_memory_by_id`. Accepts a bare UUID
/// (hyphenated or simple) and defensively tolerates a `mem-` prefix and
/// surrounding whitespace. Returns the tool-facing error string on failure so
/// the caller can hand it straight back to the model.
pub(crate) fn parse_memory_entry_id(args: &serde_json::Value) -> Result<uuid::Uuid, String> {
    let raw =
        match args.get("id").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim(),
            _ => return Err(
                "Error: id is required. Copy it from the `[id: <uuid>]` shown on the memory bullet"
                    .to_string(),
            ),
        };
    let stripped = raw.strip_prefix("mem-").unwrap_or(raw);
    uuid::Uuid::parse_str(stripped).map_err(|_| {
        format!("Error: id must be a memory entry UUID as shown in `[id: <uuid>]` (got '{raw}')")
    })
}

/// Core of `correct_memory_by_id`, factored out of the `LucidosEngine` impl so
/// tests can exercise the delete / not-found / delete-plus-correction branches
/// against a real Postgres pool with a mock embedder — no full engine, and no
/// LLM provider (the id lane skips the semantic verification `correct_memory`
/// needs).
pub(crate) async fn correct_memory_by_id_impl(
    index: &crate::memory::PgVectorIndex,
    embedder: &dyn EmbeddingProvider,
    bus: &EventBus,
    args: &serde_json::Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let id = match parse_memory_entry_id(args) {
        Ok(id) => id,
        Err(msg) => return Ok(msg),
    };

    // Confirm the target exists first: lets us echo what was removed and tell a
    // real delete apart from a no-op on a stale / hallucinated id.
    let Some(entry) = index
        .get_by_id(id)
        .await
        .map_err(|e| format!("Lookup failed: {}", e))?
    else {
        return Ok(format!(
            "No memory entry with id {}. It may already be gone — use correct_memory to search by keyword if you're unsure.",
            id
        ));
    };

    let correction = args
        .get("correction")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // The entry's own summary is the wrong fact: the user pointed at it.
    let removed = vec![CorrectedMemory::from(&entry)];
    let event_id =
        match record_correction(bus, &entry.summary, &entry.summary, removed, correction).await {
            Ok(id) => id,
            Err(e) => return Ok(unrecorded_correction(e)),
        };

    let deleted = index
        .delete(id)
        .await
        .map_err(|e| format!("Delete failed: {}", e))?;
    // Recorded either way, so the correction below still lands.
    let mut response = if deleted {
        log!(@Memory, "[correct_memory_by_id] DELETED id={} topic={:?}", id, entry.topic);
        format!("Deleted memory entry {}:\n  - {}", id, entry.summary)
    } else {
        format!("Memory entry {} was already gone.", id)
    };

    if let Some(correction_text) = correction {
        index_correction(
            index,
            embedder,
            event_id,
            correction_text,
            chrono::Utc::now(),
        )
        .await
        .map_err(|e| format!("Insert correction failed: {}", e))?;
        response.push_str(&format!("\n\nAdded corrected fact: {}", correction_text));
    }

    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::EventBus;
    use crate::memory::{MemorySource, PgVectorIndex};
    use crate::test_support::{
        aux_captures, setup_test_db, teardown_test_db, Fixed384Embedder, ScriptedProvider,
    };
    use serde_json::json;
    use uuid::Uuid;

    /// The verdict call decides which memories get deleted, and it spent real
    /// tokens doing it on the agent's own chat model.
    #[tokio::test]
    async fn the_correction_verdict_records_what_it_cost() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let capture = AuxCapture::new(
            &bus,
            thread_id,
            crate::engine::ContextPurpose::MemoryCorrection,
        );

        let provider = ScriptedProvider::new("claude-opus-4-5", vec!["1, 3"]);
        let verified = verify_entries(&provider, "which of these?".to_string(), 3, &capture)
            .await
            .expect("the scripted reply parses");
        assert_eq!(
            verified,
            vec![0, 2],
            "the numbers are 1-indexed on the wire"
        );

        let captures = aux_captures(&pool, thread_id, "memory_correction").await;
        assert_eq!(captures.len(), 1, "one call, one row: {captures:?}");
        assert_eq!(captures[0]["producer"], "auxiliary");
        assert_eq!(captures[0]["model"], "claude-opus-4-5");
        assert_eq!(captures[0]["usage"]["input_tokens"], 210);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A verdict of "none" still cost what it cost. Only a call that never
    /// reached the provider leaves no row.
    #[tokio::test]
    async fn a_verdict_of_none_is_still_recorded() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let capture = AuxCapture::new(
            &bus,
            thread_id,
            crate::engine::ContextPurpose::MemoryCorrection,
        );

        let provider = ScriptedProvider::new("claude-opus-4-5", vec!["none"]);
        let verified = verify_entries(&provider, "which of these?".to_string(), 3, &capture)
            .await
            .expect("the scripted reply parses");
        assert!(verified.is_empty());
        assert_eq!(
            aux_captures(&pool, thread_id, "memory_correction")
                .await
                .len(),
            1
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    // --- parse_memory_entry_id (pure) ---

    #[test]
    fn parse_accepts_hyphenated_uuid() {
        let id = Uuid::new_v4();
        let args = json!({ "id": id.to_string() });
        assert_eq!(parse_memory_entry_id(&args).unwrap(), id);
    }

    #[test]
    fn parse_accepts_simple_uuid() {
        let id = Uuid::new_v4();
        let args = json!({ "id": id.simple().to_string() });
        assert_eq!(parse_memory_entry_id(&args).unwrap(), id);
    }

    #[test]
    fn parse_tolerates_mem_prefix_and_whitespace() {
        let id = Uuid::new_v4();
        let args = json!({ "id": format!("  mem-{}  ", id) });
        assert_eq!(parse_memory_entry_id(&args).unwrap(), id);
    }

    #[test]
    fn parse_rejects_missing_id() {
        let err = parse_memory_entry_id(&json!({})).unwrap_err();
        assert!(err.contains("id is required"), "{err}");
    }

    #[test]
    fn parse_rejects_garbage() {
        let err = parse_memory_entry_id(&json!({ "id": "not-a-uuid" })).unwrap_err();
        assert!(err.contains("must be a memory entry UUID"), "{err}");
    }

    // --- correct_memory_by_id_impl (real PG + mock embedder) ---

    async fn insert(index: &PgVectorIndex, summary: &str) -> Uuid {
        let source = MemorySource::Event { id: Uuid::new_v4() };
        insert_at(index, summary, &source, chrono::Utc::now()).await
    }

    async fn insert_at(
        index: &PgVectorIndex,
        summary: &str,
        source: &MemorySource,
        src_created_at: chrono::DateTime<chrono::Utc>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        index
            .index_entry(
                id,
                source,
                "Config",
                summary,
                0.8,
                &[],
                &vec![0.1f32; 384],
                "test-fixed-384",
                src_created_at,
                crate::memory::EXTRACTOR_VERSION,
            )
            .await
            .unwrap();
        id
    }

    async fn summaries(index: &PgVectorIndex, keyword: &str) -> Vec<String> {
        let mut found: Vec<String> = index
            .search_by_keyword(keyword, 0.0, 100)
            .await
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.summary)
            .collect();
        found.sort();
        found
    }

    #[tokio::test]
    async fn delete_by_id_removes_only_the_target() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();
        let (bus, _rx) = EventBus::new(pool.clone());

        let target = insert(&index, "Config dir is at gws-personal").await;
        let bystander = insert(&index, "User prefers dark theme").await;

        let out = correct_memory_by_id_impl(
            &index,
            &Fixed384Embedder,
            &bus,
            &json!({ "id": target.to_string() }),
        )
        .await
        .unwrap();
        assert!(out.contains("Deleted memory entry"), "{out}");
        assert!(
            out.contains("gws-personal"),
            "echoes the deleted summary: {out}"
        );

        assert!(
            index.get_by_id(target).await.unwrap().is_none(),
            "target should be deleted"
        );
        assert!(
            index.get_by_id(bystander).await.unwrap().is_some(),
            "bystander must be untouched"
        );

        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn unknown_id_is_a_safe_no_op() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();
        let (bus, _rx) = EventBus::new(pool.clone());

        let survivor = insert(&index, "User prefers dark theme").await;
        let out = correct_memory_by_id_impl(
            &index,
            &Fixed384Embedder,
            &bus,
            &json!({ "id": Uuid::new_v4().to_string() }),
        )
        .await
        .unwrap();
        assert!(out.contains("No memory entry with id"), "{out}");
        assert!(
            index.get_by_id(survivor).await.unwrap().is_some(),
            "nothing should be deleted"
        );
        assert_eq!(index.len().await.unwrap(), 1);

        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn delete_with_correction_replaces_the_fact() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();
        let (bus, _rx) = EventBus::new(pool.clone());

        let target = insert(&index, "Config dir is at gws-personal").await;
        let out = correct_memory_by_id_impl(
            &index,
            &Fixed384Embedder,
            &bus,
            &json!({
                "id": target.to_string(),
                "correction": "The gws-personal config dir was deleted",
            }),
        )
        .await
        .unwrap();
        assert!(out.contains("Added corrected fact"), "{out}");

        assert!(
            index.get_by_id(target).await.unwrap().is_none(),
            "old entry should be gone"
        );

        // The replacement is stored under the Memory Correction topic.
        let stored: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM memory_entries WHERE topic = 'Memory Correction' AND summary = $1",
        )
        .bind("The gws-personal config dir was deleted")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stored, 1, "correction fact should be stored");

        teardown_test_db(&db_name).await;
    }

    /// A full rebuild clears memory and re-extracts from the same source
    /// events, so the extractor finds the wrong fact again. The correction must
    /// be replayed over it, or every fix the user made silently comes back.
    #[tokio::test]
    async fn a_rebuild_does_not_resurrect_a_corrected_fact() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();
        let (bus, _rx) = EventBus::new(pool.clone());
        let wrong = "User works at Acme Corp";
        let right = "User works at Globex";

        let target = insert(&index, wrong).await;
        let source = index.get_by_id(target).await.unwrap().unwrap().source;
        correct_memory_by_id_impl(
            &index,
            &Fixed384Embedder,
            &bus,
            &json!({ "id": target.to_string(), "correction": right }),
        )
        .await
        .unwrap();

        let corrections = crate::core::EventStore::new(pool.clone())
            .events_of_type_chronological(crate::engine::memory::correction::MEMORY_CORRECTED)
            .await
            .unwrap();
        assert_eq!(corrections.len(), 1, "the correction must be recorded");
        let corrected_at = corrections[0].created;

        // The forced rebuild: clear, then the extractor returns the wrong fact
        // from its original source event, which predates the correction.
        index.clear().await.unwrap();
        insert_at(
            &index,
            wrong,
            &source,
            corrected_at - chrono::Duration::hours(1),
        )
        .await;
        crate::engine::memory::correction::replay_corrections(
            &index,
            &Fixed384Embedder,
            &corrections,
        )
        .await;

        assert_eq!(summaries(&index, "Acme").await, Vec::<String>::new());
        assert_eq!(summaries(&index, "Globex").await, vec![right.to_string()]);

        teardown_test_db(&db_name).await;
    }
}
