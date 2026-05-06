//! Resume orchestration for CC's `AskUserQuestion`. The PreToolUse hook in
//! `lucidos-cli ask-user-question-hook` handles the question lifecycle inside
//! the live CC subprocess (see `crate::engine::cc_settings` and
//! `crate::api::internal::ask_user_question`). This module's job is the
//! answer-side: emit `UserQuestionAnswered` once the user picks, then wake
//! the blocked hook so it can return CC's protocol-required `tool_result`.

use std::sync::Arc;
use uuid::Uuid;

use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{AnswerKind, EventChannel, EventMeta, ThreadEvent};
use crate::engine::LucidosEngine;

/// Outcome of answering a pending question. Maps to HTTP status codes in the API layer.
#[derive(Debug)]
pub enum AnswerResult {
    /// Answer persisted; any waiting hook has been notified. The CC subprocess
    /// is already alive and continuing in its existing session.
    Resumed,
    /// No matching `UserQuestionAsked` for this `tool_use_id`, or already answered.
    Conflict(String),
}

/// Find the `tool_use_id` of the latest unresolved `UserQuestionAsked` for
/// `thread_id`, if any. One round-trip — the LEFT JOIN filters out questions
/// that already have a matching answer.
pub async fn lookup_pending_question_tool_use_id(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Option<String> {
    let result: Result<Option<(String,)>, _> = sqlx::query_as(
        "SELECT q.payload->>'tool_use_id' \
         FROM events q \
         LEFT JOIN events a ON a.thread_id = q.thread_id \
              AND a.event_type = 'UserQuestionAnswered' \
              AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id' \
         WHERE q.thread_id = $1 AND q.event_type = 'UserQuestionAsked' AND a.id IS NULL \
         ORDER BY q.sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await;
    let row = match result {
        Ok(r) => r,
        Err(e) => {
            // Don't silently treat a DB outage as "no pending question" —
            // that would let the user's free-form text spawn a brand-new CC
            // turn over the unanswered one.
            log!(
                "[CCQuestion] DB lookup failed for pending question on {}: {}",
                thread_id,
                e
            );
            return None;
        }
    };
    row.map(|(t,)| t).filter(|t| !t.is_empty())
}

/// Look up whether `(thread_id, tool_use_id)` has a `UserQuestionAsked` and
/// (if so) whether it's already been answered. Single round-trip.
/// Returns `None` when no question exists, `Some(true)` when answered,
/// `Some(false)` when pending.
async fn find_pending_question(
    engine: &LucidosEngine,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<bool>, sqlx::Error> {
    let row: Option<(bool,)> = sqlx::query_as(
        "SELECT EXISTS(\
             SELECT 1 FROM events a \
             WHERE a.thread_id = q.thread_id \
               AND a.event_type = 'UserQuestionAnswered' \
               AND a.payload->>'tool_use_id' = $2 \
         ) \
         FROM events q \
         WHERE q.thread_id = $1 AND q.event_type = 'UserQuestionAsked' \
           AND q.payload->>'tool_use_id' = $2 \
         ORDER BY q.sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(tool_use_id)
    .fetch_optional(engine.pool())
    .await?;
    Ok(row.map(|(already_answered,)| already_answered))
}

/// Persist the user's answer and wake the PreToolUse hook blocked on this
/// `tool_use_id`. Idempotent: a partial unique index on
/// `(thread_id, tool_use_id) WHERE event_type='UserQuestionAnswered'` ensures
/// duplicate concurrent answers reject at the DB layer.
pub async fn answer_pending_question(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    tool_use_id: String,
    answer: AnswerKind,
) -> AnswerResult {
    match find_pending_question(engine, thread_id, &tool_use_id).await {
        Ok(Some(false)) => {}
        Ok(Some(true)) => {
            return AnswerResult::Conflict(format!(
                "Question {} on thread {} has already been answered",
                tool_use_id, thread_id
            ));
        }
        Ok(None) => {
            return AnswerResult::Conflict(format!(
                "No pending question for tool_use_id {} on thread {}",
                tool_use_id, thread_id
            ));
        }
        Err(e) => {
            log!(
                "[CCQuestion] DB lookup failed for {}/{}: {}",
                thread_id,
                tool_use_id,
                e
            );
            return AnswerResult::Conflict("Database lookup failed".into());
        }
    }

    if let Err(e) = engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::UserQuestionAnswered {
                tool_use_id: tool_use_id.clone(),
                answer: answer.clone(),
            },
            meta: EventMeta {
                channel: Some(EventChannel::CodingAgent),
                ..EventMeta::NONE
            },
        })
        .await
    {
        let msg = e.to_string();
        if msg.contains("events_user_question_answered_unique") {
            return AnswerResult::Conflict(format!(
                "Question {} on thread {} was answered concurrently",
                tool_use_id, thread_id
            ));
        }
        log!(
            "[CCQuestion] Failed to emit UserQuestionAnswered for {}/{}: {}",
            thread_id,
            tool_use_id,
            e
        );
        return AnswerResult::Conflict(format!("Failed to persist answer: {}", e));
    }

    // Wake the blocked hook (if any). No-op if nothing is registered:
    // - Engine restart killed the hook; on resume the endpoint's crash-recovery
    //   path reads the just-persisted UserQuestionAnswered from the DB instead.
    // - User answered before the hook re-registered after a transient error;
    //   same crash-recovery path covers it.
    engine
        .question_wait_registry
        .notify(
            &tool_use_id,
            crate::engine::cc_question_wait::AnswerPayload {
                answers: serde_json::to_value(&answer).unwrap_or(serde_json::Value::Null),
            },
        )
        .await;

    AnswerResult::Resumed
}
