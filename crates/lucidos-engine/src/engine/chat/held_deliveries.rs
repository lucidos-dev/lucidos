//! Held deliveries: a child report or an event-wait delivery that reaches a
//! chat thread while its question outlived the turn that asked it.
//!
//! A live question turn already queues re-entries by injection (ADR 0255). A
//! restart keeps the question but not its turn, so the next re-entry would be
//! admitted as a fresh turn and overtake the card. The gate here holds it
//! instead, and the answer's resume carries it. See ADR 0321.
//!
//! The event store is the only state. A delivery is held while it follows the
//! open question and no turn has run since.

use uuid::Uuid;

use super::PreEmittedOrigin;
use crate::engine::thread_events::ThreadEvent;

/// The event a child's completion re-enters its parent on.
const CHILD_REPORT: &str = "ChildThreadCompleted";

/// Whether an incoming chat-lane re-entry must wait for the open question's
/// answer instead of starting a turn.
///
/// Three things must be true. The input is a wait re-entry, or an engine
/// re-entry on a child report. No turn is live to inject into. The thread has
/// an active question. A manual Continue and an answer's resume anchor on their
/// own notes, so they always run.
///
/// An unreadable anchor or question reads as "not held". The delivery then runs
/// a turn and may overtake the card, but it is never lost.
pub(crate) async fn delivery_is_held(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    pre_emitted_origin: Option<PreEmittedOrigin>,
    has_live_turn: bool,
) -> bool {
    if has_live_turn {
        return false;
    }
    let is_delivery = match pre_emitted_origin {
        Some(PreEmittedOrigin::WaitReentry(_)) => true,
        Some(PreEmittedOrigin::EngineReentry(anchor)) => anchor_is_child_report(pool, anchor).await,
        Some(PreEmittedOrigin::Message(_)) | None => false,
    };
    is_delivery
        && crate::engine::agent_question::lookup_active_question_tool_use_id(pool, thread_id)
            .await
            .is_some()
}

async fn anchor_is_child_report(pool: &sqlx::PgPool, anchor: Uuid) -> bool {
    match sqlx::query_scalar::<_, String>("SELECT event_type FROM events WHERE id = $1")
        .bind(anchor)
        .fetch_optional(pool)
        .await
    {
        Ok(event_type) => event_type.as_deref() == Some(CHILD_REPORT),
        Err(e) => {
            crate::log!(
                "[HeldDeliveries] anchor {} unreadable, running the re-entry: {}",
                anchor,
                e
            );
            false
        }
    }
}

/// What arrived behind a question while no turn was live to read it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct HeldDeliveries {
    /// Each held wait delivery's prose, oldest first. Only the re-entry anchor
    /// carries it, and history does not rebuild anchors.
    pub(crate) wait_texts: Vec<String>,
    /// How many child reports arrived. Their blocks are in the rebuilt history
    /// already, so the resume only has to point at them.
    pub(crate) child_reports: usize,
}

/// Deliveries held since the question `$2` was asked on thread `$1`, oldest
/// first.
///
/// A delivery is held when it follows the question and no turn activity follows
/// it. A wait anchor is recognised by its own payload, never by its neighbour: a
/// second resolution or a child report can land between the resolution and its
/// anchor. Only `emit_resolution` writes an agent-mode injection with no
/// `injected_message_id`. A live turn's acknowledgement of an injection names
/// the injected event.
const HELD_DELIVERIES_SQL: &str = "\
SELECT e.event_type, e.payload->>'text' \
  FROM events e \
 WHERE e.aggregate = 'thread' \
   AND e.aggregate_id = $1 \
   AND e.sequence > ( \
         SELECT q.sequence FROM events q \
          WHERE q.aggregate = 'thread' AND q.aggregate_id = $1 \
            AND q.event_type = 'UserQuestionAsked' \
            AND q.payload->>'tool_use_id' = $2 \
          ORDER BY q.sequence LIMIT 1) \
   AND ( e.event_type = 'ChildThreadCompleted' \
         OR ( e.event_type = 'UserPromptInjected' \
              AND e.payload->>'mode' = 'agent' \
              AND NOT e.payload ? 'injected_message_id' ) ) \
   AND NOT EXISTS ( \
         SELECT 1 FROM events t \
          WHERE t.aggregate = 'thread' AND t.aggregate_id = $1 \
            AND t.sequence > e.sequence \
            AND t.event_type = ANY($3) ) \
 ORDER BY e.sequence";

/// Read the deliveries held behind the question the user just answered.
///
/// Call it before the resume writes anything the turn would write. The answer's
/// own `ToolResult` is turn activity, and would make every delivery read as
/// consumed. An unreadable store yields nothing, logged: the resume still runs,
/// and each child report still reaches the model through history.
pub(crate) async fn held_deliveries(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    first_question_tool_use_id: &str,
) -> HeldDeliveries {
    let rows: Vec<(String, Option<String>)> = match sqlx::query_as(HELD_DELIVERIES_SQL)
        .bind(thread_id.to_string())
        .bind(first_question_tool_use_id)
        .bind(ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES)
        .fetch_all(pool)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            crate::log!(
                "[HeldDeliveries] read failed for thread {}: {}. The resume carries no \
                 held wait delivery",
                thread_id,
                e
            );
            return HeldDeliveries::default();
        }
    };
    let mut held = HeldDeliveries::default();
    for (event_type, text) in rows {
        if event_type == CHILD_REPORT {
            held.child_reports += 1;
        } else {
            held.wait_texts.push(text.unwrap_or_default());
        }
    }
    held
}

/// The resume note, followed by what waited behind the question.
pub(crate) fn resume_note_with_held_deliveries(note: &str, held: &HeldDeliveries) -> String {
    let mut parts = vec![note.to_string()];
    if !held.wait_texts.is_empty() || held.child_reports > 0 {
        parts.push(
            "These arrived while the question was open, and waited for the answer. \
             Handle them now."
                .to_string(),
        );
    }
    if held.child_reports > 0 {
        parts.push(format!(
            "Child thread reports waiting: {}. Each is in the history above.",
            held.child_reports
        ));
    }
    parts.extend(held.wait_texts.iter().cloned());
    parts.join("\n\n---\n\n")
}

#[cfg(test)]
#[path = "held_deliveries_tests.rs"]
mod tests;
