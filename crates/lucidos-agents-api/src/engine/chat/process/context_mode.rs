use std::collections::{HashMap, HashSet};

use uuid::Uuid;

#[cfg(test)]
use crate::engine::context_guidance::rendered_context_mode_prompt;
use crate::engine::context_guidance::rendered_context_mode_prompt_at;
pub(crate) use crate::engine::context_guidance::SweepSchedule;
#[cfg(test)]
pub(crate) use crate::engine::context_guidance::RESULT_RECOVERY;
use crate::llm::ToolCapabilities;

pub(crate) const MEMORY_SECTION: &str = "Long-term Memory";

pub(crate) const HISTORY_SECTION: &str = "Conversation History";

pub(crate) const TODO_TOOL_REFUSAL: &str =
    "Refused: there is no todo tool while self-curated context mode is on. Your \
     list lives under a [TODO] heading inside your [WORKING UNDERSTANDING] \
     block. Write the whole list there, in the same reply as your next tool \
     call. It reaches the same prompt bar the user watches.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextMode {
    Off,
    On,
}

impl ContextMode {
    pub(crate) fn from_capabilities(caps: &ToolCapabilities) -> Self {
        if caps.context_mode {
            ContextMode::On
        } else {
            ContextMode::Off
        }
    }

    pub(crate) fn is_on(self) -> bool {
        self == ContextMode::On
    }
}

pub(crate) fn event_address(handle: Uuid) -> String {
    crate::core::store::synthesize_tool_use_id(&handle)
}

pub(crate) const PART_SEPARATOR: &str = "\n\n";

pub(crate) struct CuratedTurn {
    pub(crate) mode: ContextMode,
    pub(crate) schedule: SweepSchedule,
    pub(crate) document: super::working_understanding::WorkingUnderstanding,
    /// INVARIANT: The thread's checklist as this turn found it. It renders inside the
    /// document's block, so the loop needs the snapshot it starts from.
    pub(crate) todo: Vec<crate::engine::thread_events::TodoItem>,
    /// INVARIANT: The *todo notes* beside that list, carried through untouched.
    ///
    /// No schema offers the field any more, so only a legacy row holds one.
    /// Re-emitting the list without it would erase what the agent wrote, which
    /// is the rule the settle path already follows.
    pub(crate) todo_notes: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Sweep {
    pub(crate) removed: Vec<String>,
    pub(crate) index_map: Vec<usize>,
    pub(crate) messages_dropped: usize,
}

impl Sweep {
    pub(crate) fn remap(&self, index: usize) -> usize {
        self.index_map.get(index).copied().unwrap_or(index)
    }
}

pub(crate) fn sweep_expired_pairs(
    messages: &mut Vec<crate::llm::Message>,
    first_seen: &HashMap<String, usize>,
    round: usize,
    schedule: SweepSchedule,
) -> Sweep {
    use crate::llm::{ContentBlock, MessageContent};

    let mut sweep = Sweep::default();
    if !schedule.is_sweep_round(round) {
        return sweep;
    }

    let mut doomed: HashSet<String> = HashSet::new();
    for message in messages.iter() {
        let MessageContent::Blocks(blocks) = &message.content else {
            continue;
        };
        for block in blocks {
            let ContentBlock::ToolResult {
                tool_use_id,
                content,
            } = block
            else {
                continue;
            };
            let Some(address) = crate::engine::context::address_trailer(content) else {
                continue;
            };
            let arrived = first_seen.get(address).copied().unwrap_or(round);
            if schedule.is_past_expiry(arrived, round) {
                doomed.insert(tool_use_id.clone());
                sweep.removed.push(address.to_string());
            }
        }
    }
    if doomed.is_empty() {
        return sweep;
    }

    let mut hollowed: HashSet<usize> = HashSet::new();
    for (index, message) in messages.iter_mut().enumerate() {
        let user = message.role == "user";
        let MessageContent::Blocks(blocks) = &mut message.content else {
            continue;
        };
        let before = blocks.len();
        let orphaned_images = blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
            && blocks.iter().all(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => doomed.contains(tool_use_id),
                _ => true,
            });
        blocks.retain(|block| match block {
            ContentBlock::ToolUse { id, .. } => !doomed.contains(id),
            ContentBlock::ToolResult { tool_use_id, .. } => !doomed.contains(tool_use_id),
            ContentBlock::Image { .. } => !orphaned_images,
            _ => true,
        });
        let text_only = blocks.iter().all(|b| {
            matches!(
                b,
                ContentBlock::Text { .. } | ContentBlock::EngineTail { .. }
            )
        });
        if user && blocks.len() < before && text_only {
            hollowed.insert(index);
        }
    }

    sweep.index_map = vec![0; messages.len()];
    let mut kept: Vec<crate::llm::Message> = Vec::with_capacity(messages.len());
    for (old, message) in std::mem::take(messages).into_iter().enumerate() {
        let empty = matches!(&message.content, MessageContent::Blocks(blocks) if blocks.is_empty());
        if empty || hollowed.contains(&old) {
            sweep.messages_dropped += 1;
            sweep.index_map[old] = kept.len().saturating_sub(1);
            continue;
        }
        sweep.index_map[old] = kept.len();
        kept.push(message);
    }
    *messages = kept;
    sweep
}

pub(crate) fn system_prompt_section(mode: ContextMode, schedule: SweepSchedule) -> String {
    if !mode.is_on() {
        return String::new();
    }
    format!("\n\n{}", rendered_context_mode_prompt_at(schedule))
}

#[cfg(test)]
pub(crate) fn assert_no_short_addresses(surface: &str) {
    let mut rest = surface;
    while let Some(at) = rest.find("evt-") {
        let after = &rest[at + "evt-".len()..];
        if let Some(tail) = after.strip_prefix("<hex>") {
            rest = tail;
            continue;
        }
        let hex: String = after.chars().take_while(char::is_ascii_hexdigit).collect();
        assert_eq!(
            hex.len(),
            32,
            "a shortened evt- address near: {}",
            &after[..after.floor_char_boundary(48)]
        );
        rest = &after[hex.len()..];
    }
}

#[cfg(test)]
#[path = "context_mode_tests.rs"]
mod tests;
