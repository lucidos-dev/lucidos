use std::sync::Arc;

use uuid::Uuid;

use super::question_park_sql::{open_posted_card_before_sql, open_posted_card_sql};
use super::thread_events::ThreadEvent;
use super::LucidosEngine;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

static CLOSED_CARD_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "SELECT c.sequence FROM events c \
         WHERE {open_before} AND NOT {open_now} \
         ORDER BY c.sequence LIMIT 1",
        open_before = open_posted_card_before_sql("c", "$1", "$2"),
        open_now = open_posted_card_sql("c", "$1"),
    )
});

const RESOLVED_PROMPT_SQL: &str = "SELECT p.sequence FROM events p, events r \
     WHERE r.aggregate_id = $1 AND r.sequence = $2 AND p.aggregate_id = $1 \
       AND p.sequence < $2 AND p.payload->>'request_id' = r.payload->>'request_id' \
       AND p.payload ? 'request_id' \
     ORDER BY p.sequence LIMIT 1";

async fn closed_card_posted_at(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    event: &ThreadEvent,
    closing: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let sql = if super::event_bus::prompt_resolution_id(event).is_some() {
        RESOLVED_PROMPT_SQL
    } else {
        CLOSED_CARD_SQL.as_str()
    };
    sqlx::query_scalar(sql)
        .bind(thread_id.to_string())
        .bind(closing)
        .fetch_optional(pool)
        .await
}

pub async fn release_held_wakes(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    event: &ThreadEvent,
    closing: i64,
) -> Result<(), BoxError> {
    let Some(posted) = closed_card_posted_at(engine.pool(), thread_id, event, closing).await?
    else {
        return Ok(());
    };
    let since = last_turn_start_before(engine.pool(), thread_id, posted).await?;
    hand_over_held_wakes(engine, thread_id, since).await
}

async fn last_turn_start_before(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    before: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COALESCE(MAX(sequence), 0) FROM events WHERE thread_id = $1 \
           AND event_type IN ('CodingAgentInputRead', 'ChatTurnSettingsResolved') \
           AND sequence < $2",
    )
    .bind(thread_id)
    .bind(before)
    .fetch_one(pool)
    .await
}

pub async fn hand_over_held_wakes(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    since: i64,
) -> Result<(), BoxError> {
    if !crate::core::store::thread_hold::may_start_turn(engine.pool(), thread_id).await {
        return Ok(());
    }
    let is_coding_agent = super::thread_agent(engine.pool(), thread_id).await?
        == Some(super::ThreadAgent::CodingAgent);
    let waits =
        super::thread_hold::unread_wait_reentries_since(engine.pool(), thread_id, since).await?;
    let delivered_waits = !waits.is_empty();
    for reentry in waits {
        engine
            .run_wait_reentry(thread_id, reentry, is_coding_agent)
            .await;
    }
    if is_coding_agent && !delivered_waits && reports_filed_since(engine, thread_id, since).await? {
        engine.request_report_read(thread_id, None).await?;
    }
    engine.agent_host().spawn_successor_start(engine, thread_id);
    Ok(())
}

async fn reports_filed_since(
    engine: &LucidosEngine,
    thread_id: Uuid,
    since: i64,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events WHERE thread_id = $1 \
           AND event_type IN ('ChildReportFiled', 'ChildThreadCompleted') AND sequence > $2)",
    )
    .bind(thread_id)
    .bind(since)
    .fetch_one(engine.pool())
    .await
}

#[cfg(test)]
#[path = "card_hold_tests.rs"]
mod tests;
