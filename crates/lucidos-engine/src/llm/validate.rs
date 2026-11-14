//! Pre-flight repair of an LLM request before it reaches a provider. Two
//! axes: the message array (orphan `tool_use` blocks) and the tool array
//! (names the API will not accept).
//!
//! Lives in `llm` (not `engine`) because the logic operates entirely on
//! `llm::provider` types, and the provider layer is the last place to catch a
//! broken request before the Anthropic Messages API 400s. Before this lived in
//! `engine::context`, `llm::vertex` had to reach `up` into `engine::*` to call
//! it, which the `llm_does_not_depend_on_engine` test in this module now
//! forbids.

use crate::llm::provider::ToolDefinition;
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

// ---------------------------------------------------------------------------
// Wire-safe tool names
// ---------------------------------------------------------------------------

/// Ceiling from the Anthropic tool-name pattern `^[a-zA-Z0-9_-]{1,128}$`.
pub const MAX_TOOL_NAME_LEN: usize = 128;

/// Whether `name` satisfies the pattern every tool definition must match.
///
/// The alphabet is narrower than MCP's, which puts no restriction on a tool
/// name at all. Reconciling the two is why [`wire_safe_tool_name`] exists.
pub fn is_wire_safe_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TOOL_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Rewrite `name` to satisfy the pattern: one `_` per disallowed character,
/// then truncate to the ceiling. Total, and the output always passes
/// [`is_wire_safe_tool_name`].
///
/// Lossy on purpose, so callers must not treat the result as reversible.
/// `a.b` and `a_b` both land on `a_b`, which is why `mcp::wire_tool_names`
/// disambiguates across a server's whole tool list rather than per name.
pub fn wire_safe_tool_name(name: &str) -> String {
    let mut safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_TOOL_NAME_LEN)
        .collect();
    if safe.is_empty() {
        safe.push('_');
    }
    safe
}

/// Drop every tool definition the provider would reject, returning their
/// names. Last line of defense: the Messages API rejects the WHOLE request
/// over one bad name, so sending it costs the turn rather than the tool.
///
/// A hit means an upstream layer built a name it should not have. The caller
/// logs it; nothing here is a normal occurrence.
pub fn drop_unsafe_tool_names(tools: &mut Vec<ToolDefinition>) -> Vec<String> {
    let mut dropped = Vec::new();
    tools.retain(|t| {
        if is_wire_safe_tool_name(&t.name) {
            true
        } else {
            dropped.push(t.name.clone());
            false
        }
    });
    dropped
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
