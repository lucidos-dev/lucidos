//! A child's report reaching a coding agent mid-turn. Plan:
//! `docs/plans/2026-10-23-child-report-reaches-a-mid-turn-agent.md`.
//!
//! The fan-in forwards a report as an input, and an agent reads input only at
//! a turn or tool boundary. So the agent's own `lucidos` calls carry it
//! instead: a call takes the waiting reports, and the run loop withdraws each
//! queued copy first. A report the driver already delivered is not taken, so
//! the agent reads every report exactly once. The engine never interrupts a
//! turn for one.

use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use crate::engine::agent_session::input_ledger::InputLedger;
use crate::engine::agent_session::withdraw::{
    begin_withdraw, PendingWithdraws, WithdrawInputRequest, WithdrawPurpose,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::LucidosEngine;
use crate::runtime::{AgentInput, InputWithdrawal, WithdrawRequest};

/// A child report the agent has not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildReport {
    /// The `ChildThreadCompleted` card that carried it.
    pub card_id: Uuid,
    /// The report, as a take hands it over.
    pub text: String,
    /// What the agent was sent for it, notes included. A report that goes
    /// back to the agent goes as this.
    pub sent: String,
}

/// A request for the run loop to take every waiting child report.
#[derive(Debug)]
pub struct TakeChildReportsRequest {
    /// The agent that asked, for each `ChildReportTaken`.
    actor: Option<MessageOrigin>,
    /// The reports taken, oldest first.
    reply: oneshot::Sender<Vec<ChildReport>>,
}

/// The session's end: asks the run loop for a take, and watches how many
/// reports wait unread.
#[derive(Debug, Clone)]
pub struct ChildReportsHandle {
    take_tx: mpsc::UnboundedSender<TakeChildReportsRequest>,
    waiting_rx: watch::Receiver<usize>,
}

/// The run loop's end.
#[derive(Debug)]
pub(crate) struct ChildReportsLoopEnd {
    pub(crate) take_rx: mpsc::UnboundedReceiver<TakeChildReportsRequest>,
    /// Takes whose withdraws have all answered, for the run loop to settle.
    pub(crate) settled_rx: mpsc::UnboundedReceiver<WithdrawnReports>,
    settled_tx: mpsc::UnboundedSender<WithdrawnReports>,
    waiting_tx: watch::Sender<usize>,
}

/// The reports one take withdrew from the driver. They stay owed in the ledger
/// until [`settle_take`] hands them to the caller or sends them again.
#[derive(Debug)]
pub(crate) struct WithdrawnReports {
    request: TakeChildReportsRequest,
    reports: Vec<ChildReport>,
}

pub(crate) fn child_reports_channel() -> (ChildReportsHandle, ChildReportsLoopEnd) {
    let (take_tx, take_rx) = mpsc::unbounded_channel();
    let (settled_tx, settled_rx) = mpsc::unbounded_channel();
    let (waiting_tx, waiting_rx) = watch::channel(0);
    (
        ChildReportsHandle {
            take_tx,
            waiting_rx,
        },
        ChildReportsLoopEnd {
            take_rx,
            settled_rx,
            settled_tx,
            waiting_tx,
        },
    )
}

impl ChildReportsLoopEnd {
    /// Publish how many reports `inputs` holds unread. Call after every
    /// ledger change.
    pub(crate) fn publish(&self, inputs: &InputLedger) {
        self.waiting_tx.send_if_modified(|waiting| {
            let now = inputs.owed_child_reports();
            std::mem::replace(waiting, now) != now
        });
    }
}

/// How long a take waits for the run loop and the driver. Both answer at once;
/// the slack covers a run loop busy with another event.
const TAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Start taking every report `inputs` holds unread. Each withdraw joins
/// `pending`, and the run loop settles it like any other. Once all have
/// answered, the dropped reports arrive on `loop_end.settled_rx`.
pub(crate) fn begin_take(
    inputs: &InputLedger,
    driver: &mpsc::UnboundedSender<WithdrawRequest>,
    loop_end: &ChildReportsLoopEnd,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    request: TakeChildReportsRequest,
    pending: &mut PendingWithdraws,
) {
    if request.reply.is_closed() {
        // The caller gave up, so nobody would read what a take withdrew.
        return;
    }
    let mut answers = Vec::new();
    for report in inputs.child_reports() {
        let (reply, answer) = oneshot::channel();
        let withdraw = WithdrawInputRequest {
            input_event_id: report.card_id,
            actor: request.actor.clone(),
            purpose: WithdrawPurpose::TakeChildReport,
            reply,
        };
        begin_withdraw(inputs, driver, pool, thread_id, withdraw, pending);
        answers.push((report, answer));
    }
    let settled_tx = loop_end.settled_tx.clone();
    tokio::spawn(async move {
        let mut reports = Vec::new();
        for (report, answer) in answers {
            if answer.await == Ok(InputWithdrawal::Withdrawn) {
                reports.push(report);
            }
        }
        if let Err(unsettled) = settled_tx.send(WithdrawnReports { request, reports }) {
            crate::log!(
                "[ChildReports] session ended before it settled a take of {:?}",
                unsettled
                    .0
                    .reports
                    .iter()
                    .map(|report| report.card_id)
                    .collect::<Vec<_>>()
            );
        }
    });
}

/// Hand a take's withdrawn reports to its caller, and record each take. A
/// caller that left gets nothing, so each report goes to the agent again on
/// `agent_input_tx` and stays owed: it is read once either way. Returns each
/// resent write as (withdrawn uuid, new uuid), for state keyed by the write.
pub(crate) async fn settle_take(
    bus: &EventBus,
    thread_id: Uuid,
    inputs: &mut InputLedger,
    agent_input_tx: &mpsc::UnboundedSender<AgentInput>,
    WithdrawnReports { request, reports }: WithdrawnReports,
) -> Vec<(Uuid, Uuid)> {
    let card_ids: Vec<Uuid> = reports.iter().map(|report| report.card_id).collect();
    let reports = match request.reply.send(reports) {
        Ok(()) => {
            record_takes(bus, thread_id, inputs, request.actor, card_ids).await;
            return Vec::new();
        }
        Err(unread) => unread,
    };
    let mut moved = Vec::new();
    for report in reports {
        let resent = AgentInput {
            text: report.sent.clone(),
            images: Vec::new(),
            uuid: Uuid::new_v4(),
        };
        if let Some(withdrawn) = inputs.withdrawable(report.card_id) {
            moved.push((withdrawn, resent.uuid));
        }
        inputs.child_report_taken(report.card_id);
        inputs.forwarded_child_report(report.card_id, &resent, None, report.text);
        if agent_input_tx.send(resent).is_err() {
            crate::log!(
                "[ChildReports] report {} could not go back to the agent: its process ended",
                report.card_id
            );
        }
    }
    moved
}

/// The caller has the reports: nothing owes them now.
async fn record_takes(
    bus: &EventBus,
    thread_id: Uuid,
    inputs: &mut InputLedger,
    actor: Option<MessageOrigin>,
    card_ids: Vec<Uuid>,
) {
    for card_id in card_ids {
        inputs.child_report_taken(card_id);
        bus.emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ChildReportTaken {
                    input_event_id: card_id,
                },
                meta: EventMeta::with_actor(actor.clone()),
            },
            "[AgentSession] ChildReportTaken",
        )
        .await;
    }
}

impl LucidosEngine {
    /// Take every child report the thread's agent has not read, oldest first.
    /// Empty when no live session holds one.
    pub(crate) async fn take_child_reports(
        &self,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Vec<ChildReport> {
        let Some(handle) = self.child_reports_handle(thread_id).await else {
            return Vec::new();
        };
        let (reply, answer) = oneshot::channel();
        if handle
            .take_tx
            .send(TakeChildReportsRequest { actor, reply })
            .is_err()
        {
            return Vec::new();
        }
        match tokio::time::timeout(TAKE_TIMEOUT, answer).await {
            Ok(Ok(taken)) => taken,
            Ok(Err(_loop_ended)) => Vec::new(),
            Err(_) => {
                crate::log!(
                    "[ChildReports] take for thread {} not answered within {}s",
                    thread_id,
                    TAKE_TIMEOUT.as_secs()
                );
                Vec::new()
            }
        }
    }

    /// How many child reports wait unread on the thread's live session, as a
    /// watch. `None` when no live session runs.
    pub(crate) async fn child_reports_waiting(
        &self,
        thread_id: Uuid,
    ) -> Option<watch::Receiver<usize>> {
        self.child_reports_handle(thread_id)
            .await
            .map(|handle| handle.waiting_rx)
    }

    async fn child_reports_handle(&self, thread_id: Uuid) -> Option<ChildReportsHandle> {
        self.agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .filter(|session| session.is_live())
            .map(|session| session.child_reports.clone())
    }
}

#[cfg(test)]
#[path = "child_reports_tests.rs"]
mod tests;
