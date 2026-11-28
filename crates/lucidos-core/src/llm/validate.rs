use crate::llm::provider::ToolDefinition;
use crate::llm::{ContentBlock, Message, MessageContent};

pub const ORPHAN_TOOL_RESULT_STUB: &str = "[tool result unavailable: orphaned]";

pub fn validate_tool_use_pairing(messages: &mut Vec<Message>) -> usize {
    let mut stubs_injected = 0;
    let mut i = 0;
    while i < messages.len() {
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
                crate::log_legacy!(
                    "[LlmValidate] WARNING: {} tool_use IDs missing tool_result in messages[{}]: {:?}",
                    missing.len(),
                    i + 1,
                    missing
                );

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
            crate::log_legacy!(
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

        i += 2;
    }
    stubs_injected
}

pub const MAX_TOOL_NAME_LEN: usize = 128;

pub fn is_wire_safe_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TOOL_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// INVARIANT: Rewrite `name` to satisfy the pattern: one `_` per disallowed character,
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
