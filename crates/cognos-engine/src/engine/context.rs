use super::memory::jaccard_similarity;
use super::CognosEngine;
use crate::llm::{ContentBlock, Message, MessageContent};
use crate::memory::{EmbeddingProvider, MemoryEntry, QueryClassification};
use uuid::Uuid;

/// Max character budget for messages sent to the LLM.
/// ~143K tokens at 3.5 chars/token, leaves room for system prompt + tools + response.
/// Character budget for the entire LLM request (system prompt + tools + messages).
/// JSON-heavy tool calls average ~1.5 chars/token (not ~3.5 like prose), so this
/// must be conservative. 300k chars ≈ 150–200k tokens depending on content mix.
pub(super) const AGENT_CONTEXT_CHAR_BUDGET: usize = 300_000;

/// Number of tail messages to always preserve (2 assistant+user pairs).
pub(super) const PRESERVE_RECENT_MESSAGES: usize = 4;

/// Compress conversation history when more than this many messages exist.
pub(super) const HISTORY_COMPRESS_THRESHOLD: usize = 15;

/// Always keep the last N messages verbatim.
pub(super) const HISTORY_RECENT_MESSAGES: usize = 15;

/// Hard safety-net truncation for individual messages — only catches extreme outliers
/// (e.g., someone pasting a 50K log dump). Normal messages are never touched by this.
pub(super) const HISTORY_MSG_TRUNCATE: usize = 15_000;

/// Last N messages are always kept fully verbatim (no compaction, only safety-net truncation).
pub(super) const HISTORY_VERBATIM_TAIL: usize = 4;

/// Assistant messages outside the verbatim tail are compacted to this limit.
/// User messages are never compacted — their exact phrasing matters for follow-ups.
pub(super) const HISTORY_ASSISTANT_COMPACT: usize = 1500;

/// Max bytes for a single read_file result returned to the LLM.
pub(super) const READ_FILE_MAX_BYTES: usize = 50_000;

/// Minimum content size before considering truncation of a single value.
pub(super) const TRUNCATION_THRESHOLD: usize = 500;

/// Sanitize file content before returning it to the LLM:
/// 1. Strip base64 data URIs (e.g. embedded images) — they burn tokens and the LLM can't use them.
/// 2. Apply `offset` (byte index, snapped down to a char boundary) for chunked reads.
/// 3. Truncate the returned slice to READ_FILE_MAX_BYTES; the trailing message tells the
///    LLM the exact `offset=` to pass on the next `read_file` call to continue.
pub(super) fn sanitize_file_content_for_llm(content: String, path: &str, offset: usize) -> String {
    static DATA_URI_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"data:[a-zA-Z0-9]+/[a-zA-Z0-9.+-]+;base64,[A-Za-z0-9+/=]+")
            .expect("data URI regex must compile")
    });

    let original_len = content.len();

    // Strip base64 data URIs: data:image/png;base64,iVBOR... → [embedded image, 42KB]
    let sanitized = DATA_URI_RE
        .replace_all(&content, |caps: &regex::Captures| {
            let matched = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            let kb = matched.len() / 1024;
            format!("[embedded image, {}KB]", kb)
        })
        .into_owned();

    if sanitized.len() != original_len {
        let stripped_kb = (original_len - sanitized.len()) / 1024;
        log!(
            "read_file '{}': stripped {}KB of base64 image data ({} → {} bytes)",
            path,
            stripped_kb,
            original_len,
            sanitized.len()
        );
    }

    let total = sanitized.len();

    // offset == total is the natural EOF sentinel (the previous chunk's continuation
    // offset for an exact-multiple file lands here). Only > total is a usage error.
    if offset > total {
        return format!(
            "Error: offset {} is past end of file ({} bytes total).",
            offset, total
        );
    }

    let start = sanitized.floor_char_boundary(offset);
    let remaining = &sanitized[start..];

    if remaining.len() <= READ_FILE_MAX_BYTES {
        return remaining.to_string();
    }

    let chunk_end_rel = remaining.floor_char_boundary(READ_FILE_MAX_BYTES);
    let next_offset = start + chunk_end_rel;
    let chunk = &remaining[..chunk_end_rel];

    log!(
        "read_file '{}': returning bytes {}–{} of {}",
        path,
        start,
        next_offset,
        total
    );

    format!(
        "{chunk}\n\n[Truncated. Showing bytes {start}–{next_offset} of {total}. \
         Call read_file with offset={next_offset} to continue.]"
    )
}

/// Estimate the total character count of all content in a message.
pub(super) fn estimate_message_chars(message: &Message) -> usize {
    match &message.content {
        MessageContent::Text(s) => s.len(),
        MessageContent::Blocks(blocks) => {
            blocks
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => text.len(),
                    ContentBlock::ToolUse { id, name, input } => {
                        id.len() + name.len() + input.to_string().len()
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                    } => tool_use_id.len() + content.len(),
                    ContentBlock::Image { data, .. } => {
                        // Estimate: base64 data is ~4/3 of original size, count as token-heavy
                        data.len()
                    }
                })
                .sum()
        }
    }
}

/// Trim the agent loop message history to fit within a character budget.
///
/// Two-pass approach:
/// 1. Truncate large tool results/inputs in old messages (preserving recent ones).
/// 2. If still over budget, remove oldest message pairs from index 1 onward.
///
/// Message[0] (the initial user message) and the last PRESERVE_RECENT_MESSAGES
/// messages are never removed or truncated.
///
/// Returns the number of messages removed in pass 2.
pub(super) fn trim_context_if_needed(messages: &mut Vec<Message>, budget: usize) -> usize {
    // Pass 0: strip base64 image data from all messages except message[0].
    // Message[0] is the current user message — the agentic loop strips its images
    // after iteration 1. But older messages with images (e.g. screenshots from
    // previous turns) burn tokens without value since the LLM already processed them.
    let mut image_bytes_stripped = 0usize;
    for msg in messages.iter_mut().skip(1) {
        if let MessageContent::Blocks(blocks) = &mut msg.content {
            for block in blocks.iter_mut() {
                if let ContentBlock::Image { data, .. } = block {
                    image_bytes_stripped += data.len();
                    *block = ContentBlock::Text {
                        text: "[image from earlier in conversation]".to_string(),
                    };
                }
            }
        }
    }
    if image_bytes_stripped > 0 {
        log!(
            "Context trimming: stripped {}KB of image data from older messages",
            image_bytes_stripped / 1024
        );
    }

    let total: usize = messages.iter().map(estimate_message_chars).sum();
    if total <= budget {
        return 0;
    }

    let len = messages.len();
    let preserve_start = if len > PRESERVE_RECENT_MESSAGES {
        len - PRESERVE_RECENT_MESSAGES
    } else {
        len
    };

    // Pass 1: truncate large values in old messages (skip message[0] and recent)
    for message in &mut messages[1..preserve_start] {
        if let MessageContent::Blocks(blocks) = &mut message.content {
            for block in blocks.iter_mut() {
                match block {
                    ContentBlock::ToolResult {
                        tool_use_id: _,
                        content,
                    } => {
                        if content.len() > TRUNCATION_THRESHOLD {
                            let orig_len = content.len();
                            *content = format!("[content truncated — was {} chars]", orig_len);
                        }
                    }
                    ContentBlock::ToolUse {
                        id: _,
                        name: _,
                        input,
                    } => {
                        truncate_large_json_strings(input);
                    }
                    _ => {}
                }
            }
        }
    }

    let total_after_pass1: usize = messages.iter().map(estimate_message_chars).sum();
    if total_after_pass1 <= budget {
        log!("Context trimming: pass 1 reduced ~{}k -> ~{}k tokens ({} -> {} chars, {} msgs, budget ~{}k tokens)",
            total / 3500, total_after_pass1 / 3500,
            total, total_after_pass1, messages.len(), budget / 3500
        );
        return 0;
    }

    // Pass 2: remove oldest messages (from index 1) until under budget.
    // Tool-use-aware: when removing an assistant message with ToolUse blocks,
    // also remove the following user message (which contains matching ToolResult
    // blocks). Never remove one without the other — orphaned ToolResult blocks
    // cause API validation errors.
    let mut removed = 0;
    let mut current_total = total_after_pass1;
    while current_total > budget && messages.len() > PRESERVE_RECENT_MESSAGES + 1 {
        if messages.len() <= 1 {
            break;
        }

        let has_tool_use = match &messages[1].content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. })),
            _ => false,
        };

        // Remove the message at index 1
        current_total -= estimate_message_chars(&messages[1]);
        messages.remove(1);
        removed += 1;

        // If it had tool_use blocks, the next message (now at index 1) must contain
        // the matching tool_result blocks — remove it too to keep the pair intact.
        if has_tool_use && messages.len() > 1 {
            current_total -= estimate_message_chars(&messages[1]);
            messages.remove(1);
            removed += 1;
        }
    }

    log!("Context trimming: ~{}k -> ~{}k tokens ({} -> {} chars), removed {} messages, {} remaining (budget ~{}k tokens)",
        total / 3500, current_total / 3500,
        total, current_total, removed, messages.len(), budget / 3500
    );
    if current_total > budget {
        log!(
            "Warning: context still over budget after trimming (~{}k tokens, {} chars > {} budget)",
            current_total / 3500,
            current_total,
            budget
        );
    }

    removed
}

/// Validate that every assistant tool_use block has a matching tool_result in the
/// immediately following user message. If any are missing, log a warning and inject
/// stub tool_result blocks so the API doesn't reject the request.
///
/// Returns the number of stub results injected (0 = valid).
pub(super) fn validate_tool_use_pairing(messages: &mut Vec<Message>) -> usize {
    let mut stubs_injected = 0;
    let mut i = 0;
    while i < messages.len() {
        // Find assistant messages with tool_use blocks
        let tool_use_ids: Vec<String> = match &messages[i].content {
            MessageContent::Blocks(blocks) if messages[i].role == "assistant" => blocks
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::ToolUse { id, .. } = b {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            _ => {
                i += 1;
                continue;
            }
        };

        if tool_use_ids.is_empty() {
            i += 1;
            continue;
        }

        // Check that the next message is a user message with matching tool_results
        if i + 1 < messages.len() && messages[i + 1].role == "user" {
            let existing_ids: std::collections::HashSet<String> = match &messages[i + 1].content {
                MessageContent::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                            Some(tool_use_id.clone())
                        } else {
                            None
                        }
                    })
                    .collect(),
                _ => std::collections::HashSet::new(),
            };

            let missing: Vec<&String> = tool_use_ids
                .iter()
                .filter(|id| !existing_ids.contains(*id))
                .collect();

            if !missing.is_empty() {
                log!(
                    "WARNING: {} tool_use IDs missing tool_result in messages[{}]: {:?}",
                    missing.len(),
                    i + 1,
                    missing
                );

                if let MessageContent::Blocks(blocks) = &mut messages[i + 1].content {
                    for id in &missing {
                        blocks.insert(
                            0,
                            ContentBlock::ToolResult {
                                tool_use_id: (*id).clone(),
                                content: "[tool result unavailable]".to_string(),
                            },
                        );
                        stubs_injected += 1;
                    }
                }
            }
        } else {
            // No following user message at all — the assistant message with tool_use
            // is the last message. This shouldn't happen but inject a user message.
            log!("WARNING: assistant message at index {} has tool_use blocks but no following user message", i);
            let result_blocks: Vec<ContentBlock> = tool_use_ids
                .iter()
                .map(|id| {
                    stubs_injected += 1;
                    ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: "[tool result unavailable]".to_string(),
                    }
                })
                .collect();
            messages.insert(
                i + 1,
                Message {
                    role: "user".to_string(),
                    content: MessageContent::Blocks(result_blocks),
                },
            );
        }

        i += 2; // Skip the pair
    }
    stubs_injected
}

/// Recursively truncate large string values in a JSON Value.
pub(super) fn truncate_large_json_strings(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > TRUNCATION_THRESHOLD {
                let orig_len = s.len();
                let preview: String = s.chars().take(200).collect();
                *s = format!(
                    "{}...\n[truncated — full value was {} chars]",
                    preview, orig_len
                );
            }
        }
        serde_json::Value::Object(map) => {
            for (_k, v) in map.iter_mut() {
                truncate_large_json_strings(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                truncate_large_json_strings(v);
            }
        }
        _ => {}
    }
}

/// Truncate a string using head+tail preservation.
/// Keeps 75% from the start and 25% from the end, with an omission marker in the middle.
pub(super) fn truncate_head_tail(content: &str, max_chars: usize) -> String {
    // Fast path: if byte length is under limit, char count must be too
    if content.len() <= max_chars {
        return content.to_string();
    }
    let chars: Vec<char> = content.chars().collect();
    if chars.len() <= max_chars {
        return content.to_string();
    }
    let total = chars.len();
    let head_len = max_chars * 3 / 4;
    let tail_len = max_chars / 4;
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[total - tail_len..].iter().collect();
    let omitted = total - head_len - tail_len;
    format!(
        "{}\n\n... ({} chars omitted) ...\n\n{}",
        head, omitted, tail
    )
}

/// Format a single conversation history message with tiered truncation.
/// Only assistant messages outside the verbatim tail get compacted.
/// Everything else uses the safety-net limit (HISTORY_MSG_TRUNCATE).
pub(super) fn format_history_content(content: &str, role: &str, is_verbatim: bool) -> String {
    if !is_verbatim && role == "assistant" {
        truncate_head_tail(content, HISTORY_ASSISTANT_COMPACT)
    } else {
        truncate_head_tail(content, HISTORY_MSG_TRUNCATE)
    }
}

/// Trim history context from the START (oldest messages) when over budget.
/// This preserves the most recent messages, which are most relevant for follow-ups.
pub(super) fn trim_history_from_oldest(history: &mut String, bytes_to_trim: usize) {
    if bytes_to_trim >= history.len() {
        history.clear();
        return;
    }
    // Use floor_char_boundary to avoid slicing inside a multi-byte UTF-8 character
    let start = history.floor_char_boundary(bytes_to_trim);
    if let Some(pos) = history[start..].find('\n') {
        *history = history[start + pos + 1..].to_string();
    } else {
        history.clear();
    }
}

impl CognosEngine {
    pub(crate) async fn retrieve_context(
        &self,
        query: &str,
        classification: &QueryClassification,
    ) -> (String, usize) {
        const MAX_CONTEXT_CHARS: usize = 50_000;
        use crate::memory::{RETRIEVAL_MIN_IMPORTANCE as MIN_IMPORTANCE, RETRIEVAL_MIN_SIMILARITY as MIN_SIMILARITY};
        const RESULTS_PER_QUERY: usize = 50;
        const MAX_FACTS: usize = 60;
        const KEYWORD_SIMILARITY_PROXY: f64 = 0.6;
        const KEYWORD_BOOST: f64 = 1.2;
        const JACCARD_DEDUP_THRESHOLD: f32 = 0.8;

        let mut context = String::new();
        let mut current_size = 0;

        // Skip memory retrieval entirely if classification says it's not needed
        if !classification.needs_memory {
            log!(@Memory, "Query classified as not needing memory — skipping retrieval");
            return (context, 0);
        }

        let Some(ref index) = self.memory_index else {
            return (context, 0);
        };

        // Use pre-decomposed sub-queries from classification (already done in classify_query)
        let sub_queries = if !classification.sub_queries.is_empty() {
            classification.sub_queries.clone()
        } else {
            vec![query.to_string()]
        };

        let now = chrono::Utc::now();

        // Collect entries with their best relevance score
        let mut all_entries: std::collections::HashMap<Uuid, (MemoryEntry, f64)> =
            std::collections::HashMap::new();

        // Batch embed all sub-queries in a single call
        let sub_query_strs: Vec<&str> = sub_queries.iter().map(|s| s.as_str()).collect();
        let embeddings = match self.embedder.embed_batch(&sub_query_strs).await {
            Ok(e) => e,
            Err(e) => {
                log!(@Memory, "Batch embedding failed: {}", e);
                return (context, 0);
            }
        };

        // Fire all semantic searches concurrently — using search_with_scores for real similarity
        let semantic_futures: Vec<_> = embeddings
            .iter()
            .map(|emb| index.search_with_scores(emb, MIN_IMPORTANCE, RESULTS_PER_QUERY))
            .collect();
        let semantic_results = futures::future::join_all(semantic_futures).await;

        for result in semantic_results {
            match result {
                Ok(scored_entries) => {
                    for (entry, similarity) in scored_entries {
                        if similarity < MIN_SIMILARITY {
                            continue;
                        }
                        let age_days = super::memory::age_in_days(now, entry.src_created_at);
                        let score =
                            super::memory::relevance_score(similarity, entry.importance, age_days);
                        all_entries
                            .entry(entry.id)
                            .and_modify(|(_, existing_score)| {
                                if score > *existing_score {
                                    *existing_score = score;
                                }
                            })
                            .or_insert((entry, score));
                    }
                }
                Err(e) => log!(@Memory, "Semantic search failed: {}", e),
            }
        }

        // Keyword search: boost entries found by keyword match
        let mut keywords: Vec<String> = Vec::new();
        for sub_query in &sub_queries {
            for word in sub_query.split_whitespace() {
                let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric());
                // No uppercase filter: Norwegian common nouns like "pappa"/"øye"
                // are valid entity tags but never capitalize.
                if trimmed.len() >= 3 {
                    keywords.push(trimmed.to_string());
                }
            }
        }
        keywords.sort();
        keywords.dedup();

        let keyword_futures: Vec<_> = keywords
            .iter()
            .map(|kw| index.search_by_keyword(kw, MIN_IMPORTANCE, 20))
            .collect();
        let keyword_results = futures::future::join_all(keyword_futures).await;

        // Track which entries already received a keyword boost (apply at most once)
        let mut keyword_boosted: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        for result in keyword_results {
            match result {
                Ok(results) => {
                    for entry in results.entries {
                        let id = entry.id;
                        let age_days = super::memory::age_in_days(now, entry.src_created_at);
                        let score = super::memory::relevance_score(
                            KEYWORD_SIMILARITY_PROXY,
                            entry.importance,
                            age_days,
                        );
                        all_entries
                            .entry(id)
                            .and_modify(|(_, existing)| {
                                if keyword_boosted.insert(id) {
                                    *existing *= KEYWORD_BOOST;
                                }
                            })
                            .or_insert_with(|| {
                                keyword_boosted.insert(id);
                                (entry, score * KEYWORD_BOOST)
                            });
                    }
                }
                Err(e) => log!(@Memory, "Keyword search failed: {}", e),
            }
        }

        if all_entries.is_empty() {
            return (context, 0);
        }

        // Take top-N by relevance score
        let mut scored: Vec<(MemoryEntry, f64)> = all_entries.into_values().collect();
        scored.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(MAX_FACTS);

        // Deduplicate near-identical facts (>80% word overlap) — keep higher-scored one
        let mut keep = vec![true; scored.len()];
        for i in 0..scored.len() {
            if !keep[i] {
                continue;
            }
            for j in (i + 1)..scored.len() {
                if !keep[j] {
                    continue;
                }
                if jaccard_similarity(&scored[i].0.summary, &scored[j].0.summary)
                    > JACCARD_DEDUP_THRESHOLD
                {
                    keep[j] = false; // i has higher score (list is sorted)
                }
            }
        }
        let mut idx = 0;
        scored.retain(|_| {
            let k = keep[idx];
            idx += 1;
            k
        });

        // Group by topic for presentation
        let mut topic_groups: std::collections::HashMap<String, Vec<(MemoryEntry, f64)>> =
            std::collections::HashMap::new();
        for (entry, score) in scored {
            topic_groups
                .entry(entry.topic.clone())
                .or_default()
                .push((entry, score));
        }

        // Sort chronologically within each topic
        for entries in topic_groups.values_mut() {
            entries.sort_by_key(|(e, _)| e.src_created_at);
        }

        // Prioritize topic groups by average relevance score
        let mut sorted_topics: Vec<(String, Vec<(MemoryEntry, f64)>)> =
            topic_groups.into_iter().collect();
        sorted_topics.sort_by(|(_, a), (_, b)| {
            let avg_a: f64 = a.iter().map(|(_, s)| s).sum::<f64>() / a.len() as f64;
            let avg_b: f64 = b.iter().map(|(_, s)| s).sum::<f64>() / b.len() as f64;
            avg_b
                .partial_cmp(&avg_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Format as structured timeline
        let mut memory_section = String::from("[Long-term Memory]\n\n");
        let mut total_facts = 0;

        for (topic, entries) in &sorted_topics {
            let mut topic_block = format!("## {}\n", topic);
            for (entry, _) in entries {
                let date = entry.src_created_at.format("%Y-%m-%d").to_string();
                let line = format!("- {}: {}\n", date, entry.summary);
                topic_block.push_str(&line);
            }
            topic_block.push('\n');

            if current_size + topic_block.len() > MAX_CONTEXT_CHARS {
                break;
            }

            memory_section.push_str(&topic_block);
            current_size += topic_block.len();
            total_facts += entries.len();
        }

        if total_facts > 0 {
            context.push_str(&memory_section);
        }

        (context, total_facts)
    }
}

#[cfg(test)]
#[path = "context_tests/trim.rs"]
mod context_trim_tests;

#[cfg(test)]
#[path = "context_tests/validate.rs"]
mod validate_tool_use_pairing_tests;

#[cfg(test)]
#[path = "context_tests/memory.rs"]
mod memory_retrieval_tests;

#[cfg(test)]
#[path = "context_tests/format.rs"]
mod history_format_tests;

#[cfg(test)]
#[path = "context_tests/sanitize.rs"]
mod sanitize_file_content_tests;
