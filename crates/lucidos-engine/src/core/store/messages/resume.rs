use super::collect_dismissed_event_ids;
use crate::core::EventRow;

/// Number of recent (ToolCalled, ToolResult) pairs reconstructed verbatim as
/// `Message::Blocks(...)` pairs on resume — see
/// [`build_resume_tool_blocks_with_skip_ids`]. `load_knowhow` results are
/// pinned regardless of N.
pub(crate) const RESUME_VERBATIM_TOOL_TAIL: usize = 3;

/// Tool names whose results survive the N-most-recent window. `load_knowhow`
/// returns reference material (a procedure body) that doesn't decay across
/// turns, so multi-step orchestrators keep their recipe across callback
/// resumes.
const PINNED_TOOL_NAMES: &[&str] = &[crate::llm::tool_names::LOAD_KNOWHOW];

/// Stable synthetic `tool_use_id` used by
/// [`build_resume_tool_blocks_with_skip_ids`] to pair the reconstructed
/// `ToolUse` and `ToolResult` blocks. Same event id → same synthetic id
/// across resumes (deterministic, idempotent).
///
/// Renders the full 32-hex-char simple-form UUID (`evt-<32 hex>`). The
/// `dismiss_from_context` tool handler accepts this form (and the bare
/// hyphenated/simple UUID) so the LLM can pass any tool-block id from
/// history directly back to dismiss without truncation guessing.
fn synthesize_tool_use_id(tool_called_event_id: &uuid::Uuid) -> String {
    format!("evt-{}", tool_called_event_id.simple())
}

/// One reconstructed `(ToolCalled, ToolResult)` pair, with the originating
/// `ToolCalled` event id preserved so the caller can deduplicate the
/// stringified `[tools: ...]` summary on the same assistant turn.
///
/// `result.is_none()` marks a synthetic stub for an orphan `ToolCalled` —
/// the originating call has no matching `ToolResult` event in the stream
/// (engine crash mid-tool, projection race, etc.). The builder still emits
/// both messages so every assistant `tool_use` block has a paired user
/// `tool_result` block on the wire — the alternative (silently dropping the
/// pair) yields a Claude API 400.
pub(crate) struct ResumeToolPair {
    pub(crate) tool_called_event_id: uuid::Uuid,
    pub(crate) tool_name: String,
    pub(crate) args: serde_json::Value,
    /// `None` when the matching `ToolResult` event is missing — emit a
    /// synthetic `[tool result unavailable: orphaned]` stub on the wire.
    pub(crate) result: Option<String>,
}

/// Synthetic stub body for orphan `ToolCalled` events (no matching
/// `ToolResult` row). Single source of truth lives in
/// [`crate::llm::validate::ORPHAN_TOOL_RESULT_STUB`] (called by the LLM
/// provider layer's pre-flight pairing validator) — re-exported here so the
/// resume-message assembler emits the same wording regardless of which layer
/// caught the gap.
pub(crate) use crate::llm::validate::ORPHAN_TOOL_RESULT_STUB;

/// Walk events chronologically, pairing every `ToolCalled` with its matching
/// `ToolResult` and surfacing any leftover `ToolCalled` as an orphan
/// (`result == None`). Each call reserves a slot in chronological position
/// so the downstream N-window logic sees the same ordering whether a slot
/// is paired or orphaned. Never silently drops — see
/// [`crate::llm::validate::validate_tool_use_pairing`] for why.
pub(crate) fn collect_tool_pairs_chronological(events: &[EventRow]) -> Vec<ResumeToolPair> {
    let mut slots: Vec<ResumeToolPair> = Vec::new();
    // Pending pairings: (slot_index, tool_name) — name is duplicated here so
    // the rposition match doesn't have to dereference into `slots`.
    let mut pending: Vec<(usize, String)> = Vec::new();

    for event in events {
        match event.event_type.as_str() {
            "ToolCalled" => {
                let tool_name = event
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let args = event
                    .payload
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
                let slot_idx = slots.len();
                slots.push(ResumeToolPair {
                    tool_called_event_id: event.id,
                    tool_name: tool_name.clone(),
                    args,
                    result: None,
                });
                pending.push((slot_idx, tool_name));
            }
            "ToolResult" => {
                let result_name = event
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // Pair with the most recent pending ToolCalled of the same
                // name. If names don't match (legacy events, racing tools),
                // fall back to the most recent pending entry — same forgiving
                // rule build_session_messages applies via `last_mut()`.
                let idx = pending
                    .iter()
                    .rposition(|(_, n)| n == result_name)
                    .or_else(|| pending.len().checked_sub(1));
                if let Some(i) = idx {
                    let (slot_idx, _) = pending.remove(i);
                    let result = event
                        .payload
                        .get("result")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(slot) = slots.get_mut(slot_idx) {
                        slot.result = Some(result);
                    }
                }
            }
            _ => {}
        }
    }

    // `slots` is already in chronological call order. Any slot whose
    // `result` is still `None` is an orphan — surfaced as a synthetic stub
    // by `build_resume_tool_blocks_with_skip_ids`.
    slots
}

/// Partition resume pairs into (pinned, tail) — the shared step both the
/// resume-block builder and the skip-set builder need. Pinned pairs survive
/// regardless of N (see [`PINNED_TOOL_NAMES`]); tail is the last N non-pinned
/// pairs in chronological order.
fn select_pinned_and_tail(
    pairs: &[ResumeToolPair],
    n: usize,
) -> (Vec<&ResumeToolPair>, Vec<&ResumeToolPair>) {
    let mut pinned: Vec<&ResumeToolPair> = Vec::new();
    let mut others: Vec<&ResumeToolPair> = Vec::new();
    for p in pairs {
        if PINNED_TOOL_NAMES.contains(&p.tool_name.as_str()) {
            pinned.push(p);
        } else {
            others.push(p);
        }
    }
    let tail_start = others.len().saturating_sub(n);
    let tail: Vec<&ResumeToolPair> = others[tail_start..].to_vec();
    (pinned, tail)
}

/// Return `(ToolCalled.event_id, tool_name)` for every `ToolCalled` whose
/// matching `ToolResult` is missing in `events`. Single source of truth for
/// the orphan-detection rule used both by the resume builder (which emits
/// synthetic stubs in the LLM messages payload) and the startup recovery
/// sweep (which writes a synthetic `ToolResult` event to settle the orphan).
pub(crate) fn find_orphan_tool_called_ids(events: &[EventRow]) -> Vec<(uuid::Uuid, String)> {
    collect_tool_pairs_chronological(events)
        .into_iter()
        .filter(|p| p.result.is_none())
        .map(|p| (p.tool_called_event_id, p.tool_name))
        .collect()
}

/// Reconstruct verbatim `(ToolUse, ToolResult)` `Message` pairs for the most
/// recent N tool calls — and the set of `ToolCalled` event ids those pairs
/// derive from, so callers can skip the same tools in their stringified
/// history summaries to avoid double-billing.
///
/// `load_knowhow` results are pinned: any pair where `tool_name == "load_knowhow"`
/// is included regardless of where it falls in the N-most-recent ordering.
/// Other tools follow the N window (most recent N kept, older dropped — they
/// keep their `[tools: ...]` summary in stringified history).
///
/// `ContextDismissed` records: any tool pair whose `tool_called_event_id`
/// appears in the dismissed set is dropped from BOTH the rebuilt blocks and
/// the skip set — the latter so the stringified `[tools: ...]` summary also
/// stops rendering for the dismissed tool (the agent asked for the entry to
/// be gone, not just shrunk).
///
/// `loaded_knowhow_ids` is the set of knowhow doc ids currently in the per-
/// thread loaded set. For a `load_knowhow` pair whose `args.id` is in that
/// set, the result body is replaced with a one-line stub pointing the LLM at
/// the `[LOADED KNOWHOW]` block in this turn's user message. The full body
/// already lives there (Phase 3) — sending it again in the resume tool
/// result would double-bill the tokens. Pairs whose id is NOT in the set
/// (e.g. recovery hadn't run yet, or a future `unload_knowhow` removed the
/// entry) keep the verbatim body so the LLM never loses context.
///
/// Output ordering: pinned pairs first (chronological), then tail pairs
/// (chronological). Each pair emits an assistant `ToolUse` block followed by
/// a user `ToolResult` block, matching the wire shape Anthropic and OpenAI
/// providers expect.
///
/// Single-pass: events are walked once and partitioned once — the previous
/// split between `build_resume_tool_blocks` and a separate
/// `build_resume_tool_emitted_event_ids` did both twice per resume.
pub(crate) fn build_resume_tool_blocks_with_skip_ids(
    events: &[EventRow],
    n: usize,
    loaded_knowhow_ids: &std::collections::HashSet<String>,
) -> (Vec<crate::llm::Message>, std::collections::HashSet<String>) {
    let pairs = collect_tool_pairs_chronological(events);
    let dismissed = collect_dismissed_event_ids(events);
    let pairs: Vec<ResumeToolPair> = pairs
        .into_iter()
        .filter(|p| !dismissed.contains(&p.tool_called_event_id.to_string()))
        .collect();
    if pairs.is_empty() {
        return (Vec::new(), std::collections::HashSet::new());
    }
    let (pinned, tail) = select_pinned_and_tail(&pairs, n);

    let total = pinned.len() + tail.len();
    let mut messages: Vec<crate::llm::Message> = Vec::with_capacity(total * 2);
    let mut skip_ids: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(total);
    for p in pinned.into_iter().chain(tail.into_iter()) {
        skip_ids.insert(p.tool_called_event_id.to_string());
        let id = synthesize_tool_use_id(&p.tool_called_event_id);
        messages.push(crate::llm::Message {
            role: "assistant".to_string(),
            content: crate::llm::MessageContent::Blocks(vec![crate::llm::ContentBlock::ToolUse {
                id: id.clone(),
                name: p.tool_name.clone(),
                input: p.args.clone(),
                // Synthesized from the persisted ToolCalled event; the original
                // Gemini 3 thought signature is not stored in the event payload,
                // so we omit it here. The model only enforces signatures within
                // the active turn (newest user message → now), so older history
                // pairs don't need them.
                thought_signature: None,
            }]),
        });
        // Orphans (`result: None`) emit a synthetic stub so every assistant
        // tool_use block has its paired user tool_result on the wire.
        // Anthropic 400s otherwise — see thread b101c3d7 repro.
        let mut content = p
            .result
            .clone()
            .unwrap_or_else(|| ORPHAN_TOOL_RESULT_STUB.to_string());
        // Body-stub swap for already-loaded knowhow: the doc body lives in
        // this turn's `[LOADED KNOWHOW]` user-message section (Phase 3).
        // Order matters — orphan stub takes precedence, since an orphaned
        // `load_knowhow` ToolCalled never produced a body to dedupe in the
        // first place. Only a real, paired result body gets stubbed here.
        if p.tool_name == crate::llm::tool_names::LOAD_KNOWHOW && p.result.is_some() {
            if let Some(doc_id) = p.args.get("id").and_then(|v| v.as_str()) {
                if loaded_knowhow_ids.contains(doc_id) {
                    content = format!(
                        "Loaded. Doc body now lives in the [LOADED KNOWHOW] section of this turn's user message — refer to that section for the body of `{}`.",
                        doc_id
                    );
                }
            }
        }
        messages.push(crate::llm::Message {
            role: "user".to_string(),
            content: crate::llm::MessageContent::Blocks(vec![
                crate::llm::ContentBlock::ToolResult {
                    tool_use_id: id,
                    content,
                },
            ]),
        });
    }
    (messages, skip_ids)
}
