use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::engine::agent_session::input_ledger::InputLedger;
use crate::engine::agent_session::withdraw::{begin_withdraw, PendingWithdraws};
#[cfg(test)]
use crate::engine::child_report_channel::{child_reports_channel, ChildReport};
use crate::engine::child_report_channel::{
    ChildReportsLoopEnd, TakeChildReportsRequest, WithdrawnReports,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::types::{WithdrawInputRequest, WithdrawPurpose};
use crate::runtime::{AgentInput, InputWithdrawal, WithdrawRequest};

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
            crate::log_legacy!(
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
            defer_on_steer_failure: false,
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
            crate::log_legacy!(
                "[ChildReports] report {} could not go back to the agent: its process ended",
                report.card_id
            );
        }
    }
    moved
}

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

#[cfg(test)]
#[path = "child_reports_tests.rs"]
mod tests;
