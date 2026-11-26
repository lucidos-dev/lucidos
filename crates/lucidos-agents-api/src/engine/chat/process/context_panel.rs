use std::collections::{HashMap, HashSet};

use super::context_mode::SweepSchedule;
use super::working_understanding;
use crate::llm::{ContentBlock, Message, MessageContent};

pub(crate) const PANEL_SECTION: &str = "Context Panel";

pub(crate) const PANEL_MARKER: &str = "[CONTEXT PANEL]";

const ROW_MIN_CHARS: usize = 500;

const MAX_ROWS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PanelItem {
    pub(crate) address: String,
    pub(crate) label: String,
    pub(crate) chars: usize,
    pub(crate) original_chars: Option<usize>,
    pub(crate) age_rounds: usize,
    pub(crate) leaves_in: usize,
    pub(crate) stubbed: bool,
}

impl PanelItem {
    fn is_resident(&self) -> bool {
        !self.stubbed
    }
}

pub(crate) fn tool_result_items(
    messages: &[Message],
    first_seen: &HashMap<String, usize>,
    round: usize,
    schedule: SweepSchedule,
) -> Vec<PanelItem> {
    let mut names: HashMap<&str, &str> = HashMap::new();
    let mut items: Vec<PanelItem> = Vec::new();
    for message in messages {
        let MessageContent::Blocks(blocks) = &message.content else {
            continue;
        };
        for block in blocks {
            match block {
                ContentBlock::ToolUse { id, name, .. } => {
                    names.insert(id.as_str(), name.as_str());
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } => {
                    let Some(address) = crate::engine::context::address_trailer(content) else {
                        continue;
                    };
                    let arrived = first_seen.get(address).copied().unwrap_or(round);
                    items.push(PanelItem {
                        label: match names.get(tool_use_id.as_str()) {
                            Some(name) => format!("tool: {name}"),
                            None => "tool result".to_string(),
                        },
                        chars: content.chars().count(),
                        original_chars: crate::engine::context::stub_original_chars(content),
                        age_rounds: round.saturating_sub(arrived),
                        leaves_in: schedule.leaves_in(arrived, round),
                        stubbed: crate::engine::context::is_stub(content),
                        address: address.to_string(),
                    });
                }
                _ => {}
            }
        }
    }
    items
}

pub(crate) fn addresses_in(messages: &[Message]) -> impl Iterator<Item = &str> {
    messages
        .iter()
        .filter_map(|message| match &message.content {
            MessageContent::Blocks(blocks) => Some(blocks),
            MessageContent::Text(_) => None,
        })
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => {
                crate::engine::context::address_trailer(content)
            }
            _ => None,
        })
}

pub(crate) fn note_first_seen(
    first_seen: &mut HashMap<String, usize>,
    messages: &[Message],
    round: usize,
) {
    for address in addresses_in(messages) {
        first_seen.entry(address.to_string()).or_insert(round);
    }
}

pub(crate) fn carries_address(messages: &[Message], address: &str) -> bool {
    addresses_in(messages).any(|carried| carried == address)
}

pub(crate) fn hold_open(first_seen: &mut HashMap<String, usize>, address: &str, round: usize) {
    first_seen.insert(address.to_string(), round);
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FixedRegions {
    pub(crate) system_chars: usize,
    pub(crate) tool_defs_chars: usize,
}

pub(crate) struct PanelView<'a> {
    pub(crate) items: &'a [PanelItem],
    pub(crate) fixed: FixedRegions,
    pub(crate) held_open: &'a HashSet<String>,
    pub(crate) budget_chars: usize,
    pub(crate) round: usize,
    pub(crate) schedule: SweepSchedule,
}

impl PanelView<'_> {
    pub(crate) fn render(&self, other_chars: usize) -> String {
        let probe = self.render_at(0);
        self.render_at(other_chars + probe.chars().count())
    }

    fn render_at(&self, used_chars: usize) -> String {
        let PanelView {
            items,
            fixed,
            held_open,
            budget_chars,
            round,
            schedule,
        } = *self;
        let tokens = crate::engine::context::estimate_tokens_from_chars;
        let share = |chars: usize| {
            if budget_chars == 0 {
                0
            } else {
                (chars.saturating_mul(100) / budget_chars).min(999)
            }
        };

        let resident: Vec<&PanelItem> = items.iter().filter(|item| item.is_resident()).collect();
        let gone: Vec<&PanelItem> = items.iter().filter(|item| !item.is_resident()).collect();
        let resident_chars: usize = resident.iter().map(|item| item.chars).sum();
        let held: Vec<&PanelItem> = resident
            .iter()
            .copied()
            .filter(|item| held_open.contains(&item.address))
            .collect();

        let mut block = String::from(PANEL_MARKER);
        block.push('\n');
        block.push_str(&format!(
            "You are holding {} of {} tokens ({}%).\n",
            field(tokens(used_chars)),
            field(tokens(budget_chars)),
            pad(share(used_chars), 3)
        ));
        block.push_str(&format!(
            "Still resident:  {} tokens across {} addressable items.\n",
            grouped(tokens(resident_chars)),
            resident.len()
        ));
        if !held.is_empty() {
            let held_chars: usize = held.iter().map(|item| item.chars).sum();
            block.push_str(&format!(
                "Held open:       {} items weighing {} tokens, {}% of the room.\n",
                held.len(),
                grouped(tokens(held_chars)),
                share(held_chars)
            ));
        }
        if !gone.is_empty() {
            block.push_str(&format!(
                "Already let go:  {} items now costing {} tokens, in place of {}.\n",
                gone.len(),
                grouped(tokens(gone.iter().map(|item| item.chars).sum())),
                grouped(tokens(
                    gone.iter()
                        .map(|item| item.original_chars.unwrap_or(item.chars))
                        .sum()
                ))
            ));
        }
        block.push_str(&format!(
            "Fixed, and not yours to move: system instructions {} tokens, tool definitions {}.\n",
            grouped(tokens(fixed.system_chars)),
            grouped(tokens(fixed.tool_defs_chars))
        ));
        let until = schedule.rounds_to_next_sweep(round);
        block.push_str(&format!(
        "The next sweep is in {until} round(s). One runs every {} rounds, and takes everything \
         more than {} rounds old.\n",
        schedule.sweep_every_rounds, schedule.expire_after_rounds
    ));
        if until == 1 {
            let leaving: Vec<&str> = items
                .iter()
                .filter(|item| item.leaves_in == 1)
                .map(|item| item.address.as_str())
                .collect();
            if leaving.is_empty() {
                block.push_str("NEXT ROUND IS A SWEEP, and it takes nothing.\n");
            } else {
                block.push_str(&format!(
                "NEXT ROUND IS A SWEEP. These go at the top of it, so consolidate your working \
                 understanding now: {}.\n",
                leaving.join(", ")
            ));
            }
        }
        block.push_str(
        "Doing nothing holds a result until then. Name an address under a [KEEP OPEN] heading to \
         reset its clock. The leaves-in column shows which one needs it.\n",
    );

        let (listed, elided) = select_rows(items);
        if listed.is_empty() {
            block.push_str("\n  Nothing addressable is in the prompt yet.\n");
        } else {
            block.push_str(
            "\n   age  leaves in       tokens  address                                   what\n",
        );
            for item in &listed {
                let was = match item.original_chars {
                    Some(original) => format!("  (was {})", grouped(tokens(original))),
                    None => String::new(),
                };
                block.push_str(&format!(
                    "  {} {} {}  {}  {}{}\n",
                    pad(item.age_rounds.min(9_999), 4),
                    pad(item.leaves_in.min(9_999), 10),
                    field(tokens(item.chars)),
                    item.address,
                    item.label,
                    was
                ));
            }
        }
        if !elided.is_empty() {
            let (small, capped): (Vec<&PanelItem>, Vec<&PanelItem>) =
                elided.iter().partition(|item| item.chars < ROW_MIN_CHARS);
            if !small.is_empty() {
                block.push_str(&format!(
                "\n  {} items under {} chars are not listed, {} tokens together. Holding one open \
                 costs more cache than it saves.\n",
                small.len(),
                ROW_MIN_CHARS,
                grouped(tokens(small.iter().map(|item| item.chars).sum()))
            ));
            }
            if !capped.is_empty() {
                block.push_str(&format!(
                "\n  {} more items did not fit this table, {} tokens together. They are smaller \
                 than the rows above and larger than everything else.\n",
                capped.len(),
                grouped(tokens(capped.iter().map(|item| item.chars).sum()))
            ));
            }
        }
        block.push_str("[END CONTEXT PANEL]");
        block
    }
}

fn select_rows(items: &[PanelItem]) -> (Vec<PanelItem>, Vec<PanelItem>) {
    let mut by_size: Vec<(usize, &PanelItem)> = items.iter().enumerate().collect();
    by_size.sort_by_key(|(index, item)| (std::cmp::Reverse(item.chars), *index));
    let mut keep: Vec<usize> = by_size
        .iter()
        .filter(|(_, item)| item.chars >= ROW_MIN_CHARS)
        .take(MAX_ROWS)
        .map(|(index, _)| *index)
        .collect();
    keep.sort_unstable();
    let listed: Vec<PanelItem> = keep.iter().map(|index| items[*index].clone()).collect();
    let elided: Vec<PanelItem> = items
        .iter()
        .enumerate()
        .filter(|(index, _)| !keep.contains(index))
        .map(|(_, item)| item.clone())
        .collect();
    (listed, elided)
}

fn field(n: usize) -> String {
    format!("{:>11}", grouped(n))
}

fn pad(n: usize, width: usize) -> String {
    format!("{:>width$}", n, width = width)
}

fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

const SUPERSEDED: [(&str, &str); 2] = [
    (
        PANEL_MARKER,
        "[a context panel from an earlier round. The newest one is below.]",
    ),
    (
        working_understanding::OPEN_REPLACE,
        "[an earlier rendering of your working understanding. The current one is below.]",
    ),
];

/// INVARIANT: Collapse the superseded panels and documents in the array.
///
/// Two cases, selected by BLOCK TYPE rather than by marker string. An
/// engine-written block holds nothing but itself, so it is overwritten whole.
/// The model's own reply holds its prose, its next action and its document
/// together, so its span is SPLICED and everything around it stays.
///
/// Typed, because the engine's blocks and the user's words used to be told
/// apart by a displayed prefix. A user message opening with `[CONTEXT PANEL]`
/// was then replaced by the superseded-panel note and lost from the request.
///
/// **The assistant half runs only on a sweep round.** The penultimate cache
/// breakpoint sits on last round's assistant message, and folding it every
/// round destroys that anchor. The engine half runs every round, because its
/// blocks sit in the message the tail marker already rewrites.
///
/// Called immediately before this round appends its own pair, so "every" and
/// "every superseded one" are the same set.
///
/// Returns how many were collapsed.
pub(crate) fn collapse_tail_blocks(messages: &mut [Message], fold_assistant_spans: bool) -> usize {
    let mut collapsed = 0usize;
    for message in messages.iter_mut() {
        let fold_prose = message.role == "assistant" && fold_assistant_spans;
        match &mut message.content {
            MessageContent::Text(text) => {
                if fold_prose {
                    collapsed += usize::from(fold_assistant_text(text));
                }
            }
            MessageContent::Blocks(blocks) => {
                for block in blocks.iter_mut() {
                    match block {
                        ContentBlock::EngineTail { text } => {
                            collapsed += usize::from(fold_engine_block(text));
                        }
                        ContentBlock::Text { text } if fold_prose => {
                            collapsed += usize::from(fold_assistant_text(text));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    collapsed
}

/// INVARIANT: Both blocks the loop appends are ones the fold above knows how to shrink.
///
/// A tail block missing from [`SUPERSEDED`] is never folded, so a 75-round turn
/// carries 75 copies of it. Nothing else ties the appenders to the table.
#[cfg(test)]
#[test]
fn every_block_the_loop_appends_can_be_folded() {
    let folded: Vec<&str> = SUPERSEDED.iter().map(|(marker, _)| *marker).collect();
    for marker in [PANEL_MARKER, working_understanding::OPEN_REPLACE] {
        assert!(
            folded.contains(&marker),
            "{marker} is an engine tail block the collapse never folds"
        );
    }
}

fn fold_engine_block(text: &mut String) -> bool {
    for (marker, note) in SUPERSEDED {
        if text.starts_with(marker) && note.len() < text.len() {
            *text = note.to_string();
            return true;
        }
    }
    false
}

fn fold_assistant_text(text: &mut String) -> bool {
    match working_understanding::fold_spans(text) {
        Some(folded) => {
            *text = folded;
            true
        }
        None => false,
    }
}

pub(crate) fn append_to_tail(messages: &mut [Message], panel: String) -> bool {
    let Some(last) = messages.last_mut() else {
        return false;
    };
    match &mut last.content {
        MessageContent::Blocks(blocks) => blocks.push(ContentBlock::EngineTail { text: panel }),
        MessageContent::Text(text) => {
            last.content = MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: std::mem::take(text),
                },
                ContentBlock::EngineTail { text: panel },
            ]);
        }
    }
    true
}

pub(crate) fn tail_block_section(name: &str, block: &str) -> crate::engine::ContextSection {
    let chars = block.chars().count();
    crate::engine::ContextSection {
        name: name.to_string(),
        content: None,
        budget_delta_chars: chars,
        content_chars: Some(chars),
        role: crate::engine::ContextRole::User,
        group: Some("The request".to_string()),
    }
}

pub(crate) fn panel_section(panel: &str) -> crate::engine::ContextSection {
    tail_block_section(PANEL_SECTION, panel)
}

#[cfg(test)]
#[path = "context_panel_tests.rs"]
mod tests;
