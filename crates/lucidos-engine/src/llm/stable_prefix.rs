//! The *stable prefix* of an Anthropic request, and its fingerprint.
//!
//! The stable prefix is the part of a chat turn's request that the next turn
//! sends again unchanged: the tools, the system block and, on a Tree
//! workspace, the memory view blocks. The per-turn block after it is rebuilt
//! every turn. A cache keep-alive beat can keep only this part warm, so it
//! must send exactly these bytes.
//!
//! The fingerprint hashes that prefix with every `cache_control` marker left
//! out. A beat places its markers differently from a turn, and the question
//! is whether the content matches. The model and the thinking config are part
//! of it: Opus 5.5 renders both into the prefix.

use super::anthropic_wire::{ClaudeOutputConfig, ClaudeRequest, ClaudeThinking};
use serde::ser::{SerializeMap, Serializer};
use serde::Serialize;
use serde_json::Value;

/// The fingerprint of `request`'s stable prefix, for a call to `model`.
/// `None` only when the prefix does not serialize, which the real request
/// would then fail on too.
pub(crate) fn fingerprint(request: &ClaudeRequest, model: &str) -> Option<String> {
    let bytes = serde_json::to_vec(&StablePrefix::of(request, model)).ok()?;
    Some(short_hash(&bytes))
}

/// The first 16 hex characters of the SHA-256: wide enough that two
/// prefixes never collide in practice, short enough to compare by eye.
pub(crate) fn short_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    crate::api::hex::hex_lower(&Sha256::digest(bytes)[..8])
}

#[derive(Serialize)]
struct StablePrefix<'a> {
    model: &'a str,
    thinking: Option<&'a ClaudeThinking>,
    output_config: Option<&'a ClaudeOutputConfig>,
    tools: Vec<UnmarkedTool<'a>>,
    system: Option<Content<'a>>,
    messages: Vec<UnmarkedMessage<'a>>,
}

impl<'a> StablePrefix<'a> {
    fn of(request: &'a ClaudeRequest, model: &'a str) -> Self {
        let tools = request
            .tools
            .iter()
            .flatten()
            .map(|tool| UnmarkedTool {
                name: &tool.name,
                description: &tool.description,
                input_schema: &tool.input_schema,
            })
            .collect();
        let messages = match request.stable_prefix_end {
            None => Vec::new(),
            Some((last, block)) => request.messages[..=last]
                .iter()
                .enumerate()
                .map(|(i, message)| {
                    let content = Content::of(&message.content);
                    UnmarkedMessage {
                        role: &message.role,
                        content: if i == last {
                            content.through(block)
                        } else {
                            content
                        },
                    }
                })
                .collect(),
        };
        Self {
            model,
            thinking: request.thinking.as_ref(),
            output_config: request.output_config.as_ref(),
            tools,
            system: request.system.as_ref().map(Content::of),
            messages,
        }
    }
}

#[derive(Serialize)]
struct UnmarkedTool<'a> {
    name: &'a str,
    description: &'a str,
    input_schema: &'a Value,
}

#[derive(Serialize)]
struct UnmarkedMessage<'a> {
    role: &'a str,
    content: Content<'a>,
}

/// A `system` or message `content` value, serialized as content blocks with
/// no `cache_control` key. Marking a bare string rewrites it into the block
/// form, so a string serializes as the one text block it would become.
enum Content<'a> {
    Text(&'a str),
    Blocks(&'a [Value]),
}

impl<'a> Content<'a> {
    fn of(value: &'a Value) -> Self {
        match value {
            Value::Array(blocks) => Self::Blocks(blocks),
            Value::String(text) => Self::Text(text),
            _ => Self::Blocks(&[]),
        }
    }

    /// The blocks up to and including `block`.
    fn through(self, block: usize) -> Self {
        match self {
            Self::Blocks(blocks) => Self::Blocks(&blocks[..blocks.len().min(block + 1)]),
            text => text,
        }
    }
}

impl Serialize for Content<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Text(text) => {
                serializer.collect_seq([serde_json::json!({"type": "text", "text": text})])
            }
            Self::Blocks(blocks) => serializer.collect_seq(blocks.iter().map(UnmarkedBlock)),
        }
    }
}

struct UnmarkedBlock<'a>(&'a Value);

impl Serialize for UnmarkedBlock<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Value::Object(fields) = self.0 else {
            return self.0.serialize(serializer);
        };
        let mut map = serializer.serialize_map(None)?;
        for (key, value) in fields.iter().filter(|(key, _)| *key != "cache_control") {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

#[cfg(test)]
#[path = "stable_prefix_tests.rs"]
mod tests;
