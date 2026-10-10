//! The inputs a coding-agent session forwarded to its agent that the agent has
//! not read yet. See ADR 0268.
//!
//! An input is owed from the moment the run loop forwards it until the agent
//! reports it read with `AgentEvent::InputRead`. A `Result` settles
//! nothing. Claude Code runs an input that arrived after the turn's last tool
//! call as a second turn, after the first turn's `Result`. Inputs that queued
//! behind a busy turn share one replay, so one read can settle several.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::runtime::{AgentEvent, AgentInput, ReplayedInput};

/// How long an idle agent may stay silent with inputs still owed before the
/// engine takes them as answered. A local command such as `/cost` runs a turn
/// with no replay. A real queued input starts its turn within milliseconds.
pub(crate) const SILENT_GRACE: Duration = Duration::from_secs(10);

/// Owed inputs, oldest first.
#[derive(Debug, Default)]
pub(crate) struct InputLedger {
    unread: VecDeque<OwedInput>,
    /// Inputs the agent dropped whose `QueuedMessageRemoved` is not recorded
    /// yet, by event id. A retry records it without asking the agent again.
    awaiting_tombstone: HashSet<Uuid>,
}

/// One write to the agent.
#[derive(Debug)]
struct OwedInput {
    /// The events that carried it: none for an engine-made prompt, several
    /// for coalesced messages.
    event_ids: Vec<Uuid>,
    /// What the agent was sent, to match against a replay.
    text: String,
    images: usize,
    /// [`AgentInput::uuid`], the name a withdraw uses.
    uuid: Uuid,
}

impl InputLedger {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The run loop handed `input` to the agent. `input_event_ids` are the
    /// events that carried it, such as its `MessageReceived`.
    pub(crate) fn forwarded(&mut self, input_event_ids: Vec<Uuid>, input: &AgentInput) {
        self.unread.push_back(OwedInput {
            event_ids: input_event_ids,
            text: input.text.clone(),
            images: input.images.len(),
            uuid: input.uuid,
        });
    }

    /// The uuid the agent knows `input_event_id`'s message by, while it is owed
    /// and its write carried that message alone. A coalesced write cannot be
    /// taken back one message at a time.
    pub(crate) fn withdrawable(&self, input_event_id: Uuid) -> Option<Uuid> {
        self.unread
            .iter()
            .find(|input| input.event_ids == [input_event_id])
            .map(|input| input.uuid)
    }

    /// The agent dropped the input carrying `input_event_id`. It will never be
    /// read, so nothing owes it now but its tombstone.
    pub(crate) fn withdrawn(&mut self, input_event_id: Uuid) {
        self.unread
            .retain(|input| input.event_ids != [input_event_id]);
        self.awaiting_tombstone.insert(input_event_id);
    }

    /// Whether the agent dropped `input_event_id`'s input and its tombstone is
    /// still unrecorded.
    pub(crate) fn awaits_tombstone(&self, input_event_id: Uuid) -> bool {
        self.awaiting_tombstone.contains(&input_event_id)
    }

    pub(crate) fn tombstone_recorded(&mut self, input_event_id: Uuid) {
        self.awaiting_tombstone.remove(&input_event_id);
    }

    /// Account for one agent event. Returns the events of the inputs it marks
    /// read.
    pub(crate) fn observe(&mut self, event: &AgentEvent) -> Vec<Uuid> {
        let AgentEvent::InputRead(replayed) = event else {
            return Vec::new();
        };
        let read = replayed.as_ref().map_or(1, |r| self.inputs_in(r));
        let read = read.min(self.unread.len());
        self.unread
            .drain(..read)
            .flat_map(|input| input.event_ids)
            .collect()
    }

    /// How many of the oldest owed inputs one replay carried. Joining the
    /// texts with newlines compares both of Claude Code's shapes. A replay
    /// that matches no run of inputs, such as `/compact` output, reads one.
    fn inputs_in(&self, replay: &ReplayedInput) -> usize {
        let replayed = replay.texts.join("\n");
        let mut texts: Vec<&str> = Vec::new();
        let mut images = 0;
        for (at, input) in self.unread.iter().enumerate() {
            if !input.text.is_empty() {
                texts.push(&input.text);
            }
            images += input.images;
            if images == replay.images && texts.join("\n") == replayed {
                return at + 1;
            }
        }
        1
    }

    /// Settle every owed input as answered without a read report, after the
    /// agent stayed silent at idle for [`SILENT_GRACE`]. Returns their events.
    pub(crate) fn settle_silent(&mut self) -> Vec<Uuid> {
        self.unread
            .drain(..)
            .flat_map(|input| input.event_ids)
            .collect()
    }

    pub(crate) fn owed(&self) -> u32 {
        u32::try_from(self.unread.len()).unwrap_or(u32::MAX)
    }
}

/// Record each input the agent read, so the transcript can mark it "Read".
/// `started_turn` says the read opened a turn after the last one ended.
pub(crate) async fn announce_reads(
    bus: &EventBus,
    thread_id: Uuid,
    meta: &EventMeta,
    input_event_ids: Vec<Uuid>,
    started_turn: bool,
) {
    for input_event_id in input_event_ids {
        bus.emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::CodingAgentInputRead {
                    input_event_id,
                    started_turn,
                },
                meta: meta.clone(),
            },
            "[AgentSession] CodingAgentInputRead",
        )
        .await;
    }
}

#[cfg(test)]
#[path = "input_ledger_tests.rs"]
mod tests;
