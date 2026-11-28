use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use futures::FutureExt;
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;

use crate::engine::agent_session::input_ledger::InputLedger;
use crate::engine::agent_session::stranded_follow_ups::{input_fate, InputFate};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::types::{AgentSession, WithdrawInputRequest, WithdrawPurpose};
use crate::engine::LucidosEngine;
use crate::runtime::{InputWithdrawal, WithdrawRequest};

pub(crate) const NOT_YET_FORWARDED: &str =
    "No coding-agent session has been sent this message yet, so it cannot be taken back now.";
pub(crate) const NO_LIVE_SESSION: &str =
    "The coding agent's session has ended, so the message can no longer be taken back.";

const WITHDRAW_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub(crate) struct WithdrawAnswered {
    request: WithdrawInputRequest,
    outcome: InputWithdrawal,
}

pub(crate) type PendingWithdraws =
    FuturesUnordered<Pin<Box<dyn Future<Output = WithdrawAnswered> + Send>>>;

pub(crate) fn begin_withdraw(
    inputs: &InputLedger,
    driver: &mpsc::UnboundedSender<WithdrawRequest>,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    request: WithdrawInputRequest,
    pending: &mut PendingWithdraws,
) {
    if inputs.awaits_tombstone(request.input_event_id) {
        let outcome = InputWithdrawal::Withdrawn;
        pending.push(Box::pin(std::future::ready(WithdrawAnswered {
            request,
            outcome,
        })));
        return;
    }
    if !inputs.forwarded_here(request.input_event_id) {
        let pool = pool.clone();
        pending.push(Box::pin(async move {
            let outcome = unheld_outcome(&pool, thread_id, request.input_event_id).await;
            WithdrawAnswered { request, outcome }
        }));
        return;
    }
    let Some(input_uuid) = inputs.withdrawable(request.input_event_id) else {
        let _ = request.reply.send(InputWithdrawal::AlreadyRead);
        return;
    };
    let (reply, answer) = oneshot::channel();
    let asked = driver.send(WithdrawRequest { input_uuid, reply }).is_ok();
    pending.push(Box::pin(async move {
        let outcome = match (asked, answer.await) {
            (true, Ok(outcome)) => outcome,
            _ => InputWithdrawal::Refused(NO_LIVE_SESSION.to_string()),
        };
        WithdrawAnswered { request, outcome }
    }));
}

pub(crate) async fn withdraw_ceiling_before_stop(
    bus: &EventBus,
    thread_id: Uuid,
    inputs: &mut InputLedger,
    driver: &mpsc::UnboundedSender<WithdrawRequest>,
    pending: &mut PendingWithdraws,
    anchor: Uuid,
    actor: Option<MessageOrigin>,
) -> bool {
    let (reply, mut answer) = oneshot::channel();
    begin_withdraw(
        inputs,
        driver,
        bus.pool(),
        thread_id,
        WithdrawInputRequest {
            input_event_id: anchor,
            actor,
            purpose: WithdrawPurpose::Remove,
            reply,
        },
        pending,
    );
    let settled = tokio::time::timeout(WITHDRAW_TIMEOUT, async {
        loop {
            tokio::select! {
                outcome = &mut answer => break outcome,
                Some(answered) = pending.next() => {
                    finish_withdraw(bus, thread_id, inputs, answered).await;
                }
            }
        }
    })
    .await;
    matches!(
        settled,
        Ok(Ok(InputWithdrawal::Withdrawn | InputWithdrawal::AlreadyRead))
    )
}

async fn unheld_outcome(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    input_event_id: Uuid,
) -> InputWithdrawal {
    match input_fate(pool, thread_id, input_event_id).await {
        Ok(InputFate::Read) => InputWithdrawal::AlreadyRead,
        Ok(InputFate::Stranded) => InputWithdrawal::Withdrawn,
        Ok(InputFate::NeverForwarded) => InputWithdrawal::Refused(NOT_YET_FORWARDED.to_string()),
        Err(e) => InputWithdrawal::Refused(format!(
            "Could not check whether the coding agent read the message. Remove it again to retry: {e}"
        )),
    }
}

pub(crate) async fn settle_answered_withdraws(
    bus: &EventBus,
    thread_id: Uuid,
    inputs: &mut InputLedger,
    pending: &mut PendingWithdraws,
) {
    while let Some(Some(answered)) = pending.next().now_or_never() {
        finish_withdraw(bus, thread_id, inputs, answered).await;
    }
}

pub(crate) async fn finish_withdraw(
    bus: &EventBus,
    thread_id: Uuid,
    inputs: &mut InputLedger,
    WithdrawAnswered { request, outcome }: WithdrawAnswered,
) {
    let outcome = match (outcome, request.purpose) {
        (InputWithdrawal::Withdrawn, WithdrawPurpose::Remove) => {
            inputs.withdrawn(request.input_event_id);
            let recorded =
                record_tombstone(bus, thread_id, request.input_event_id, request.actor).await;
            if recorded == InputWithdrawal::Withdrawn {
                inputs.tombstone_recorded(request.input_event_id);
            }
            recorded
        }
        (other, _) => other,
    };
    let _ = request.reply.send(outcome);
}

async fn record_tombstone(
    bus: &EventBus,
    thread_id: Uuid,
    input_event_id: Uuid,
    actor: Option<MessageOrigin>,
) -> InputWithdrawal {
    let recorded = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::QueuedMessageRemoved {
                removed_message_id: input_event_id,
                restore_to_compose: false,
                compose: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                actor,
                ..EventMeta::NONE
            },
        })
        .await;
    match recorded {
        Ok(_) => InputWithdrawal::Withdrawn,
        Err(e) => InputWithdrawal::Refused(format!(
            "The agent dropped the message, but its removal could not be recorded. Remove it again to retry: {e}"
        )),
    }
}

async fn withdraw_from_session(
    sessions: &Mutex<HashMap<Uuid, AgentSession>>,
    thread_id: Uuid,
    input_event_id: Uuid,
    actor: Option<MessageOrigin>,
    timeout: Duration,
) -> Option<InputWithdrawal> {
    let sender = match sessions.lock().await.get(&thread_id) {
        Some(session) if session.is_live() => session.withdraw_tx.clone(),
        _ => return None,
    };
    let (reply, answer) = oneshot::channel();
    let request = WithdrawInputRequest {
        input_event_id,
        actor,
        purpose: WithdrawPurpose::Remove,
        reply,
    };
    sender.send(request).ok()?;
    match tokio::time::timeout(timeout, answer).await {
        Ok(Ok(outcome)) => Some(outcome),
        Ok(Err(_loop_ended)) => None,
        Err(_) => Some(InputWithdrawal::Refused(format!(
            "The coding agent did not answer within {} seconds. If it took the message back, the message disappears.",
            timeout.as_secs()
        ))),
    }
}

/// INVARIANT: Withdraw before dispatch without asking a coding agent to drop an input
/// it never received. The queue row lock orders this against admission.
async fn take_back_permission_held_input(
    bus: &EventBus,
    thread_id: Uuid,
    message_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let is_held: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM events WHERE thread_id=$1 AND id=$2
                       AND payload->>'queued_for_permission'='true')",
    )
    .bind(thread_id)
    .bind(message_id)
    .fetch_one(bus.pool())
    .await?;
    if !is_held {
        return Ok(false);
    }
    let result = bus
        .emit_with_meta_and_mutation(
            thread_id,
            ThreadEvent::QueuedMessageRemoved {
                removed_message_id: message_id,
                restore_to_compose: false,
                compose: None,
            },
            EventMeta {
                event_id: Some(Uuid::new_v5(
                    &thread_id,
                    format!("take-back:{message_id}").as_bytes(),
                )),
                channel: Some(EventChannel::ClaudeCode),
                actor,
                ..EventMeta::NONE
            },
            move |conn| {
                Box::pin(async move {
                    let row: Option<(String, i64)> = sqlx::query_as(
                        "SELECT q.status, m.sequence FROM thread_queue q
                         JOIN events m ON m.thread_id=q.thread_id AND m.id=$2
                         WHERE q.thread_id=$1 AND q.request->>'event_id'=m.id::text
                         FOR UPDATE OF q",
                    )
                    .bind(thread_id)
                    .bind(message_id)
                    .fetch_optional(&mut *conn)
                    .await?;
                    let Some((status, sequence)) = row else {
                        return Err(sqlx::Error::RowNotFound);
                    };
                    let waiting = status == "admitted"
                        && crate::engine::question_park_sql::open_permission_card_before(
                            conn,
                            thread_id,
                            Some(sequence),
                        )
                        .await?;
                    if status == "queued" || waiting {
                        Ok(())
                    } else {
                        Err(sqlx::Error::RowNotFound)
                    }
                })
            },
        )
        .await;
    match result {
        Ok(true) => Ok(true),
        Err(error) if matches!(error.downcast_ref(), Some(sqlx::Error::RowNotFound)) => Ok(false),
        other => {
            match crate::engine::consumption_claim::was_removed(bus.pool(), thread_id, message_id)
                .await?
            {
                true => Ok(true),
                false => other,
            }
        }
    }
}

#[easy_ext::ext(WithdrawExt)]
pub(crate) impl LucidosEngine {
    async fn withdraw_coding_agent_input(
        &self,
        thread_id: Uuid,
        input_event_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> InputWithdrawal {
        match take_back_permission_held_input(
            &self.event_bus,
            thread_id,
            input_event_id,
            actor.clone(),
        )
        .await
        {
            Ok(true) => return InputWithdrawal::Withdrawn,
            Ok(false) => {}
            Err(error) => {
                return InputWithdrawal::Refused(format!(
                    "Could not take back the message waiting for permission: {error}"
                ))
            }
        }
        let ask_session = || {
            withdraw_from_session(
                &self.agent_sessions,
                thread_id,
                input_event_id,
                actor.clone(),
                WITHDRAW_TIMEOUT,
            )
        };
        if let Some(outcome) = ask_session().await {
            return outcome;
        }
        let claim = self.claim_thread_for_spawn(thread_id, None);
        match withdraw_while_claimed(
            claim,
            &self.event_bus,
            thread_id,
            input_event_id,
            actor.clone(),
        )
        .await
        {
            Some(outcome) => outcome,
            None => ask_session()
                .await
                .unwrap_or_else(|| InputWithdrawal::Refused(NO_LIVE_SESSION.to_string())),
        }
    }
}

/// INVARIANT: Answer a withdraw while holding the thread claim `claim` yields, so no
/// session can start and forward the message while the tombstone is written.
/// `None` when a live run holds the thread.
async fn withdraw_while_claimed<C, E: std::fmt::Display>(
    claim: impl Future<Output = Result<Option<C>, E>>,
    bus: &EventBus,
    thread_id: Uuid,
    input_event_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Option<InputWithdrawal> {
    let _claim = match tokio::time::timeout(WITHDRAW_TIMEOUT, claim).await {
        Ok(Ok(Some(claim))) => claim,
        Ok(Ok(None)) => return None,
        Ok(Err(e)) => {
            return Some(InputWithdrawal::Refused(format!(
                "Could not hold the thread to take the message back: {e}"
            )))
        }
        Err(_) => {
            return Some(InputWithdrawal::Refused(
                "The coding agent's session is still winding down. Remove the message again in a moment."
                    .to_string(),
            ))
        }
    };
    Some(
        match unheld_outcome(bus.pool(), thread_id, input_event_id).await {
            InputWithdrawal::Withdrawn => {
                record_tombstone(bus, thread_id, input_event_id, actor).await
            }
            other => other,
        },
    )
}

#[cfg(test)]
#[path = "withdraw_tests.rs"]
mod tests;
