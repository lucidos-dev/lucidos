use super::super::LucidosEngine;
use crate::engine::memory::MEMORY_CORRECTION_THRESHOLD;
use crate::llm::{Message, MessageContent};
use crate::memory::{cosine_similarity, EmbeddingProvider};

impl LucidosEngine {
    pub(crate) async fn execute_memory_tool(
        &self,
        args: &serde_json::Value,
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

        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(verify_prompt),
        }];

        let verified_indices: Vec<usize> = match self
            .current_provider()
            .chat(messages, vec![], None, None, None, None)
            .await
        {
            Ok(response) => {
                let answer = response
                    .content
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .to_lowercase();
                log!(@Memory, "[correct_memory] LLM batch verdict: '{}'", &answer[..answer.floor_char_boundary(100)]);

                if answer.starts_with("none") || answer.is_empty() {
                    vec![]
                } else {
                    // Parse comma-separated numbers (1-indexed)
                    answer
                        .split(|c: char| c == ',' || c.is_whitespace())
                        .filter_map(|s| s.trim().parse::<usize>().ok())
                        .filter(|&n| n >= 1 && n <= candidates.len())
                        .map(|n| n - 1) // Convert to 0-indexed
                        .collect()
                }
            }
            Err(e) => {
                log!(@Memory, "[correct_memory] LLM batch verification failed: {} — aborting to be safe", e);
                return Ok("Memory correction aborted: could not verify which entries to delete. Please try again.".to_string());
            }
        };

        if verified_indices.is_empty() {
            return Ok(format!(
                "Found {} candidate memories matching '{}', but LLM verification determined none actually express the wrong fact '{}'. No changes made.",
                candidates.len(), search_query, wrong_fact
            ));
        }

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

        if deleted_summaries.is_empty() {
            let errors = if failed_summaries.is_empty() {
                String::new()
            } else {
                format!(
                    "\nFailed entries:\n{}",
                    failed_summaries
                        .iter()
                        .map(|s| format!("  - {}", s))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            };
            return Ok(format!(
                "No memories were deleted (all deletions failed). Please try again.{}",
                errors
            ));
        }

        // Optionally add corrected fact
        let correction = args
            .get("correction")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        if let Some(correction_text) = correction {
            let embedding = self
                .embedder
                .embed(correction_text)
                .await
                .map_err(|e| format!("Embedding failed: {}", e))?;

            let fact_id = uuid::Uuid::new_v4();
            let source = crate::memory::pgvector::MemorySource::Event { id: fact_id };
            index
                .index_entry(
                    fact_id,
                    &source,
                    "Memory Correction",
                    correction_text,
                    0.8,
                    &[],
                    &embedding,
                    self.embedder.model_id(),
                    chrono::Utc::now(),
                    crate::memory::EXTRACTOR_VERSION,
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
        correct_memory_by_id_impl(index, self.embedder.as_ref(), args).await
    }
}

/// Parse the `id` arg for `correct_memory_by_id`. Accepts a bare UUID
/// (hyphenated or simple) and defensively tolerates a `mem-` prefix and
/// surrounding whitespace. Returns the tool-facing error string on failure so
/// the caller can hand it straight back to the model.
pub(crate) fn parse_memory_entry_id(args: &serde_json::Value) -> Result<uuid::Uuid, String> {
    let raw = match args.get("id").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => {
            return Err(
                "Error: id is required — copy it from the `[id: <uuid>]` shown on the memory bullet"
                    .to_string(),
            )
        }
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

    let deleted = index
        .delete(id)
        .await
        .map_err(|e| format!("Delete failed: {}", e))?;
    if !deleted {
        return Ok(format!(
            "Memory entry {} was already gone — no changes made.",
            id
        ));
    }
    log!(@Memory, "[correct_memory_by_id] DELETED id={} topic={:?}", id, entry.topic);

    let correction = args
        .get("correction")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let mut response = format!("Deleted memory entry {}:\n  - {}", id, entry.summary);

    if let Some(correction_text) = correction {
        let embedding = embedder
            .embed(correction_text)
            .await
            .map_err(|e| format!("Embedding failed: {}", e))?;
        let fact_id = uuid::Uuid::new_v4();
        let source = crate::memory::pgvector::MemorySource::Event { id: fact_id };
        index
            .index_entry(
                fact_id,
                &source,
                "Memory Correction",
                correction_text,
                0.8,
                &[],
                &embedding,
                embedder.model_id(),
                chrono::Utc::now(),
                crate::memory::EXTRACTOR_VERSION,
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
    use crate::memory::{MemorySource, PgVectorIndex};
    use crate::test_support::{setup_test_db, teardown_test_db};
    use async_trait::async_trait;
    use serde_json::json;
    use uuid::Uuid;

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

    /// pgvector stores `vector(384)`, so the correction path needs a 384-dim
    /// embedder; `KeywordEmbedder` would emit `keywords.len()` dims. The delete
    /// path ignores the embedder entirely.
    struct Fixed384Embedder;
    #[async_trait]
    impl EmbeddingProvider for Fixed384Embedder {
        async fn embed(
            &self,
            _text: &str,
        ) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(vec![0.1f32; 384])
        }
        async fn embed_batch(
            &self,
            texts: &[&str],
        ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(texts.iter().map(|_| vec![0.1f32; 384]).collect())
        }
        fn dimensions(&self) -> usize {
            384
        }
        fn model_id(&self) -> &str {
            "test-fixed-384"
        }
    }

    async fn insert(index: &PgVectorIndex, summary: &str) -> Uuid {
        let id = Uuid::new_v4();
        index
            .index_entry(
                id,
                &MemorySource::Event { id: Uuid::new_v4() },
                "Config",
                summary,
                0.8,
                &[],
                &vec![0.1f32; 384],
                "test-fixed-384",
                chrono::Utc::now(),
                crate::memory::EXTRACTOR_VERSION,
            )
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn delete_by_id_removes_only_the_target() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();

        let target = insert(&index, "Config dir is at gws-personal").await;
        let bystander = insert(&index, "User prefers dark theme").await;

        let out = correct_memory_by_id_impl(
            &index,
            &Fixed384Embedder,
            &json!({ "id": target.to_string() }),
        )
        .await
        .unwrap();
        assert!(out.contains("Deleted memory entry"), "{out}");
        assert!(out.contains("gws-personal"), "echoes the deleted summary: {out}");

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

        let survivor = insert(&index, "User prefers dark theme").await;
        let out = correct_memory_by_id_impl(
            &index,
            &Fixed384Embedder,
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

        let target = insert(&index, "Config dir is at gws-personal").await;
        let out = correct_memory_by_id_impl(
            &index,
            &Fixed384Embedder,
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
}
