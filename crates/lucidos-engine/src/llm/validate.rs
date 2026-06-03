//! Pre-flight repair of LLM message arrays before they reach a provider.
//!
//! Lives in `llm` (not `engine`) because the logic operates entirely on
//! `llm::provider` types — `Message`, `MessageContent`, `ContentBlock` — and
//! the provider layer is the last place to catch a broken pairing before the
//! Anthropic Messages API 400s. Before this lived in `engine::context`,
//! `llm::vertex` had to reach `up` into `engine::*` to call it, which the
//! `llm_does_not_depend_on_engine` test in this module now forbids.

use crate::llm::{ContentBlock, Message, MessageContent};

/// Synthetic stub body emitted in place of a missing `tool_result`. Single
/// source of truth for the wording — the resume-message assembler in
/// `crate::core::store` re-exports this const so the LLM sees a consistent
/// signal regardless of which layer caught the gap.
pub const ORPHAN_TOOL_RESULT_STUB: &str = "[tool result unavailable: orphaned]";

/// Validate that every assistant tool_use block has a matching tool_result in
/// the immediately following user message. If any are missing, inject stub
/// `tool_result` blocks so Anthropic doesn't 400 with "tool_use ids were
/// found without tool_result blocks immediately after". Promotes the next
/// user message from `Text` to `Blocks` first so the stubs land — Text
/// content can't carry tool_results.
///
/// Returns the number of stub results injected (0 = valid). Called by the
/// agentic loop each iteration AND by the LLM provider layer (defense in
/// depth — covers callers that bypass the loop).
pub fn validate_tool_use_pairing(messages: &mut Vec<Message>) -> usize {
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
                // Text content carries no tool_results — every tool_use_id is
                // missing. Falls through to the injection path below, which
                // promotes the Text content to Blocks so the stubs land.
                _ => std::collections::HashSet::new(),
            };

            let missing: Vec<&String> = tool_use_ids
                .iter()
                .filter(|id| !existing_ids.contains(*id))
                .collect();

            if !missing.is_empty() {
                crate::log!(
                    "[LlmValidate] WARNING: {} tool_use IDs missing tool_result in messages[{}]: {:?}",
                    missing.len(),
                    i + 1,
                    missing
                );

                // Promote Text -> Blocks before injecting stubs: ToolResult
                // blocks can only live inside `Blocks` content. Without this,
                // an orphan tool_use followed by the user's plain-text prompt
                // (the thread-b101c3d7 shape) silently sailed through to
                // Anthropic and produced a 400.
                if let MessageContent::Text(text) = &mut messages[i + 1].content {
                    let promoted = if text.is_empty() {
                        Vec::new()
                    } else {
                        vec![ContentBlock::Text {
                            text: std::mem::take(text),
                        }]
                    };
                    messages[i + 1].content = MessageContent::Blocks(promoted);
                }

                if let MessageContent::Blocks(blocks) = &mut messages[i + 1].content {
                    for id in &missing {
                        blocks.insert(
                            0,
                            ContentBlock::ToolResult {
                                tool_use_id: (*id).clone(),
                                content: ORPHAN_TOOL_RESULT_STUB.to_string(),
                            },
                        );
                        stubs_injected += 1;
                    }
                }
            }
        } else {
            // No following user message at all — the assistant message with tool_use
            // is the last message. This shouldn't happen but inject a user message.
            crate::log!(
                "[LlmValidate] WARNING: assistant message at index {} has tool_use blocks but no following user message",
                i
            );
            let result_blocks: Vec<ContentBlock> = tool_use_ids
                .iter()
                .map(|id| {
                    stubs_injected += 1;
                    ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: ORPHAN_TOOL_RESULT_STUB.to_string(),
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

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
