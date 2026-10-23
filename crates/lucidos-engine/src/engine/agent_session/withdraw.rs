//! Withdrawing a queued follow-up the coding agent has not read yet. Plan:
//! `docs/plans/2026-09-29-withdraw-a-queued-claude-code-message.md`.
//!
//! A withdraw travels API, then run loop, then driver, then Claude Code. The
//! run loop owns the input ledger, so it decides whether the message is still
//! owed. It never blocks on Claude Code: it keeps each pending answer, and
//! settles the ledger and records the `QueuedMessageRemoved` tombstone once
//! the answer lands. The tombstone therefore always matches what Claude Code
//! did, even when the HTTP caller gave up first.
//!
//! For an input this session never forwarded, the store answers
//! (`stranded_follow_ups::InputFate`). One an earlier session forwarded and no
//! agent read sat in a dead process's queue, so it is withdrawn. One no session
//! forwarded may still be on its way to a spawn, so it is refused. With no
//! session at all, the engine answers while it holds the thread claim.

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
use crate::engine::types::AgentSession;
use crate::engine::LucidosEngine;
use crate::runtime::{CodingAgent, InputWithdrawal, WithdrawRequest};

pub(crate) const CODEX_UNSUPPORTED: &str = "Codex cannot take back a message it was sent.";
/// A message no session has forwarded yet may be on its way to a spawn, which
/// would deliver it over a tombstone.
pub(crate) const NOT_YET_FORWARDED: &str =
    "No coding-agent session has been sent this message yet, so it cannot be taken back now.";
pub(crate) const NO_LIVE_SESSION: &str =
    "The coding agent's session has ended, so the message can no longer be taken back.";

/// How long the API waits for a withdraw. Claude Code answers at once; the
/// slack covers a run loop busy with another event.
const WITHDRAW_TIMEOUT: Duration = Duration::from_secs(15);

/// A withdraw on its way to a session's run loop.
#[derive(Debug)]
pub struct WithdrawInputRequest {
    /// The `MessageReceived` event that carried the follow-up.
    pub input_event_id: Uuid,
    /// Who asked, for the tombstone.
    pub actor: Option<MessageOrigin>,
    pub reply: oneshot::Sender<InputWithdrawal>,
}

/// A withdraw Claude Code has answered, back in the run loop.
#[derive(Debug)]
pub(crate) struct WithdrawAnswered {
    request: WithdrawInputRequest,
    outcome: InputWithdrawal,
}

/// The run loop's withdraws still waiting on Claude Code's answer.
pub(crate) type PendingWithdraws =
    FuturesUnordered<Pin<Box<dyn Future<Output = WithdrawAnswered> + Send>>>;

/// Start answering a withdraw for an input the run loop has already forwarded.
/// The answer joins `pending`, unless it is known at once.
pub(crate) fn begin_withdraw(
    inputs: &InputLedger,
    driver: Option<&mpsc::UnboundedSender<WithdrawRequest>>,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    request: WithdrawInputRequest,
    pending: &mut PendingWithdraws,
) {
    if inputs.awaits_tombstone(request.input_event_id) {
        // The agent already dropped it; only the tombstone is left to record.
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
        // The requester may have gone away; nobody is left to tell.
        let _ = request.reply.send(InputWithdrawal::AlreadyRead);
        return;
    };
    let Some(driver) = driver else {
        let _ = request
            .reply
            .send(InputWithdrawal::Refused(CODEX_UNSUPPORTED.to_string()));
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

/// The answer for an input this session never forwarded. Never a read of its
/// own (the plan's first invariant).
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

/// Settle every withdraw Claude Code has already answered. The driver fills an
/// answer before it forwards the next stdout line. So running this before each
/// agent event keeps the ledger in stdout order: a read that follows a drop
/// never settles the dropped input.
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

/// Settle a withdraw Claude Code answered: on success the input leaves the
/// ledger and the tombstone hides its bubble on every device.
pub(crate) async fn finish_withdraw(
    bus: &EventBus,
    thread_id: Uuid,
    inputs: &mut InputLedger,
    WithdrawAnswered { request, outcome }: WithdrawAnswered,
) {
    let outcome = if outcome == InputWithdrawal::Withdrawn {
        inputs.withdrawn(request.input_event_id);
        let recorded =
            record_tombstone(bus, thread_id, request.input_event_id, request.actor).await;
        if recorded == InputWithdrawal::Withdrawn {
            inputs.tombstone_recorded(request.input_event_id);
        }
        recorded
    } else {
        outcome
    };
    let _ = request.reply.send(outcome);
}

/// Record the `QueuedMessageRemoved` that hides the bubble on every device.
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

/// Ask the thread's live session to take back `input_event_id`'s message.
/// `None` when no run loop took the request: no live session, or one that
/// ended before it answered.
async fn withdraw_from_session(
    sessions: &Mutex<HashMap<Uuid, AgentSession>>,
    thread_id: Uuid,
    input_event_id: Uuid,
    actor: Option<MessageOrigin>,
    timeout: Duration,
) -> Option<InputWithdrawal> {
    let sender = match sessions.lock().await.get(&thread_id) {
        Some(session) if session.is_live() => match session.coding_agent {
            CodingAgent::Codex => {
                return Some(InputWithdrawal::Refused(CODEX_UNSUPPORTED.to_string()))
            }
            CodingAgent::ClaudeCode => session.withdraw_tx.clone(),
        },
        _ => return None,
    };
    let (reply, answer) = oneshot::channel();
    let request = WithdrawInputRequest {
        input_event_id,
        actor,
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

impl LucidosEngine {
    /// Take back a follow-up the thread's coding agent has not read yet.
    pub(crate) async fn withdraw_coding_agent_input(
        &self,
        thread_id: Uuid,
        input_event_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> InputWithdrawal {
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
            // A run took the thread meanwhile, so its loop answers.
            None => ask_session()
                .await
                .unwrap_or_else(|| InputWithdrawal::Refused(NO_LIVE_SESSION.to_string())),
        }
    }
}

/// Answer a withdraw while holding the thread claim `claim` yields, so no
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
