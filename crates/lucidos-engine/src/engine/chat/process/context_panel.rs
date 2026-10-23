//! The context panel: how full the prompt is, and what is in it.
//!
//! The mode's ledger named an address, a label and a recovery call. It never
//! named a size, an age, a total or a percent of budget. So the model was asked
//! to curate blind, and it declined on all 206 rounds of the first run.
//!
//! The panel is appended to the NEWEST message and never edited into an older
//! one. Lucidos spends an Anthropic cache breakpoint on the last message, so a
//! byte changed anywhere earlier re-writes the whole suffix at 1.25x. An
//! appended block costs only what it adds. The tail is also where attention is
//! strongest, which is why Manus recites its todo list there.
//!
//! A superseded panel IS replaced in place, by [`collapse_tail_blocks`], and so
//! is a superseded working understanding. That rewrites everything after it,
//! which is affordable: the block is one round old and sits at the tail.
//! Leaving them would make the two the fastest growing thing in the prompt, and
//! the only one the model cannot address.

use std::collections::{HashMap, HashSet};

use super::context_mode::SweepSchedule;
use super::working_understanding;
use crate::llm::{ContentBlock, Message, MessageContent};

/// The panel's own section name, in the prompt and in the capture.
///
/// The eval's manipulation check reads this row: present on every round of a
/// lean thread, absent from every round of a control thread. The name IS that
/// contract, so a rename here silently passes an arm that did nothing.
pub(crate) const PANEL_SECTION: &str = "Context Panel";

/// What the panel's block opens with.
///
/// A heading for the model to read, and nothing else keys off it.
/// [`ContentBlock::EngineTail`] carries which blocks the engine wrote. So
/// neither the fold below nor the wire layer's cache anchor guesses from text.
pub(crate) const PANEL_MARKER: &str = "[CONTEXT PANEL]";

/// Rows below this size are counted rather than listed.
///
/// Not a silent cap: the panel states how many were elided and what they weigh.
/// Nothing here gates an eviction. Expiry is uniform, so a small item leaves on
/// the same schedule whether or not it has a row. What a row buys is a keep,
/// and holding 500 chars open is never worth the line.
const ROW_MIN_CHARS: usize = 500;

/// The most rows the panel lists, largest first.
const MAX_ROWS: usize = 20;

/// One addressable thing in the request, as the panel names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PanelItem {
    /// The `evt-<32 hex>` form, which is what a keep takes.
    pub(crate) address: String,
    pub(crate) label: String,
    /// What it costs the request right now. For a stub that is the stub.
    pub(crate) chars: usize,
    /// What it cost before a budget pass cut it, when one did.
    ///
    /// Shown beside `chars` so the model can price a re-fetch. Without it a
    /// stubbed row reads as 12 tokens and says nothing about the 16,000 the
    /// address would bring back.
    pub(crate) original_chars: Option<usize>,
    /// Rounds since it entered the prompt. Zero is this round.
    pub(crate) age_rounds: usize,
    /// Rounds until the sweep that takes it. Every item sharing that sweep
    /// reports the same number, because one pass takes them together.
    pub(crate) leaves_in: usize,
    /// Whether a budget pass already cut the body away.
    pub(crate) stubbed: bool,
}

impl PanelItem {
    /// Whether the request is still carrying this item's own bytes.
    fn is_resident(&self) -> bool {
        !self.stubbed
    }
}

/// Every addressable tool result in the array, oldest first.
///
/// A result is addressable because `core::store::with_event_address` appends
/// `[evt-<hex>]` to it, on the live path and on the resume rebuild alike. A
/// result whose `ToolCalled` emit failed has no id to render, so it carries no
/// address and this skips it. The label comes from the `tool_use` block it
/// answers, which is the only place the name survives.
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

/// Every addressable tool result in the array, by address alone.
///
/// Held apart from [`tool_result_items`] because the two callers want different
/// things. Ages want only the addresses. Building a whole [`PanelItem`] for
/// them counts the characters of every result body in the request, then throws
/// the count away.
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

/// Remember the round each address first appeared on, so ages are real.
///
/// The map only grows, and an address that leaves keeps its entry. Nothing
/// reads that entry again: within a turn no address comes back, because
/// re-running a call writes a new `ToolCalled` and so a new address. The map is
/// a turn local, so the next turn starts it empty and every age restarts at
/// zero.
///
/// It deliberately never overwrites. [`hold_open`] is the one thing that does.
pub(crate) fn note_first_seen(
    first_seen: &mut HashMap<String, usize>,
    messages: &[Message],
    round: usize,
) {
    for address in addresses_in(messages) {
        first_seen.entry(address.to_string()).or_insert(round);
    }
}

/// Whether a keep can be honoured: is the request carrying this address NOW?
///
/// Asked of the messages, never of the first-seen map. That map keeps an
/// address forever, so a keep on something a sweep already took would report
/// success and hold nothing.
///
/// A budget-stubbed body still counts. Its address line survives the cut, and
/// the panel prints the size that address would bring back. So holding the
/// pointer in view is a choice the model can price.
/// [`PanelItem::is_resident`] answers the other question, whether the bytes
/// themselves are still here.
pub(crate) fn carries_address(messages: &[Message], address: &str) -> bool {
    addresses_in(messages).any(|carried| carried == address)
}

/// Set one item's clock back to zero, which is the whole of what a keep does.
///
/// It OVERWRITES. [`note_first_seen`] uses `or_insert` on purpose, so reusing
/// it here would give a keep that reads perfectly and changes nothing.
pub(crate) fn hold_open(first_seen: &mut HashMap<String, usize>, address: &str, round: usize) {
    first_seen.insert(address.to_string(), round);
}

/// What the panel says about the request's fixed half.
///
/// Held apart from [`PanelItem`] because no keep reaches these. Naming them
/// anyway is what stops the model reading the addressable total as the whole
/// bill, and concluding it has more room than it has.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FixedRegions {
    pub(crate) system_chars: usize,
    pub(crate) tool_defs_chars: usize,
}

/// Everything the panel says that does not depend on its own size.
///
/// One struct rather than seven arguments, because `used_chars`, `budget_chars`
/// and `round` are adjacent bare `usize`. Transposing two of them compiles
/// clean and silently misreports the headline the model steers by.
pub(crate) struct PanelView<'a> {
    pub(crate) items: &'a [PanelItem],
    pub(crate) fixed: FixedRegions,
    pub(crate) held_open: &'a HashSet<String>,
    pub(crate) budget_chars: usize,
    pub(crate) round: usize,
    pub(crate) schedule: SweepSchedule,
}

impl PanelView<'_> {
    /// Render the panel, given the chars everything ELSE in the request takes.
    ///
    /// Two passes, because the panel is part of the request it measures. The
    /// first learns its own length and the second states the true total. Every
    /// figure is a fixed-width field, so the two passes are the same length and
    /// the total is exact rather than an estimate.
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
        // The subtotal the model steers by. The headline includes the fixed half
        // and this turn's prose, neither of which it can move. This is the part it
        // is still carrying and could still act on.
        block.push_str(&format!(
            "Still resident:  {} tokens across {} addressable items.\n",
            grouped(tokens(resident_chars)),
            resident.len()
        ));
        // No cap on what is held. A cap would make the engine choose which keeps to
        // honour, which is selective eviction in a new place. The bill is stated
        // instead, so the model sees the cost of its own keeping every round.
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
        // The one fact a per-item remainder cannot carry, so it is stated once.
        let until = schedule.rounds_to_next_sweep(round);
        block.push_str(&format!(
        "The next sweep is in {until} round(s). One runs every {} rounds, and takes everything \
         more than {} rounds old.\n",
        schedule.sweep_every_rounds, schedule.expire_after_rounds
    ));
        // The sweep runs at the TOP of a round, so a warning on the sweep round
        // itself arrives after the pages are gone. It belongs to the round before.
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
        // Three lines at most, and none of them repeats the standing instructions.
        // The panel rides in front of the cache mark, so every word here is
        // re-sent at write price on every round of the turn.
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
            // Two reasons an item is not listed, and they mean opposite things.
            // Under ROW_MIN_CHARS it is not worth acting on. Over it, the row cap
            // ran out, and those are the ones where letting go pays MOST. Saying
            // "each is under 500 chars" about the second kind sends the model away
            // from its biggest wins.
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

/// The rows worth listing, and the ones only worth counting.
///
/// Largest first for the choice, then back into arrival order for the render,
/// so age reads down the column.
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

/// A count right-aligned in a fixed field, so two renders are the same length.
fn field(n: usize) -> String {
    format!("{:>11}", grouped(n))
}

fn pad(n: usize, width: usize) -> String {
    format!("{:>width$}", n, width = width)
}

/// `12345` as `12,345`. A bare digit run is where a size gets misread by an
/// order of magnitude, and the panel exists to be read accurately.
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

/// What a superseded ENGINE-WRITTEN tail block collapses to, by its marker.
///
/// The panel and the document are both rewritten every round, so without this a
/// 75-round turn carries 75 of each. Neither is a `ToolResult`, so no trim pass
/// can reach them: pass 2 is assistant-only and passes 1, 3 and 4 match tool
/// blocks. They would be the fastest-growing thing in the prompt, and the only
/// thing the model cannot address.
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

/// Collapse the superseded panels and documents in the array.
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

/// Both blocks the loop appends are ones the fold above knows how to shrink.
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

/// Append a block to the newest message, as its own engine-tail block.
///
/// Append, never edit. The last message carries a cache breakpoint, so a byte
/// changed in any earlier one re-writes every byte after it.
///
/// The block goes in typed. It renders as ordinary text on the wire, and the
/// type is what tells the fold and the cache anchor that the engine wrote it.
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

/// A tail block's capture row, so the Context Viewer and the eval both see it.
///
/// One builder for both blocks. Nothing else counts their bytes, so a delta is
/// the block's own size. The two must be grouped and billed alike, or the
/// viewer and the eval disagree about which one grew.
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

/// The panel's own row.
pub(crate) fn panel_section(panel: &str) -> crate::engine::ContextSection {
    tail_block_section(PANEL_SECTION, panel)
}

#[cfg(test)]
#[path = "context_panel_tests.rs"]
mod tests;
