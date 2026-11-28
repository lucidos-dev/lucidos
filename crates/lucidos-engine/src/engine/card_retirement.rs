use std::sync::Arc;

use uuid::Uuid;

use super::agent_question::{answer_pending_question, AnswerResult};
use super::question_park_sql::{open_posted_card_sql, waiting_queued_card_sql};
use super::thread_events::{AnswerKind, ThreadEvent};
use super::LucidosEngine;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const TURN_END_EVENT_TYPES: &[&str] = &[
    "CodingAgentIdled",
    "ResponseGenerated",
    "ResponseFailed",
    "ResponseCanceled",
    "ResponseAborted",
    "SessionEnded",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retirement {
    Archived,
    TurnEnded,
}

fn retirement_for(event: &ThreadEvent) -> Option<Retirement> {
    match event.event_type() {
        "ThreadArchived" => Some(Retirement::Archived),
        t if TURN_END_EVENT_TYPES.contains(&t) => Some(Retirement::TurnEnded),
        _ => None,
    }
}

static WAITING_CARDS_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "SELECT q.payload->>'tool_use_id' FROM events q \
         WHERE ({open} OR (q.aggregate_id = $1 AND {queued})) \
           AND ($2 = false OR COALESCE( \
                 (q.payload->'permission_request'->>'holds_a_turn')::boolean, false)) \
           AND ($3::text[] IS NULL OR q.payload->'permission_request'->>'action' = ANY($3)) \
         ORDER BY q.sequence",
        open = open_posted_card_sql("q", "$1"),
        queued = waiting_queued_card_sql("q"),
    )
});

fn engine_step_actions() -> Result<Vec<String>, BoxError> {
    super::permission::Action::ENGINE_STEPS
        .iter()
        .map(|action| match serde_json::to_value(action)? {
            serde_json::Value::String(name) => Ok(name),
            other => Err(format!("an action serializes as a string, not {other}").into()),
        })
        .collect()
}

pub async fn retire_unwanted_cards(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    event: &ThreadEvent,
) -> Result<(), BoxError> {
    let Some(retirement) = retirement_for(event) else {
        return Ok(());
    };
    retire_waiting(engine, thread_id, retirement == Retirement::TurnEnded, None).await
}

pub async fn retire_step_cards(engine: &Arc<LucidosEngine>) -> Result<(), BoxError> {
    let steps = engine_step_actions()?;
    let threads: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT aggregate_id FROM events \
         WHERE event_type IN ('UserQuestionAsked', 'UserQuestionQueued') \
           AND payload->'permission_request'->>'action' = ANY($1)",
    )
    .bind(&steps)
    .fetch_all(engine.pool())
    .await?;
    for thread_id in threads.iter().filter_map(|t| Uuid::parse_str(t).ok()) {
        retire_waiting(engine, thread_id, false, Some(&steps)).await?;
    }
    Ok(())
}

async fn retire_waiting(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    turn_holding_only: bool,
    actions: Option<&[String]>,
) -> Result<(), BoxError> {
    let ids: Vec<String> = sqlx::query_scalar(&WAITING_CARDS_SQL)
        .bind(thread_id.to_string())
        .bind(turn_holding_only)
        .bind(actions)
        .fetch_all(engine.pool())
        .await?;
    for id in ids {
        if let AnswerResult::Conflict(reason) =
            answer_pending_question(engine, thread_id, id.clone(), AnswerKind::Superseded, None)
                .await
        {
            crate::log_legacy!("[CardRetirement] Card {id} on {thread_id} stays: {reason}");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "card_retirement_tests.rs"]
mod tests;
