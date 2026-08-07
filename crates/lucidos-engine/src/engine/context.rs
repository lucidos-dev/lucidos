use super::memory::jaccard_similarity;
use super::LucidosEngine;
use crate::core::store::types::Step;
use crate::llm::{ContentBlock, Message, MessageContent};
use crate::memory::{EmbeddingProvider, MemoryEntry, QueryClassification};
use uuid::Uuid;

/// Tokens reserved out of the model's context window when sizing the input
/// char budget — a heuristic margin so a long reply doesn't push the request
/// total past the window. The engine never enforces this on the output side
/// (the model is free to keep generating); the reserve just shrinks how
/// much input we'll pack in. 8k covers a long assistant turn plus tool
/// calls; smaller reserves stop being safe past ~32k responses, which we
/// don't ship today.
pub(super) const RESPONSE_TOKEN_RESERVE: usize = 8_000;

/// Per-request char budget for `(system_prompt + tools + messages)`, derived
/// from the model's resolved context window. JSON-heavy tool calls average
/// ~1.5 chars/token (prose is ~3.5); the 3/2 multiplier picks the
/// conservative end so the char total never under-counts the token total.
/// A 1M-token model gets ~1.49M chars; the default 200k Claude gets ~288k
/// chars (close to the previous hardcoded 300k); GPT-5 at 400k gets ~588k
/// chars. Callers subtract their own prompt overhead (system prompt + tool
/// definitions) before handing the remainder to `trim_context_if_needed`.
///
/// Takes the already-resolved window rather than a model string: the window
/// now comes from the `models` registry when the row declares one, and only
/// falls back to [`context_window_from_prefix`] when it doesn't. Resolution
/// lives in `llm::model_registry::context_window_for`.
///
/// Why this exists: the old `AGENT_CONTEXT_CHAR_BUDGET = 300_000` constant
/// was calibrated for the 200k-token default Claude and ignored the 1M
/// window on the Opus `[1m]` build entirely, so trim pass 2 would silently
/// drop the original user message in long tool loops even though the model
/// could easily have held the whole thread. Per-model derivation lets us
/// actually use the headroom we paid for.
pub(super) fn agent_context_char_budget(context_window: usize) -> usize {
    let usable_tokens = context_window.saturating_sub(RESPONSE_TOKEN_RESERVE);
    // 3/2 = 1.5 chars/token (integer math).
    usable_tokens.saturating_mul(3) / 2
}

/// Exact inverse of [`agent_context_char_budget`]: how many tokens the
/// *budget* believes a char count is worth, at its own conservative 1.5
/// chars/token. Only for talking about the budget, chiefly the trim log
/// lines, which state the content and the budget side by side and would be
/// unreadable in mismatched units. For "how many tokens is this really",
/// use [`estimate_tokens_from_chars`].
pub(super) fn budget_tokens_from_chars(chars: usize) -> usize {
    // Inverse of `chars = tokens * 3 / 2` → `tokens = chars * 2 / 3`.
    chars.saturating_mul(2) / 3
}

/// Best estimate of the real token count behind a char count, at a measured
/// 2.5 chars/token. Feeds `ContextCaptured.estimated_total_tokens` and the
/// `Context: N tokens` thought-stream line, i.e. everything the user reads.
///
/// **Deliberately NOT the budget's ratio**, and the two must not be
/// re-conflated. They answer different questions. The budget's 1.5 exists so
/// the packer can never overflow the window: being conservative there is the
/// whole point, and it stays. A number shown to the user has the opposite
/// duty, and 1.5 made the LLM Context Viewer report a 205k prompt as 361k,
/// contradicting the measured `usage.input_tokens` printed directly above it.
///
/// 2.5 is measured, not guessed. Across 12,069 `ContextCaptured` rows with
/// real usage on `producer = main_llm`, taken after `a997aa403` started
/// counting tool schemas in the total so estimate and actual cover the same
/// content, the implied ratio was p01 2.28, p50 2.60, p99 2.74. Sitting just
/// under the median keeps a small conservative lean, and the other tokenizers
/// measured (Gemini ~3.3, GPT-5.6 ~4.0 once the schemas are corrected for) are
/// more efficient still, so one Claude-calibrated constant errs the safe way
/// for them too. `[Context] calibration` in `agentic_loop/run.rs` keeps
/// logging the comparison, which is what a future per-family split would need.
///
/// History: this was `chars / 4` (the prose ratio) until May 25, when a
/// `workspace-learning` trigger reported "Context: 649 K tokens" to the UI,
/// sent 1.54 M tokens to the API past the 1 M Opus cap, and 400'd. The fix
/// pinned it to the budget's 1.5, which cured the under-count by conflating
/// the two jobs; splitting them is what lets this one be accurate without
/// touching the budget's safety margin.
pub(crate) fn estimate_tokens_from_chars(chars: usize) -> usize {
    // 2/5 = 2.5 chars/token (integer math).
    chars.saturating_mul(2) / 5
}

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

/// Threshold for Pass 1.5 — truncates `ToolResult` blocks in the PRESERVED
/// tail (the last [`PRESERVE_RECENT_MESSAGES`]) when Pass 1 alone can't get
/// under budget. Higher than [`TRUNCATION_THRESHOLD`] because tail
/// preservation exists so the LLM can see what just happened — only the
/// outlier "tool result that dumped 100 KB of events" gets cut, normal
/// small results survive verbatim. The truncated note still reaches the
/// LLM ("[content truncated — was N chars]") so it knows to re-query if
/// the data actually mattered.
pub(super) const TAIL_TRUNCATION_THRESHOLD: usize = 20_000;

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
        // `saturating_sub`, because the replacement can be LONGER than what it
        // replaced: the regex's shortest possible match is 17 bytes
        // (`data:a/b;base64,x`) while `[embedded image, 0KB]` is 21. A file
        // whose only data-URI-shaped substrings are that short therefore grows,
        // and a bare `-` underflows: a panic under `overflow-checks` (every
        // debug/test build) and a wrapped ~18-quintillion-KB figure in the log
        // otherwise. The raw before/after byte counts below stay exact either way.
        let stripped_kb = original_len.saturating_sub(sanitized.len()) / 1024;
        log!(
            "[Context] read_file '{}': stripped {}KB of base64 image data ({} → {} bytes)",
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
        "[Context] read_file '{}': returning bytes {}–{} of {}",
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

/// Flat per-tool overhead for the JSON scaffolding around a tool definition
/// (braces, key names, the `input_schema` wrapper) that isn't in the name /
/// description / parameters strings themselves.
const TOOL_DEF_OVERHEAD_CHARS: usize = 100;

/// Total chars the tool definitions contribute to the request.
///
/// Tool schemas are sent on EVERY request and are a large fixed cost (~70 tools
/// in a chat turn). Both the trim budget and the `ContextCaptured` estimate must
/// account for them, which is why this lives in one place: the budget used to
/// subtract them while the reported `estimated_total_tokens` ignored them
/// entirely, so the LLM Context Viewer under-reported the real prompt and
/// disagreed with the engine's own accounting.
pub(crate) fn tool_definitions_chars(tools: &[crate::llm::provider::ToolDefinition]) -> usize {
    tools
        .iter()
        .map(|t| {
            t.name.len()
                + t.description.len()
                + t.parameters.to_string().len()
                + TOOL_DEF_OVERHEAD_CHARS
        })
        .sum()
}

/// Per-image char cost used by [`estimate_message_chars`] for budget sizing.
///
/// An image's base64 `data.len()` is NOT its cost to the model: providers
/// tokenize an image by its (resized) pixel dimensions, not its byte length.
/// Anthropic bills a resized image at roughly its tile count — on the order of
/// ~1.6k tokens for a full-size photo, regardless of how many megabytes of
/// base64 it serializes to. Counting `data.len()` (2–3 M chars for a phone
/// photo) made one attached image dwarf the entire context budget, which forced
/// trim Pass 2 to evict real conversation/tool context just to "fit" the image
/// — or, with the image pinned, made the loop strip the bytes after the first
/// LLM call so the model went blind to the image mid-turn ("the bot can't see
/// my attached image"). Estimating the real token cost lets the image stay in
/// context for the whole turn without distorting the budget.
pub(super) const IMAGE_BUDGET_TOKEN_ESTIMATE: usize = 1_600;

/// Estimate the total character count of all content in a message.
///
/// The unit is **budget-chars**, not literal characters: an image contributes
/// `IMAGE_BUDGET_TOKEN_ESTIMATE * 3 / 2`, i.e. a token count converted at the
/// budget's own 1.5 chars/token, because its base64 byte length says nothing
/// about what it costs. That is exact for the budget, which is the caller that
/// matters. It does skew [`estimate_tokens_from_chars`], which divides those
/// same budget-chars by the measured 2.5: an image goes in at 1,600 tokens,
/// becomes 2,400 budget-chars, and reads back as 960, so the capture
/// under-reports it by 640 tokens (0.6x). Bounded and well inside the ratio's
/// own spread, so it is accepted rather than plumbed around; revisit if
/// image-heavy turns ever dominate a capture.
pub(super) fn estimate_message_chars(message: &Message) -> usize {
    match &message.content {
        MessageContent::Text(s) => s.len(),
        MessageContent::Blocks(blocks) => {
            blocks
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => text.len(),
                    ContentBlock::ToolUse {
                        id, name, input, ..
                    } => id.len() + name.len() + input.to_string().len(),
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                    } => tool_use_id.len() + content.len(),
                    // Count the real token cost (`* 3 / 2` = the 1.5 chars/token
                    // ratio the budget assumes), not the base64 byte length.
                    ContentBlock::Image { .. } => IMAGE_BUDGET_TOKEN_ESTIMATE * 3 / 2,
                })
                .sum()
        }
    }
}

/// Replace every `ContentBlock::Image` in `blocks` with a text placeholder
/// produced by `placeholder()`. Returns total base64 bytes removed (used for
/// logging by callers).
pub(super) fn replace_image_blocks(
    blocks: &mut [ContentBlock],
    placeholder: impl Fn() -> String,
) -> usize {
    let mut stripped = 0usize;
    for block in blocks.iter_mut() {
        if let ContentBlock::Image { data, .. } = block {
            stripped += data.len();
            *block = ContentBlock::Text {
                text: placeholder(),
            };
        }
    }
    stripped
}

/// What a [`trim_context_if_needed`] call actually did to the messages.
///
/// Both fields mean the LLM saw less than the assembled context. They are kept
/// separate because pass 2's `messages_removed` also shifts the caller's tracked
/// message indices, whereas truncation does not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct TrimOutcome {
    /// Messages evicted by pass 2.
    pub messages_removed: usize,
    /// Content cut in place by pass 1 / 1.5 — `ToolResult` bodies replaced with
    /// a truncation note, plus oversized `ToolUse` argument strings.
    pub blocks_truncated: usize,
}

impl TrimOutcome {
    /// Whether anything was dropped at all. This is what `ContextCaptured.trimmed`
    /// reports — it used to be `messages_removed > 0` alone, which silently
    /// under-reported every turn where passes 1/1.5 gutted tool results but pass
    /// 2 never had to evict a message.
    pub fn any(&self) -> bool {
        self.messages_removed > 0 || self.blocks_truncated > 0
    }
}

/// Trim the agent loop message history to fit within a character budget.
///
/// Three passes:
/// 0. Strip image bytes from every message except the last and `keep_image_idxs`.
/// 1. Truncate large tool results/inputs in old messages.
/// 2. If still over budget, remove oldest message pairs from index 1 onward.
///
/// `messages[0]` and the last `PRESERVE_RECENT_MESSAGES` messages are never
/// removed or truncated in pass 2. If `protected_idx` is `Some(i)`, the message
/// at that index is also pinned: pass 2 stops before removing it, even if that
/// leaves the loop over budget. Callers use this to pin the current turn's
/// user message — once tool iterations push it out of the last
/// `PRESERVE_RECENT_MESSAGES` slots, the recent-tail rule alone no longer
/// covers it and pass 2 would otherwise drop the original request (and, with
/// the image pins covering it, the attached image) from the prompt.
///
/// `keep_image_idxs` lists every message whose image bytes pass 0 must preserve,
/// in addition to the literal last message. Callers pin two kinds of message:
/// the turn's user messages (the initial one plus any mid-turn injection that
/// carried images), and tool results holding an image the model *explicitly
/// asked to see* (`view_image`, `read_file` on an image file). Both must stay
/// visible for the WHOLE turn: once the model makes another tool call the
/// message is no longer last, and stripping its image there blinds the model
/// for the rest of the turn — the "the bot can't see my attached image" bug,
/// and the reason a `view_image` call used to be undone by the very next tool
/// call. Ambient captures (`capture_app`, `browser_screenshot`) are
/// deliberately NOT pinned: they snapshot state that changes under the model,
/// so they must age out after one call.
///
/// The two roles stay distinct: pass 2's eviction floor derives from the
/// *minimum* pinned index only. Pinning a later tool result therefore never
/// weakens eviction — and because every pin sits at or above that floor, no
/// pinned message can be removed, so the caller's index bookkeeping stays exact.
///
/// Returns what the trim actually did — see [`TrimOutcome`].
pub(super) fn trim_context_if_needed(
    messages: &mut Vec<Message>,
    budget: usize,
    protected_idx: Option<usize>,
    keep_image_idxs: &[usize],
) -> TrimOutcome {
    // The current user message is the LAST entry on the first iteration —
    // `chat::process` builds `messages = resume_tool_blocks;
    // messages.push(current_user_message)`. As the tool loop appends pairs it
    // is no longer last, so the pins keep its image too. Every other message's
    // images were already seen by the LLM on prior turns / iterations.
    let mut image_bytes_stripped = 0usize;
    let last_idx = messages.len().saturating_sub(1);
    for (idx, msg) in messages.iter_mut().enumerate() {
        if idx == last_idx || keep_image_idxs.contains(&idx) {
            continue;
        }
        if let MessageContent::Blocks(blocks) = &mut msg.content {
            image_bytes_stripped += replace_image_blocks(blocks, || {
                "[image from earlier in conversation]".to_string()
            });
        }
    }
    if image_bytes_stripped > 0 {
        log!(
            "[Context] Context trimming: stripped {}KB of image data from older messages",
            image_bytes_stripped / 1024
        );
    }

    let total: usize = messages.iter().map(estimate_message_chars).sum();
    if total <= budget {
        return TrimOutcome::default();
    }

    let mut blocks_truncated = 0usize;
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
                            blocks_truncated += 1;
                        }
                    }
                    ContentBlock::ToolUse { input, .. } => {
                        blocks_truncated += truncate_large_json_strings(input);
                    }
                    _ => {}
                }
            }
        }
    }

    let total_after_pass1: usize = messages.iter().map(estimate_message_chars).sum();
    if total_after_pass1 <= budget {
        log!("[Context] Context trimming: pass 1 reduced ~{}k -> ~{}k tokens ({} -> {} chars, {} msgs, budget ~{}k tokens)",
            budget_tokens_from_chars(total) / 1000, budget_tokens_from_chars(total_after_pass1) / 1000,
            total, total_after_pass1, messages.len(), budget_tokens_from_chars(budget) / 1000
        );
        return TrimOutcome {
            messages_removed: 0,
            blocks_truncated,
        };
    }

    // Pass 1.5: if still over budget, truncate large ToolResult blocks in the
    // preserved tail (except the very last message). Tail preservation exists
    // so the LLM can see what just happened — but a tail full of huge
    // `query_events` dumps will blow the budget no matter how aggressively
    // pass 2 removes old pairs. The cut still leaves the "[content truncated
    // — was N chars]" note so the LLM can re-query if the data mattered.
    let mut total_after_truncation = total_after_pass1;
    if len > PRESERVE_RECENT_MESSAGES + 1 {
        let tail_start = len - PRESERVE_RECENT_MESSAGES;
        let tail_end = len - 1; // skip the very last message
        for message in &mut messages[tail_start..tail_end] {
            if let MessageContent::Blocks(blocks) = &mut message.content {
                for block in blocks.iter_mut() {
                    if let ContentBlock::ToolResult {
                        tool_use_id: _,
                        content,
                    } = block
                    {
                        if content.len() > TAIL_TRUNCATION_THRESHOLD {
                            let orig_len = content.len();
                            *content = format!("[content truncated — was {} chars]", orig_len);
                            blocks_truncated += 1;
                        }
                    }
                }
            }
        }
        total_after_truncation = messages.iter().map(estimate_message_chars).sum();
        if total_after_truncation <= budget {
            log!("[Context] Context trimming: pass 1.5 (tail trim) reduced ~{}k -> ~{}k tokens ({} -> {} chars, budget ~{}k tokens)",
                budget_tokens_from_chars(total_after_pass1) / 1000, budget_tokens_from_chars(total_after_truncation) / 1000,
                total_after_pass1, total_after_truncation, budget_tokens_from_chars(budget) / 1000
            );
            return TrimOutcome {
                messages_removed: 0,
                blocks_truncated,
            };
        }
    }

    // Pass 2: remove oldest messages (from index 1) until under budget.
    // Tool-use-aware: when removing an assistant message with ToolUse blocks,
    // also remove the following user message (which contains matching ToolResult
    // blocks). Never remove one without the other — orphaned ToolResult blocks
    // cause API validation errors.
    //
    // `current_total` starts from the post-truncation count (after both Pass 1
    // and Pass 1.5) — using the pre-1.5 number would over-credit the budget
    // and cause Pass 2 to evict more old pairs than necessary.
    let mut removed = 0;
    let mut current_total = total_after_truncation;
    // Track the protected message's position as removals shift it down. Once
    // it reaches index 1 (or its tool_result pair-mate would be removed),
    // stop — losing the pinned request is worse than going over budget.
    //
    // Protect down to the LOWER of `protected_idx` (the latest user input) and
    // the LOWEST image pin. They're equal in the common case, but a mid-turn
    // prompt injection moves `protected_idx` to the new last message while the
    // image stays at a lower index — pass 0 keeps the image bytes, but pass 2
    // must also refuse to remove that whole message, or the image (and the
    // original request) is lost anyway. Stopping at the min keeps both:
    // everything from that index up survives — which is also what makes every
    // *other* pin safe from eviction without protecting them individually.
    let lowest_image_pin = keep_image_idxs.iter().min().copied();
    let mut protected = match (protected_idx, lowest_image_pin) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, None) => a,
        (None, b) => b,
    };
    while current_total > budget && messages.len() > PRESERVE_RECENT_MESSAGES + 1 {
        if messages.len() <= 1 {
            break;
        }

        // Protected message currently at the only removable slot — stop.
        if protected == Some(1) {
            break;
        }

        let has_tool_use = match &messages[1].content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. })),
            _ => false,
        };

        // Pair removal would also drop messages[2]; if that's the protected
        // message we'd corrupt it (lose the request line) rather than orphan
        // a tool_use. Stop instead.
        if has_tool_use && protected == Some(2) {
            break;
        }

        // Remove the message at index 1
        current_total -= estimate_message_chars(&messages[1]);
        messages.remove(1);
        removed += 1;
        protected = protected.map(|p| p.saturating_sub(1));

        // If it had tool_use blocks, the next message (now at index 1) must contain
        // the matching tool_result blocks — remove it too to keep the pair intact.
        if has_tool_use && messages.len() > 1 {
            current_total -= estimate_message_chars(&messages[1]);
            messages.remove(1);
            removed += 1;
            protected = protected.map(|p| p.saturating_sub(1));
        }
    }

    log!("[Context] Context trimming: ~{}k -> ~{}k tokens ({} -> {} chars), removed {} messages, {} remaining (budget ~{}k tokens)",
        budget_tokens_from_chars(total) / 1000, budget_tokens_from_chars(current_total) / 1000,
        total, current_total, removed, messages.len(), budget_tokens_from_chars(budget) / 1000
    );
    if current_total > budget {
        log!(
            "[Context] Warning: context still over budget after trimming (~{}k tokens, {} chars > {} budget)",
            budget_tokens_from_chars(current_total) / 1000,
            current_total,
            budget
        );
    }

    TrimOutcome {
        messages_removed: removed,
        blocks_truncated,
    }
}

/// Recursively truncate large string values in a JSON Value. Returns how many
/// strings were cut, so the caller can report the content loss — a truncated
/// tool-call argument is just as invisible to the LLM as a dropped message.
pub(super) fn truncate_large_json_strings(value: &mut serde_json::Value) -> usize {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > TRUNCATION_THRESHOLD {
                let orig_len = s.len();
                let preview: String = s.chars().take(200).collect();
                *s = format!(
                    "{}...\n[truncated — full value was {} chars]",
                    preview, orig_len
                );
                return 1;
            }
            0
        }
        serde_json::Value::Object(map) => map
            .iter_mut()
            .map(|(_k, v)| truncate_large_json_strings(v))
            .sum(),
        serde_json::Value::Array(arr) => arr.iter_mut().map(truncate_large_json_strings).sum(),
        _ => 0,
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
///
/// Before truncation, any verbatim occurrence of a loaded knowhow body is
/// replaced with a one-line pointer back to the `[LOADED KNOWHOW]` section
/// in this turn's prompt — the body lives there now and the LLM doesn't
/// need to see it again inside `[CONVERSATION HISTORY]`. Match is by exact
/// body substring (the loaded body is the same `[SYSTEM-KNOWHOW: <name>]`
/// block produced by `core::knowhow::load_one_knowhow_section`); organic
/// discussion of the marker that doesn't reproduce a real body survives
/// unchanged.
pub(super) fn format_history_content(
    content: &str,
    role: &str,
    is_verbatim: bool,
    loaded_knowhow_bodies: &[&str],
) -> String {
    let stripped = strip_loaded_knowhow_bodies(content, loaded_knowhow_bodies);
    if !is_verbatim && role == "assistant" {
        truncate_head_tail(&stripped, HISTORY_ASSISTANT_COMPACT)
    } else {
        truncate_head_tail(&stripped, HISTORY_MSG_TRUNCATE)
    }
}

const LOADED_KNOWHOW_POINTER: &str = "(body in [LOADED KNOWHOW] section above)";

/// Replace each verbatim occurrence of a loaded knowhow body with a one-line
/// pointer to the `[LOADED KNOWHOW]` section. Bodies are matched as exact
/// substrings — the loaded body is the formatted block returned by
/// `load_one_knowhow_section`, which is what the LLM saw on the original
/// `load_knowhow` tool result. Bodies for unloaded docs (or stray markers
/// the LLM typed in discussion) pass through unchanged.
fn strip_loaded_knowhow_bodies(content: &str, loaded_bodies: &[&str]) -> String {
    if loaded_bodies.is_empty() {
        return content.to_string();
    }
    let mut out = content.to_string();
    for body in loaded_bodies {
        if !body.is_empty() && out.contains(body) {
            out = out.replace(body, LOADED_KNOWHOW_POINTER);
        }
    }
    out
}

/// Bounds runaway tool-loop turns; ~6 typical tool calls fit in well under this.
const HISTORY_STEPS_MAX_BYTES: usize = 2_000;

/// Compact summary of an assistant turn's tool calls for the LLM's history
/// string. Returns `None` when there's nothing tool-shaped to report
/// (synthetic Thinking/MemorySearched steps lack `tool_name` and are
/// skipped). Output is capped at [`HISTORY_STEPS_MAX_BYTES`].
///
/// `skip_tool_event_ids` is the set of `ToolCalled` event ids that are
/// already represented as full `Message::Blocks(...)` pairs prepended to the
/// LLM messages vec by `build_resume_tool_blocks_with_skip_ids`. Their
/// summary lines are suppressed here so the LLM doesn't see the same tool
/// call twice (once verbatim, once as a `[tools: ...]` summary).
pub(crate) fn format_history_steps(
    steps: &[Step],
    skip_tool_event_ids: &std::collections::HashSet<String>,
) -> Option<String> {
    let descs: Vec<String> = steps
        .iter()
        .filter(|s| s.tool_name.is_some())
        .filter(|s| {
            s.tool_called_event_id
                .as_ref()
                .map(|id| !skip_tool_event_ids.contains(id))
                .unwrap_or(true)
        })
        .map(|s| {
            let mark = if s.success { "ok" } else { "FAIL" };
            // describe_tool's trailing "..." is for live progress, not history.
            let desc = s.description.trim_end_matches("...");
            format!("{} [{}]", desc, mark)
        })
        .collect();
    if descs.is_empty() {
        return None;
    }
    let joined = descs.join(" | ");
    let capped = truncate_head_tail(&joined, HISTORY_STEPS_MAX_BYTES);
    Some(format!(" [tools: {}]", capped))
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

/// Render one memory entry as its `[Long-term Memory]` bullet. The trailing
/// `[id: <uuid>]` is what lets the agent target the exact entry for deletion or
/// correction via the `correct_memory_by_id` tool; the fuzzy `correct_memory`
/// path needs no id. Keep this in sync with the id form the tool parses.
fn format_memory_bullet(entry: &MemoryEntry) -> String {
    let date = entry.src_created_at.format("%Y-%m-%d");
    format!("- {}: {} [id: {}]\n", date, entry.summary, entry.id)
}

impl LucidosEngine {
    pub(crate) async fn retrieve_context(
        &self,
        query: &str,
        classification: &QueryClassification,
    ) -> (String, usize) {
        const MAX_CONTEXT_CHARS: usize = 50_000;
        use crate::memory::{
            RETRIEVAL_MIN_IMPORTANCE as MIN_IMPORTANCE, RETRIEVAL_MIN_SIMILARITY as MIN_SIMILARITY,
        };
        const RESULTS_PER_QUERY: usize = 50;
        const MAX_FACTS: usize = 25;
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
                // No uppercase filter: Norwegian common nouns like "bil"/"hund"
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
                topic_block.push_str(&format_memory_bullet(entry));
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
#[path = "context_tests/memory.rs"]
mod memory_retrieval_tests;

#[cfg(test)]
#[path = "context_tests/format.rs"]
mod history_format_tests;

#[cfg(test)]
#[path = "context_tests/sanitize.rs"]
mod sanitize_file_content_tests;

#[cfg(test)]
mod tool_definition_sizing_tests {
    use super::{estimate_tokens_from_chars, tool_definitions_chars, TOOL_DEF_OVERHEAD_CHARS};
    use crate::llm::provider::ToolDefinition;

    fn tool(name: &str, description: &str, params: serde_json::Value) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters: params,
        }
    }

    #[test]
    fn sums_name_description_schema_and_per_tool_overhead() {
        let t = tool("read_file", "Read a file", serde_json::json!({"a": 1}));
        let schema_len = serde_json::json!({"a": 1}).to_string().len();
        assert_eq!(
            tool_definitions_chars(std::slice::from_ref(&t)),
            "read_file".len() + "Read a file".len() + schema_len + TOOL_DEF_OVERHEAD_CHARS
        );
        // And it scales with the list — the real chat turn ships ~70 of these.
        assert_eq!(
            tool_definitions_chars(&[t.clone(), t.clone()]),
            tool_definitions_chars(std::slice::from_ref(&t)) * 2
        );
    }

    #[test]
    fn empty_tool_list_costs_nothing() {
        assert_eq!(tool_definitions_chars(&[]), 0);
    }

    /// `ContextCaptured.estimated_total_tokens` used to be computed from the
    /// system prompt + messages only, while the trim budget subtracted the tool
    /// schemas as well — so the number shown in the LLM Context Viewer was
    /// smaller than the prompt the engine actually sent and budgeted for. This
    /// pins the direction of the fix: including the schemas can only raise the
    /// reported total.
    #[test]
    fn including_tool_definitions_raises_the_reported_total() {
        let tools: Vec<ToolDefinition> = (0..70)
            .map(|i| {
                tool(
                    &format!("tool_{i}"),
                    "does a thing",
                    serde_json::json!({"type": "object", "properties": {}}),
                )
            })
            .collect();
        let system_chars = 77_636; // measured system prompt from the kimi-k3 thread
        let context_chars = 120_000;
        let without = estimate_tokens_from_chars(system_chars + context_chars);
        let with = estimate_tokens_from_chars(
            system_chars + tool_definitions_chars(&tools) + context_chars,
        );
        assert!(
            with > without,
            "tool schemas must count toward the reported total ({with} vs {without})"
        );
    }
}

#[cfg(test)]
mod chars_per_token_ratio_tests {
    use super::{agent_context_char_budget, budget_tokens_from_chars, estimate_tokens_from_chars};

    /// Pin the measured display ratio at 2.5 chars/token. The number the user
    /// reads is the one this produces, and it was 1.5 (the budget's safety
    /// ratio) until the LLM Context Viewer was caught reporting a 205k prompt
    /// as 361k, contradicting the measured `usage.input_tokens` above it.
    #[test]
    fn display_estimate_uses_the_measured_ratio() {
        assert_eq!(estimate_tokens_from_chars(1_000), 400);
        assert_eq!(estimate_tokens_from_chars(0), 0);
    }

    /// And pin the budget ratio at 1.5, separately, so a future edit has to
    /// touch two assertions to re-conflate them.
    #[test]
    fn budget_conversion_keeps_the_conservative_ratio() {
        assert_eq!(budget_tokens_from_chars(1_000), 666);
        assert_eq!(budget_tokens_from_chars(0), 0);
    }

    /// The two answer different questions and must stay apart. Conflating them
    /// is the actual regression this split prevents, in either direction: give
    /// the budget the display's ratio and the packer overflows the window (the
    /// May 25 400); give the display the budget's ratio and every context
    /// readout runs ~1.7x high.
    #[test]
    fn the_two_ratios_are_deliberately_different() {
        let chars = 540_100; // the reported capture: 540.1K chars, 205k real tokens
        let displayed = estimate_tokens_from_chars(chars);
        let budgeted = budget_tokens_from_chars(chars);
        assert!(
            budgeted > displayed,
            "the budget must stay the conservative one ({budgeted} vs {displayed})"
        );
        // Within 10% of the 205k the provider actually charged for, where the
        // budget ratio was out by 73%. This capture's own implied ratio was
        // 2.63, a shade above the 2.60 median, so 2.5 reads it ~5% high: the
        // deliberate conservative lean, not slack in the assertion.
        assert!(
            (185_000..=226_000).contains(&displayed),
            "displayed estimate {displayed} should be within 10% of the measured 205k"
        );
        assert_eq!(budgeted, 360_066, "the budget ratio's 73% overcount");
    }

    /// The budget itself is untouched by the split: it is still expressed in
    /// chars at 1.5 chars/token, and the display ratio must not leak into it.
    #[test]
    fn the_split_did_not_move_the_budget() {
        assert_eq!(agent_context_char_budget(200_000), 288_000);
        assert_eq!(
            budget_tokens_from_chars(agent_context_char_budget(200_000)),
            192_000
        );
    }
}

#[cfg(test)]
mod context_window_tests {
    use super::agent_context_char_budget;
    use crate::llm::model_registry::context_window_from_prefix;

    /// The budget for the 1M-token Opus build must be substantially larger
    /// than the budget for the 200k default — that's the entire point of
    /// per-model derivation. The hardcoded 300k constant we replaced was
    /// capping Opus at ~5× too small, so the new value must be at least
    /// 4× the old default to be a real fix.
    #[test]
    fn opus_1m_budget_is_much_larger_than_default_claude() {
        let opus_1m = agent_context_char_budget(context_window_from_prefix("claude-opus-4-7[1m]"));
        let default_claude =
            agent_context_char_budget(context_window_from_prefix("claude-opus-4-7"));
        assert!(
            opus_1m >= default_claude * 4,
            "1M Opus budget ({}) must be ≥ 4× default Claude budget ({})",
            opus_1m,
            default_claude
        );
        // And large enough to hold the 552 KB iPhone screenshot from the
        // regression thread without trim pass 2 evicting anything.
        assert!(opus_1m > 1_000_000, "Opus 1M budget too small: {}", opus_1m);
    }

    /// A registry-declared window drives the budget directly — the whole point
    /// of the `context_window` column. kimi-k3's declared 1,048,576 must yield
    /// a ~1.56M-char budget, not the 288k its id shape would have produced.
    #[test]
    fn declared_window_budget_dwarfs_the_prefix_fallback() {
        let declared = agent_context_char_budget(1_048_576);
        let fallback = agent_context_char_budget(context_window_from_prefix("moonshotai/kimi-k3"));
        assert_eq!(declared, (1_048_576 - 8_000) * 3 / 2);
        assert_eq!(fallback, 288_000);
        assert!(
            declared >= fallback * 5,
            "declared 1M budget ({declared}) must be ≥ 5× the 200k fallback ({fallback})"
        );
    }

    /// Pin the exact formula output for the default 200k Claude. The fuzzy
    /// "close to 300k" range invited silent drift; the exact value
    /// `(200_000 - 8_000) * 3 / 2 = 288_000` documents the math and trips
    /// loudly if the multiplier or `RESPONSE_TOKEN_RESERVE` change without
    /// a deliberate update here.
    #[test]
    fn default_claude_budget_matches_formula() {
        assert_eq!(agent_context_char_budget(200_000), 288_000);
    }

    /// And the same pin for the 1M Opus build — the regression scenario.
    #[test]
    fn opus_1m_budget_matches_formula() {
        assert_eq!(agent_context_char_budget(1_000_000), 1_488_000);
    }
}
