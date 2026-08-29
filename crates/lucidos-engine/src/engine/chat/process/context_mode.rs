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
//! The mode is one rule and two instruments. A tool result stays until a sweep
//! takes it, whole, with the call that made it. The context panel shows what
//! that leaves, and the working understanding is where the model writes what a
//! result told it. See ADR 0109 and
//! `docs/plans/2026-08-24-the-working-understanding-and-the-ten-round-window.md`.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::llm::ToolCapabilities;

/// The memory-recall section's name, as the prompt and the capture both spell
/// it. Shared with `context_build`, and read by the eval's manipulation check.
pub(crate) const MEMORY_SECTION: &str = "Long-term Memory";

/// The conversation-history section's name. Same sharing as [`MEMORY_SECTION`].
///
/// Not to be confused with `Conversation`, the within-turn delta the agentic
/// loop appends. That one is what the tool loop added this turn. The mode does
/// not reorder it, and the only thing that leaves it is a pair a sweep took.
pub(crate) const HISTORY_SECTION: &str = "Conversation History";

/// The one call that reads a swept result back.
///
/// Stated ONCE per request, in the standing instructions, and nowhere else. A
/// request naming its most expensive move on every line is a request asking for
/// it: read-backs ran 31 in one mode arm against 1 in its control.
pub(crate) const RESULT_RECOVERY: &str = "events(action=\"query\", event_id=\"evt-<hex>\")";

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

/// How long a result gets before a sweep may take it, by default.
pub const DEFAULT_EXPIRE_AFTER_ROUNDS: usize = 5;
/// How often the sweep runs, by default.
pub const DEFAULT_SWEEP_EVERY_ROUNDS: usize = 10;

/// A ceiling on both numbers, so a mistyped preference cannot make
/// [`SweepSchedule::leaves_at`] walk forever.
const MAX_SCHEDULE_ROUNDS: usize = 1_000;

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

/// When results leave: an expiry age and how often the sweep runs.
///
/// **The clear-out is a schedule, not a per-round drop.** Removing a pair from
/// the middle of the array invalidates every cached byte after the cut, and
/// both message-tier breakpoints sit behind it. Paying that on every round
/// costs about ten times what it saves. So nine rounds in ten are pure appends,
/// and the tenth takes everything past expiry at once.
///
/// On the defaults an item lives 6 to 15 rounds, averaging the ten decision 1
/// asks for. The panel states each item's exact remainder, so the variable
/// lifetime never reaches the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SweepSchedule {
    pub(crate) expire_after_rounds: usize,
    pub(crate) sweep_every_rounds: usize,
}

impl Default for SweepSchedule {
    fn default() -> Self {
        Self::new(DEFAULT_EXPIRE_AFTER_ROUNDS, DEFAULT_SWEEP_EVERY_ROUNDS)
    }
}

impl SweepSchedule {
    /// Both numbers are provisional, so the eval sweeps them without a rebuild.
    /// A zero interval would divide by zero, and either number unbounded would
    /// leave [`Self::leaves_at`] walking, so both are clamped here.
    pub(crate) fn new(expire_after_rounds: usize, sweep_every_rounds: usize) -> Self {
        Self {
            expire_after_rounds: expire_after_rounds.min(MAX_SCHEDULE_ROUNDS),
            sweep_every_rounds: sweep_every_rounds.clamp(1, MAX_SCHEDULE_ROUNDS),
        }
    }

    /// Whether the pass runs at the top of this round.
    pub(crate) fn is_sweep_round(self, round: usize) -> bool {
        round > 0 && round.is_multiple_of(self.sweep_every_rounds)
    }

    /// The next round the pass runs on, strictly after `round`.
    pub(crate) fn next_sweep_after(self, round: usize) -> usize {
        (round / self.sweep_every_rounds + 1) * self.sweep_every_rounds
    }

    /// How many rounds until the next sweep. The panel states this once.
    pub(crate) fn rounds_to_next_sweep(self, round: usize) -> usize {
        self.next_sweep_after(round).saturating_sub(round)
    }

    /// Whether an item first seen on `first_seen` is old enough to go.
    pub(crate) fn is_past_expiry(self, first_seen: usize, round: usize) -> bool {
        round.saturating_sub(first_seen) > self.expire_after_rounds
    }

    /// The round an item's pair leaves on, given it survived `round`.
    pub(crate) fn leaves_at(self, first_seen: usize, round: usize) -> usize {
        let mut at = self.next_sweep_after(round);
        while !self.is_past_expiry(first_seen, at) {
            at += self.sweep_every_rounds;
        }
        at
    }

    /// How many rounds an item has left, counted from `round`.
    pub(crate) fn leaves_in(self, first_seen: usize, round: usize) -> usize {
        self.leaves_at(first_seen, round).saturating_sub(round)
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

/// Take every pair past expiry, whole, on a sweep round.
///
/// The rule the whole mode rests on. A result and the call that made it leave
/// together, and nothing stands in their place: no placeholder, no reference,
/// no engine-kept index of what happened. It is uniform, so the engine picks no
/// victims and the model picks what stays by keeping it open.
///
/// **Nothing moves on any other round.** That is what buys the cache back, so a
/// stray mid-round removal costs the whole schedule.
///
/// The current round's pair always stays: the API requires a `tool_use` to be
/// answered in the same exchange.
///
/// **Two assistant messages can end up side by side, and that is accepted.**
/// Taking a pair whole empties the user message that held the results. Probed
/// against Vertex `claude-haiku-4-5`: three consecutive assistant messages
/// answered 200, and so did the same array ending in a `tool_use` and its
/// `tool_result`. No same-role merge runs here, and one would rewrite the
/// prefix every cache hit depends on.
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

    // Text left behind by the pair is the engine's own, on the USER side only.
    // `build_tool_result_blocks` appends one instruction block to every results
    // message. An assistant's text is the model's own words, and they outlive
    // the call that sat beside them.
    let mut hollowed: HashSet<usize> = HashSet::new();
    for (index, message) in messages.iter_mut().enumerate() {
        let user = message.role == "user";
        let MessageContent::Blocks(blocks) = &mut message.content else {
            continue;
        };
        let before = blocks.len();
        // An image rides after the results in the same message. It holds no
        // address of its own, so nothing can name it once its result is gone.
        // Dropped only when EVERY result here is doomed. A result held open
        // could be the one that produced the image, and nothing pairs them by
        // the time the blocks are built.
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

/// The system-prompt section the mode adds, or the empty string when it is off.
///
/// It lives in the cached prefix because it never changes within a turn (ADR
/// 0085 decision 8). It is appended the way `coding_surface_section` is, so the
/// off path is byte-identical by construction rather than by careful
/// whitespace.
pub(crate) fn system_prompt_section(mode: ContextMode, schedule: SweepSchedule) -> String {
    if !mode.is_on() {
        return String::new();
    }
    format!("\n\n{}", rendered_context_mode_prompt_at(schedule))
}

/// The mode's prompt text at a given schedule, with no leading separator.
///
/// Public because the eval's `guidance_hash` covers the text a model actually
/// saw. Hashing a source literal would give two arms swept at different values
/// the same hash, which blinds the one instrument built for the sweep.
pub fn rendered_context_mode_prompt(
    expire_after_rounds: usize,
    sweep_every_rounds: usize,
) -> String {
    rendered_context_mode_prompt_at(SweepSchedule::new(expire_after_rounds, sweep_every_rounds))
}

fn rendered_context_mode_prompt_at(schedule: SweepSchedule) -> String {
    let expire = schedule.expire_after_rounds;
    let sweep = schedule.sweep_every_rounds;
    format!(
        "SELF-CURATED CONTEXT MODE IS ON FOR THIS WORKSPACE:
You can see your own context, and you decide what survives it.

- A CONTEXT PANEL rides at the tail of every round. One row per addressable
  item: its evt-<hex> address, its size, its age in rounds, and how many rounds
  it has left. Read the panel. Nothing else tells you how full you are.
- TOOL RESULTS ARE SWEPT AWAY IN BATCHES. Every {sweep} rounds the sweep takes
  everything more than {expire} rounds old, and the call that made each one
  leaves with it. Nothing stands in their place. Doing nothing holds a result
  until then, so most of the time you need do nothing at all.
- KEEP ONE OPEN BY NAMING IT. Write its address under a [KEEP OPEN] heading in
  your working understanding and its clock goes back to zero. Reach for it when
  the panel shows something running down that you are still working with. That
  is something you can see. You are not being asked to predict, on the round a
  result arrives, whether you will still want it later.
- YOUR WORKING UNDERSTANDING is the one thing that outlives a turn. You write it
  as ordinary text in your reply, and it comes back to you at the tail of every
  round, whether you touched it or not.
- A NEW TURN REBUILDS YOUR LAST FEW TOOL CALLS, same addresses, age back to
  zero. Older ones are one summary line in the history.
- Everything else is unchanged, the app, file and URL you are looking at
  included.

NOTHING IS LOST, ONLY UNSENT. The event store holds all of it:
  {RESULT_RECOVERY}
    reads one result back in full.
  events(action=\"query\", thread_id=\"current\", event_type=\"ToolCalled\")
    lists every call this thread has made.

{}",
        note_guidance(schedule)
    )
}

/// The note-writing guidance the system prompt carries.
///
/// It is the design record's own words, apart from the departures the mode
/// forced: the two numbers, the marked headings, the sweep, and the write
/// rhythm. Nowhere does it say a note is cheaper than a keep. That sentence, in
/// three wordings across three surfaces, is what produced zero keeps in 301
/// rounds.
fn note_guidance(schedule: SweepSchedule) -> String {
    let expire = schedule.expire_after_rounds;
    let sweep = schedule.sweep_every_rounds;
    format!(
        "YOUR WORKING UNDERSTANDING

It is your picture of the job. What you worked out, what you decided and why,
what a result told you, what you ruled out, and what you mean to do next. Write
it in your own words, in whatever shape reads best.

Mark it out inside your reply:

  [WORKING UNDERSTANDING]
  ...the whole thing, replacing what you had...
  [/WORKING UNDERSTANDING]

To add to it without restating the rest:

  [WORKING UNDERSTANDING: ADD]
  ...the new lines only...
  [/WORKING UNDERSTANDING]

Close the block. Everything after an unclosed marker becomes part of it.

Put the block in the same reply as your next tool call. Text and a tool call
travel together, so writing this costs you no round.

WHEN TO WRITE. Add to it on an ordinary round: the lines that round taught you,
and nothing else. Rewrite it whole on the round the panel says the sweep is
next. That is the last round the pages about to leave are still in front of you,
so it is where the rewrite is worth most. Over {sweep} rounds that is one
consolidation, and the rest are cheap.

TRUST WHAT YOU WROTE. You wrote it while you were looking at the thing. Read it
back and act on it. Go to the source again when something tells you an entry is
wrong, and not because it is old.

Copy in whatever you need, raw text from a result included. An exact error
string, a signature you are about to match, a line you will compare against:
paste it. Once written it caches like everything else.

A [CONSTRAINTS] heading renders under your document every round, empty or
not. What goes under it is what the user told you to do or not do, in words that
have since scrolled away. \"Do not touch the frontend.\" \"Call it X, not Y.\" You
will see the heading every round, so you will see when it is empty. Write it
inside your block under the same heading.

Your todo list lives in the same block, under a [TODO] heading:

  [TODO]
  - [ ] not started
  - [>] the one you are on
  - [x] done

You write those three. Two more, waiting and abandoned, are written for you and
come back as words. It is the same list the user sees.

To hold an item past the sweep, name it under a [KEEP OPEN] heading:

  [KEEP OPEN]
  evt-<hex>

One address per line, copied from the panel, and nothing else on the line. Each
one sets that item's clock back to zero, so it gets another {expire} rounds at
least. The keep applies once, from the reply that wrote it, so a later rewrite
of your document does not repeat it. Nothing you hold is exempt from the
trimmer: when a request will not fit, the wall takes held items last, and it
still takes them."
    )
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
