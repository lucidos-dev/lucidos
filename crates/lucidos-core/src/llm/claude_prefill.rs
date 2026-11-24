//! The [`PrefillPair`] of a Messages request, which direct Anthropic, the
//! Claude gateway and Vertex share.
//!
//! A cut keeps the turn's bytes, its cache markers and every generation
//! parameter, and appends one text block after the last marker. Vertex keys
//! its cache pools on `max_tokens`, `stream` and the effort. So only a beat
//! that shares them reads the entries the turn wrote, and it writes none of
//! its own. The beat hangs up at `message_start`
//! ([`super::prefill::send`]), so it never pays for the model's answer.

use super::anthropic_wire::{ClaudeMessage, ClaudeRequest};
use super::prefill::{pair_of, BeatShape, CutItem, PrefillPair, ReplyShape, StableEnd, TurnRoute};
use serde_json::Value;

/// What a beat asks, appended after the last cache marker so it is part of
/// no cache key. The beat drops the reply, so the ask only keeps it short.
pub(crate) const BEAT_PROMPT: &str = "Reply with only the word ok.";

/// The pair a beat on `request` sends, or `None` when it may not beat.
///
/// A request with no cache marker in its stable prefix never beats: a read
/// could land nowhere. So does one whose last message is not the user's.
pub(crate) fn pair(request: &ClaudeRequest, route: TurnRoute<'_>) -> Option<PrefillPair> {
    let value = serde_json::to_value(request).ok()?;
    let end = stable_end(request, &value)?;
    let prefix_end = match &end {
        StableEnd::Part { item, part } => Some((*item, *part)),
        StableEnd::Placeholder { .. } => None,
    };
    let stable_fingerprint =
        super::stable_prefix::fingerprint_through(request, route.wire_model, prefix_end);
    let whole_fingerprint = super::stable_prefix::fingerprint(request, route.wire_model);
    let shape = BeatShape {
        output_cap: None,
        reply: ReplyShape::MessageStart,
        closing_part: Some(serde_json::json!({"type": "text", "text": BEAT_PROMPT})),
    };
    let mut pair = pair_of(
        request,
        |r: &mut ClaudeRequest| Some(&mut r.messages),
        end,
        shape,
        route,
    )?;
    pair.stable.stable_prefix_fingerprint = stable_fingerprint;
    pair.whole.stable_prefix_fingerprint = whole_fingerprint;
    Some(pair)
}

impl CutItem for ClaudeMessage {
    fn role(&self) -> Option<&str> {
        Some(&self.role)
    }

    fn content_mut(&mut self) -> Option<&mut Value> {
        Some(&mut self.content)
    }

    fn placeholder() -> Self {
        Self {
            role: "user".into(),
            content: Value::String(".".into()),
            cache_anchor: None,
        }
    }
}

/// The last block at or before `stable_prefix_end` that carries a cache
/// marker, in a user message. The turn wrote an entry there, so the beat
/// reads it. An unmarked block would make the beat write a new entry.
///
/// An assistant message never ends the cut: a `tool_use` there would lose
/// its `tool_result`, which the API refuses. With no marked message, the cut
/// keeps the system block and the tools, if either carries a marker.
fn stable_end(request: &ClaudeRequest, value: &Value) -> Option<StableEnd> {
    let messages = value.get("messages")?.as_array()?;
    if let Some((last, block)) = request.stable_prefix_end {
        for index in (0..=last).rev() {
            let message = &messages[index];
            let Some(blocks) = message.get("content").and_then(Value::as_array) else {
                continue;
            };
            if message.get("role").and_then(Value::as_str) != Some("user") {
                continue;
            }
            let through = if index == last {
                block + 1
            } else {
                blocks.len()
            };
            let marked = blocks[..through.min(blocks.len())]
                .iter()
                .rposition(|b| b.get("cache_control").is_some());
            if let Some(at) = marked {
                return Some(StableEnd::Part {
                    item: index,
                    part: at,
                });
            }
        }
    }
    let system_marked = value
        .get("system")
        .and_then(Value::as_array)
        .is_some_and(|blocks| blocks.iter().any(|b| b.get("cache_control").is_some()));
    let tools_marked = value
        .get("tools")
        .and_then(Value::as_array)
        .and_then(|tools| tools.last())
        .is_some_and(|tool| tool.get("cache_control").is_some());
    (system_marked || tools_marked).then_some(StableEnd::Placeholder { keep: 0 })
}

#[cfg(test)]
#[path = "claude_prefill_tests.rs"]
mod tests;
