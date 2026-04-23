use super::super::CognosEngine;
use crate::engine::memory::MEMORY_CORRECTION_THRESHOLD;
use crate::llm::{Message, MessageContent};
use crate::memory::{cosine_similarity, EmbeddingProvider};

impl CognosEngine {
    pub(crate) async fn execute_memory_tool(
        &self,
        args: &serde_json::Value,
        _extraction_ctx: &str,
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
            .llm
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
}
