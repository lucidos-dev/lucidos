//! The triage proposal and the gate in front of `apply_triage` (ADR 0349).
//!
//! `triage` records what it proposed as a `ThreadTriageProposed` event on the
//! calling thread. `apply_triage` then needs two facts the engine can read:
//! that proposal, and a reply from the user after it in the same thread.

use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent, TriageProposalEntry};

/// Record a proposal on the calling thread, with the agent as its actor.
pub(crate) async fn record_proposal(
    bus: &EventBus,
    caller: Uuid,
    entries: Vec<TriageProposalEntry>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    bus.emit(BusEvent::Thread {
        thread_id: caller,
        event: ThreadEvent::ThreadTriageProposed { entries },
        meta: EventMeta::with_actor(Some(
            crate::engine::chat::agent_archive::agent_thread_actor(caller),
        )),
    })
    .await?;
    Ok(())
}

/// Why `apply_triage` may not run at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApprovalRefusal {
    /// No `triage` ran in this thread.
    NoProposal,
    /// The user has not replied since the newest proposal.
    NoReply,
}

impl std::fmt::Display for ApprovalRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProposal => write!(
                f,
                "No triage was proposed in this thread. Run the threads tool's 'triage' \
                 action, show the result to the user, and wait for their answer."
            ),
            Self::NoReply => write!(
                f,
                "The user has not replied to the triage yet. Show it to them, ask which \
                 actions to apply, and call apply_triage only after they answer."
            ),
        }
    }
}

/// A user reply over `events e`: their answer to a question card, or their
/// message once the agent has read it. Both carry a `Device` actor.
///
/// A follow-up typed mid-turn is persisted before the agent reads it, so the
/// message must also be consumed: a turn took it as its request, or the loop
/// injected it (`chat/queued_recovery.rs`). A withdrawn one never counts.
const USER_REPLY_SQL: &str =
    "COALESCE(e.payload->'origin', e.payload->'actor')->>'kind' = 'device' \
     AND (e.event_type = 'UserQuestionAnswered' OR (e.event_type = 'MessageReceived' \
       AND NOT EXISTS (SELECT 1 FROM events r WHERE r.thread_id = e.thread_id \
         AND r.event_type = 'QueuedMessageRemoved' \
         AND r.payload->>'removed_message_id' = e.id::text) \
       AND EXISTS (SELECT 1 FROM events c WHERE c.thread_id = e.thread_id \
         AND (c.payload->>'request_event_id' = e.id::text \
           OR (c.event_type = 'UserPromptInjected' \
               AND c.payload->>'injected_message_id' = e.id::text)))))";

/// The newest proposal's entries, when the user replied after it.
pub(crate) async fn approved_proposal(
    pool: &PgPool,
    caller: Uuid,
) -> Result<Result<Vec<TriageProposalEntry>, ApprovalRefusal>, sqlx::Error> {
    let proposal: Option<(i64, serde_json::Value)> = sqlx::query_as(
        "SELECT sequence, payload->'entries' FROM events \
         WHERE thread_id = $1 AND event_type = 'ThreadTriageProposed' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(caller)
    .fetch_optional(pool)
    .await?;
    let Some((sequence, entries)) = proposal else {
        return Ok(Err(ApprovalRefusal::NoProposal));
    };
    let replied: bool = sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM events e \
         WHERE e.thread_id = $1 AND e.sequence > $2 AND {USER_REPLY_SQL})"
    ))
    .bind(caller)
    .bind(sequence)
    .fetch_one(pool)
    .await?;
    if !replied {
        return Ok(Err(ApprovalRefusal::NoReply));
    }
    serde_json::from_value(entries)
        .map(Ok)
        .map_err(|e| sqlx::Error::Decode(Box::new(e)))
}

#[cfg(test)]
#[path = "proposal_tests.rs"]
mod tests;
