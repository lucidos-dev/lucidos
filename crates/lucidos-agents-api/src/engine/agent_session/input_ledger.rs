use std::collections::{HashSet, VecDeque};

use uuid::Uuid;

use crate::engine::child_report_channel::ChildReport;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::InputAnswered;
use crate::runtime::agent_runtime::forwards_verbatim;
use crate::runtime::{AgentEvent, AgentInput};

#[derive(Debug, Default)]
pub(crate) struct InputLedger {
    unread: VecDeque<OwedInput>,
    forwarded_here: HashSet<Uuid>,
    awaiting_tombstone: HashSet<Uuid>,
    read_this_turn: Vec<InputCompletion>,
}

type InputCompletion = tokio::sync::oneshot::Sender<InputAnswered>;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReadInput {
    pub event_id: Uuid,
    pub verbatim: bool,
}

#[derive(Debug)]
struct OwedInput {
    event_ids: Vec<Uuid>,
    uuid: Uuid,
    verbatim: bool,
    completion: Option<InputCompletion>,
    child_report: Option<ChildReport>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Settled {
    Nothing,
    Read(Vec<ReadInput>),
    Rejected,
}

impl InputLedger {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn forwarded(
        &mut self,
        input_event_ids: Vec<Uuid>,
        input: &AgentInput,
        completion: Option<InputCompletion>,
    ) {
        self.push(input_event_ids, input, completion, None);
    }

    pub(crate) fn forwarded_child_report(
        &mut self,
        card_id: Uuid,
        input: &AgentInput,
        completion: Option<InputCompletion>,
        text: String,
    ) {
        let report = ChildReport {
            card_id,
            text,
            sent: input.text.clone(),
        };
        self.push(vec![card_id], input, completion, Some(report));
    }

    fn push(
        &mut self,
        event_ids: Vec<Uuid>,
        input: &AgentInput,
        completion: Option<InputCompletion>,
        child_report: Option<ChildReport>,
    ) {
        self.forwarded_here.extend(event_ids.iter().copied());
        self.unread.push_back(OwedInput {
            event_ids,
            uuid: input.uuid,
            verbatim: forwards_verbatim(&input.text),
            completion,
            child_report,
        });
    }

    pub(crate) fn child_reports(&self) -> Vec<ChildReport> {
        self.unread
            .iter()
            .filter_map(|input| input.child_report.clone())
            .collect()
    }

    pub(crate) fn owed_child_reports(&self) -> usize {
        self.unread
            .iter()
            .filter(|input| input.child_report.is_some())
            .count()
    }

    pub(crate) fn child_report_taken(&mut self, card_id: Uuid) {
        self.unread
            .retain(|input| input.child_report.is_none() || input.event_ids != [card_id]);
    }

    pub(crate) fn withdrawable(&self, input_event_id: Uuid) -> Option<Uuid> {
        self.unread
            .iter()
            .find(|input| input.event_ids == [input_event_id])
            .map(|input| input.uuid)
    }

    pub(crate) fn owes(&self, uuid: Uuid) -> bool {
        self.unread.iter().any(|owed| owed.uuid == uuid)
    }

    pub(crate) fn forwarded_here(&self, input_event_id: Uuid) -> bool {
        self.forwarded_here.contains(&input_event_id)
    }

    pub(crate) fn withdrawn(&mut self, input_event_id: Uuid) {
        self.unread
            .retain(|input| input.event_ids != [input_event_id]);
        self.awaiting_tombstone.insert(input_event_id);
    }

    pub(crate) fn awaits_tombstone(&self, input_event_id: Uuid) -> bool {
        self.awaiting_tombstone.contains(&input_event_id)
    }

    pub(crate) fn tombstone_recorded(&mut self, input_event_id: Uuid) {
        self.awaiting_tombstone.remove(&input_event_id);
    }

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

    pub(crate) fn turn_ended(&mut self) {
        for completion in self.read_this_turn.drain(..) {
            let _ = completion.send(InputAnswered::TurnEnded);
        }
    }

    pub(crate) fn owed(&self) -> u32 {
        u32::try_from(self.unread.len()).unwrap_or(u32::MAX)
    }
}

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
