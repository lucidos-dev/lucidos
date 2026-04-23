//! Interactive `AskUserQuestion` resume orchestration. Once the user answers,
//! `answer_pending_question` emits `UserQuestionAnswered` and spawns a fresh
//! CC subprocess that resumes the session with the answer as a user message.

use std::sync::Arc;
use uuid::Uuid;

use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{
    AnswerKind, EventChannel, EventMeta, QuestionOption, ThreadEvent,
};
use crate::engine::CognosEngine;

/// Outcome of answering a pending question. Maps to HTTP status codes in the API layer.
#[derive(Debug)]
pub enum AnswerResult {
    /// Resume started successfully; CC is running again.
    Resumed,
    /// No matching `UserQuestionAsked` for this `tool_use_id`, or already answered.
    Conflict(String),
    /// Resume failed — for example, CC's session id can no longer be resumed.
    /// The caller should surface this to the user; UserQuestionAnswered { Canceled }
    /// is emitted before this is returned so the question card resolves cleanly.
    ResumeFailed(String),
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

/// Look up the latest `UserQuestionAsked` for `(thread_id, tool_use_id)` and
/// whether a matching answer already exists. Single round-trip; returns the
/// originating event id so callers can chain further events without an
/// extra query.
async fn find_pending_question(
    engine: &CognosEngine,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<PendingQuestion>, sqlx::Error> {
    let row: Option<(Uuid, serde_json::Value, bool)> = sqlx::query_as(
        "SELECT q.id, q.payload, EXISTS(\
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
    Ok(
        row.map(|(event_id, payload, already_answered)| PendingQuestion {
            event_id: Some(event_id),
            payload,
            already_answered,
        }),
    )
}

struct PendingQuestion {
    event_id: Option<Uuid>,
    payload: serde_json::Value,
    already_answered: bool,
}

/// Format the user-message text CC will see for the given answer. Sent as a
/// regular user message (not a `tool_result` block) — see the comment on the
/// send call site in `agent_session::run_direct_agent` for why.
/// Pure helper — unit-tested in this file.
pub fn format_answer_for_cc(answer: &AnswerKind, options: &[QuestionOption]) -> String {
    match answer {
        AnswerKind::Selected { option_id } => {
            let label = options
                .iter()
                .find(|o| &o.id == option_id)
                .map(|o| o.label.as_str())
                .unwrap_or(option_id.as_str());
            format!("User selected: {}", label)
        }
        AnswerKind::FreeText { text } => format!("User answered: {}", text),
        AnswerKind::Canceled => "User canceled the question.".into(),
    }
}

/// Public entry: emit `UserQuestionAnswered` and spawn a fresh CC that resumes
/// with the formatted answer as the next user message. Idempotent against
/// double-clicks via `find_pending_question`.
pub async fn answer_pending_question(
    engine: &Arc<CognosEngine>,
    thread_id: Uuid,
    tool_use_id: String,
    answer: AnswerKind,
) -> AnswerResult {
    let pending = match find_pending_question(engine, thread_id, &tool_use_id).await {
        Ok(Some(p)) => p,
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
    };
    if pending.already_answered {
        return AnswerResult::Conflict(format!(
            "Question {} on thread {} has already been answered",
            tool_use_id, thread_id
        ));
    }

    let cc_session_id = pending
        .payload
        .get("cc_session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let options: Vec<QuestionOption> = pending
        .payload
        .get("options")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|o| serde_json::from_value(o.clone()).ok())
                .collect()
        })
        .unwrap_or_default();

    // Emit the answer first so the projection sets status='running' before we
    // spawn CC. A partial unique index on (thread_id, tool_use_id) WHERE
    // event_type='UserQuestionAnswered' makes the second of two concurrent
    // emits fail at the DB layer, closing the race window between the
    // application-level idempotency check and the emit.
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

    // For Canceled, don't bother resuming CC — the answer event resolves the
    // question card; CC has been dead since the question fired.
    if matches!(answer, AnswerKind::Canceled) {
        return AnswerResult::Resumed;
    }

    if cc_session_id.is_empty() {
        log!(
            "[CCQuestion] No cc_session_id on UserQuestionAsked for {}/{} — cannot resume",
            thread_id,
            tool_use_id
        );
        return AnswerResult::ResumeFailed(
            "Original CC session id missing — cannot resume.".into(),
        );
    }

    let content = format_answer_for_cc(&answer, &options);

    // Spawn CC asynchronously so the HTTP handler returns immediately.
    // The originating event id (used as `request_event_id` on resume events)
    // came back with the question lookup — no need to round-trip again.
    let engine_arc = engine.clone();
    let origin_id = pending.event_id.unwrap_or_else(Uuid::new_v4);
    tokio::spawn(async move {
        let request_id = Uuid::new_v4();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let result = engine_arc
            .run_direct_agent(
                request_id,
                thread_id,
                &content,
                None,
                origin_id,
                &cancel_token,
                None,
                None,
                None,
                None,
                Some(cc_session_id),
                None,
                None,
            )
            .await;
        if let Err(e) = result {
            log!(
                "[CCQuestion] Resume after answer failed for {}: {}",
                thread_id,
                e
            );
            // Surface to user. Without this the thread may sit stuck in 'running' status
            // because the engine never emitted a terminal event.
            engine_arc
                .event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::ResponseFailed {
                            error: format!("Failed to resume CC after question: {}", e),
                        },
                        meta: EventMeta::NONE,
                    },
                    "[CCQuestion] ResponseFailed",
                )
                .await;
        }
    });

    AnswerResult::Resumed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Vec<QuestionOption> {
        vec![
            QuestionOption {
                id: "opt-0".into(),
                label: "Yes".into(),
                description: None,
            },
            QuestionOption {
                id: "opt-1".into(),
                label: "Cancel build".into(),
                description: Some("Stop the build".into()),
            },
        ]
    }

    #[test]
    fn format_selected_uses_label_not_id() {
        let content = format_answer_for_cc(
            &AnswerKind::Selected {
                option_id: "opt-1".into(),
            },
            &opts(),
        );
        assert_eq!(content, "User selected: Cancel build");
    }

    #[test]
    fn format_selected_falls_back_to_id_if_label_missing() {
        // Robust to options being out of sync (shouldn't happen normally, but cheap insurance).
        let content = format_answer_for_cc(
            &AnswerKind::Selected {
                option_id: "opt-99".into(),
            },
            &opts(),
        );
        assert_eq!(content, "User selected: opt-99");
    }

    #[test]
    fn format_free_text_includes_user_text() {
        let content = format_answer_for_cc(
            &AnswerKind::FreeText {
                text: "go ahead but skip tests".into(),
            },
            &opts(),
        );
        assert_eq!(content, "User answered: go ahead but skip tests");
    }

    #[test]
    fn format_canceled_returns_canceled_text() {
        let content = format_answer_for_cc(&AnswerKind::Canceled, &opts());
        assert_eq!(content, "User canceled the question.");
    }
}
