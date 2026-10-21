//! The inputs a coding-agent session forwarded to its agent that the agent has
//! not read yet. See ADR 0268.
//!
//! An input is owed from the moment the run loop forwards it until the agent
//! names it: read with `AgentEvent::InputRead`, or rejected with
//! `AgentEvent::InputRejected`. A `Result` settles nothing. Claude Code runs an
//! input that arrived after the turn's last tool call as a second turn, after
//! the first turn's `Result`.
//!
//! An input can carry a completion sender. A read moves it to the turn that
//! read it, and that turn's end answers it. So a caller waiting on its own
//! prompt is never answered by a turn that was already running.

use std::collections::{HashSet, VecDeque};

use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::InputAnswered;
use crate::runtime::{AgentEvent, AgentInput};

/// Whether the engine forwards `text` exactly as typed, with no note or device
/// block in front. Claude Code runs a slash command only when the input starts
/// with `/`, so anything in front turns the command into a model prompt.
pub(crate) fn forwards_verbatim(text: &str) -> bool {
    text.starts_with('/')
}

/// Owed inputs, oldest first.
#[derive(Debug, Default)]
pub(crate) struct InputLedger {
    unread: VecDeque<OwedInput>,
    /// Every event this session forwarded, read or not. A withdraw for any
    /// other input asks the store, since only a dead process could hold it.
    forwarded_here: HashSet<Uuid>,
    /// Inputs the agent dropped whose `QueuedMessageRemoved` is not recorded
    /// yet, by event id. A retry records it without asking the agent again.
    awaiting_tombstone: HashSet<Uuid>,
    /// Completions of the inputs the current turn read, answered at its end.
    read_this_turn: Vec<InputCompletion>,
}

type InputCompletion = tokio::sync::oneshot::Sender<InputAnswered>;

/// One event of an input the agent read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReadInput {
    pub event_id: Uuid,
    /// The input went verbatim (see [`forwards_verbatim`]), so it carried no
    /// turn-gap note.
    pub verbatim: bool,
}

/// One write to the agent.
#[derive(Debug)]
struct OwedInput {
    /// The events that carried it: its origin, or for an engine prompt with
    /// none its `CodingAgentPromptSent`. Several for coalesced messages, and
    /// none when no event was written.
    event_ids: Vec<Uuid>,
    /// [`AgentInput::uuid`], the name the agent reads it by and a withdraw uses.
    uuid: Uuid,
    /// Whether it went verbatim. The text the agent was sent decides it, so
    /// the flag on its read cannot disagree with it.
    verbatim: bool,
    completion: Option<InputCompletion>,
}

/// What one agent event settled.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Settled {
    Nothing,
    /// The agent read an owed input, carried by these events.
    Read(Vec<ReadInput>),
    /// The agent rejected an owed input. Nobody will ever read it.
    Rejected,
}

impl InputLedger {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The run loop handed `input` to the agent. `input_event_ids` are the
    /// events that carried it, such as its `MessageReceived`. `completion` is
    /// answered when the turn that reads it ends.
    pub(crate) fn forwarded(
        &mut self,
        input_event_ids: Vec<Uuid>,
        input: &AgentInput,
        completion: Option<InputCompletion>,
    ) {
        self.forwarded_here.extend(input_event_ids.iter().copied());
        self.unread.push_back(OwedInput {
            event_ids: input_event_ids,
            uuid: input.uuid,
            verbatim: forwards_verbatim(&input.text),
            completion,
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

    /// Whether the agent has yet to read or reject the input named `uuid`.
    pub(crate) fn owes(&self, uuid: Uuid) -> bool {
        self.unread.iter().any(|owed| owed.uuid == uuid)
    }

    /// Whether this session forwarded `input_event_id`'s input to its agent.
    pub(crate) fn forwarded_here(&self, input_event_id: Uuid) -> bool {
        self.forwarded_here.contains(&input_event_id)
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

    /// Account for one agent event. Only an event naming an owed input's uuid
    /// settles it, so a replay of Claude Code's own output reads nothing.
    pub(crate) fn observe(&mut self, event: &AgentEvent) -> Settled {
        let (named, read) = match event {
            AgentEvent::InputRead { input } => (input, true),
            AgentEvent::InputRejected { input } => (input, false),
            _ => return Settled::Nothing,
        };
        let at = self.unread.iter().position(|owed| owed.uuid == *named);
        let Some(mut settled) = at.and_then(|at| self.unread.remove(at)) else {
            return Settled::Nothing;
        };
        if !read {
            if let Some(completion) = settled.completion {
                // A caller that stopped waiting dropped its receiver.
                let _ = completion.send(InputAnswered::Rejected);
            }
            return Settled::Rejected;
        }
        self.read_this_turn.extend(settled.completion.take());
        let verbatim = settled.verbatim;
        Settled::Read(
            settled
                .event_ids
                .into_iter()
                .map(|event_id| ReadInput { event_id, verbatim })
                .collect(),
        )
    }

    /// The current turn reached its `Result`. Answers every input it read.
    pub(crate) fn turn_ended(&mut self) {
        for completion in self.read_this_turn.drain(..) {
            let _ = completion.send(InputAnswered::TurnEnded);
        }
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
    reads: Vec<ReadInput>,
    started_turn: bool,
) {
    for ReadInput { event_id, verbatim } in reads {
        bus.emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::CodingAgentInputRead {
                    input_event_id: event_id,
                    started_turn,
                    verbatim,
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
