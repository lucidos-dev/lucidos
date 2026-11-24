//! The self-curated context mode: what a turn holds, what the model can let go
//! of, and where the prompt splits so letting go is cheap.
//!
//! Everything the mode changes about a chat or trigger turn is decided here.
//! `run.rs` reads the answers and applies them; the agentic loop applies the
//! ones that land mid-turn. Nothing else branches on the flag, so "what does
//! the mode do" is answerable by reading this file.
//!
//! **Off is the control arm, and it must stay byte-identical.** Every function
//! here answers with nothing when the mode is off. The off path therefore adds
//! nothing to the prompt and takes nothing away. ADR 0087's eval compares the
//! two arms directly, and a drift in the off path is a drift in its baseline.
//!
//! One thing outside this file does move the off arm, deliberately: the
//! `[TODO LIST]` block a turn opens with, in `tools::todo::turn_start_block`.
//! It is suppressed under the mode, which already renders the checklist at the
//! tail of every round. Both arms now show the agent its own list, where before
//! only the mode did.
//!
//! The mode is one rule and two instruments. A tool result stays until a sweep
//! takes it, whole, with the call that made it. The context panel shows what
//! that leaves, and the working understanding is where the model writes what a
//! result told it. See ADR 0109 and
//! `docs/plans/2026-08-24-the-working-understanding-and-the-ten-round-window.md`.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

#[cfg(test)]
use crate::engine::context_guidance::rendered_context_mode_prompt;
use crate::engine::context_guidance::rendered_context_mode_prompt_at;
pub(crate) use crate::engine::context_guidance::SweepSchedule;
#[cfg(test)]
pub(crate) use crate::engine::context_guidance::RESULT_RECOVERY;
use crate::llm::ToolCapabilities;

/// The memory-recall section's name, as the prompt and the capture both spell
/// it. Shared with `context_build`, and read by the eval's manipulation check.
pub(crate) const MEMORY_SECTION: &str = "Long-term Memory";

pub(crate) const HISTORY_SECTION: &str = "Conversation History";

/// What the loop answers a `todo_write` call with while the mode is on.
///
/// The tool is not in the mode's array, so reaching it means a cached prompt or
/// a hallucination. It names the heading, because a refusal that only says no
/// leaves the model with a list and nowhere to put it.
pub(crate) const TODO_TOOL_REFUSAL: &str =
    "Refused: there is no todo tool while self-curated context mode is on. Your \
     list lives under a [TODO] heading inside your [WORKING UNDERSTANDING] \
     block. Write the whole list there, in the same reply as your next tool \
     call. It reaches the same prompt bar the user watches.";

/// Whether this turn assembles its prompt the mode's way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextMode {
    /// Today's behaviour, and the eval's control arm.
    Off,
    /// Self-curated context mode.
    On,
}

impl ContextMode {
    /// Read off the turn's capability snapshot, which is where the preference
    /// was resolved. One read per turn reaches the tools array, the prompt and
    /// the payload, so none of the three can describe a different mode.
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

/// The `evt-<32 hex>` address every tool result states and every keep takes.
pub(crate) fn event_address(handle: Uuid) -> String {
    crate::core::store::synthesize_tool_use_id(&handle)
}

/// What the user message's parts are joined with.
///
/// Shared with `run.rs` rather than written twice, so the two arms join their
/// sections the same way.
pub(crate) const PART_SEPARATOR: &str = "\n\n";

/// What the agentic loop needs from the turn's setup, and nothing else.
pub(crate) struct CuratedTurn {
    pub(crate) mode: ContextMode,
    /// When results leave, as this turn resolved it.
    pub(crate) schedule: SweepSchedule,
    /// The thread's working understanding as this turn found it.
    ///
    /// The seed only. The loop replaces it as each span is parsed, and renders
    /// whatever it holds at the tail of every round.
    pub(crate) document: super::working_understanding::WorkingUnderstanding,
    /// The thread's checklist as this turn found it. It renders inside the
    /// document's block, so the loop needs the snapshot it starts from.
    pub(crate) todo: Vec<crate::engine::thread_events::TodoItem>,
    /// The *todo notes* beside that list, carried through untouched.
    ///
    /// No schema offers the field any more, so only a legacy row holds one.
    /// Re-emitting the list without it would erase what the agent wrote, which
    /// is the rule the settle path already follows.
    pub(crate) todo_notes: Option<String>,
}

/// What one sweep did to the message array.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Sweep {
    /// The addresses whose pair left, in array order.
    pub(crate) removed: Vec<String>,
    /// Where each old message index landed. A message the sweep emptied maps to
    /// the surviving message before it.
    pub(crate) index_map: Vec<usize>,
    /// How many messages the sweep emptied and dropped.
    pub(crate) messages_dropped: usize,
}

impl Sweep {
    /// Move a message index the caller pinned onto its new position.
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

    // An empty map is the identity, which is what [`Sweep::remap`] answers
    // when it finds no entry. Filling one in on a round that moves nothing
    // would allocate the length of the whole array to say so.
    let mut sweep = Sweep::default();
    if !schedule.is_sweep_round(round) {
        return sweep;
    }

    // Which calls go, found from their results: the result is the block
    // carrying the address, and its `tool_use_id` is what pairs the two.
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
            // An address the panel never saw arrived this round, so it is age
            // zero and the model is reading it now.
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
        // A tail block counts as text. An older one is already folded to a
        // one-line note. The current panel rides on the newest message,
        // whose results are age zero and so never doomed.
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

    // Two shapes go, and both are what the pair left behind. An assistant
    // message of nothing but `tool_use` blocks is now empty, which the provider
    // rejects and `validate_tool_use_pairing` does not see. A results message is
    // now the round's trailing instruction and nothing else, so it says "Results
    // above" above nothing. Every pinned index moves with them.
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

/// Fail if a surface shows an `evt-` run that is neither the placeholder nor
/// the whole 32 hex digits.
///
/// Our defect, not the model's. The old guidance opened its worked example with
/// a shortened address, and Opus copied that ellipsis into 28 of the 31
/// addresses it wrote back. Every one of them resolved to nothing.
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
