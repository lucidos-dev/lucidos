use sqlx::PgPool;
use uuid::Uuid;

use crate::core::store::thread_stop::{latest_stop_run, lifted_stop};
pub use crate::core::store::thread_stop::{thread_is_stopped, transition};

pub fn is_automatic_input(kind: crate::engine::AgentInputKind) -> bool {
    use crate::engine::{AgentInputKind as K, RedeliveredInput as R};
    matches!(
        kind,
        K::Continuation
            | K::ReentryFromEngine
            | K::CeilingReentry
            | K::Redelivered {
                input: R::ChildReport
                    | R::EnginePrompt
                    | R::Message {
                        sender: crate::engine::thread_events::ActorMode::Engine
                    },
                ..
            }
    )
}

pub(crate) async fn held_waits(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Vec<crate::engine::event_wait::WaitReentry>, sqlx::Error> {
    if thread_is_stopped(pool, thread_id).await {
        return Ok(vec![]);
    }
    let Some(since) = held_boundary(pool, thread_id).await? else {
        return Ok(vec![]);
    };
    crate::engine::thread_hold::unread_wait_reentries_since(pool, thread_id, since).await
}

async fn held_boundary(pool: &PgPool, thread_id: Uuid) -> Result<Option<i64>, sqlx::Error> {
    let Some(first_stop) = latest_stop_run(pool, thread_id).await? else {
        return Ok(None);
    };
    let session: i64 = sqlx::query_scalar(
        "SELECT COALESCE(max(sequence), 0) FROM events WHERE thread_id = $1 \
         AND event_type = 'SessionStarted' AND sequence < $2",
    )
    .bind(thread_id)
    .bind(first_stop)
    .fetch_one(pool)
    .await?;
    Ok(Some(
        session.min(read_through(pool, thread_id, first_stop).await?),
    ))
}

async fn read_through(pool: &PgPool, thread_id: Uuid, before: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COALESCE((SELECT r.sequence FROM events r \
           JOIN events m ON m.id = (r.payload->>'request_event_id')::uuid \
           WHERE r.thread_id = $1 AND r.event_type = 'ResponseGenerated' AND r.sequence < $2 \
             AND m.thread_id = $1 AND (m.payload->'origin'->>'kind' = 'device' \
               OR m.payload->'actor'->>'kind' = 'device') \
           ORDER BY r.sequence DESC LIMIT 1), 0)",
    )
    .bind(thread_id)
    .bind(before)
    .fetch_one(pool)
    .await
}

pub async fn held_inputs(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Vec<crate::engine::AgentUserInput>, sqlx::Error> {
    use crate::engine::{AgentInputKind, AgentUserInput};
    let waits = held_waits(pool, thread_id).await?;
    if thread_is_stopped(pool, thread_id).await {
        return Ok(vec![]);
    }
    let Some(since) = held_boundary(pool, thread_id).await? else {
        return Ok(vec![]);
    };
    let rows: Vec<crate::core::EventRow> = sqlx::query_as(
        "SELECT id, event_type, payload, created, thread_id, sequence FROM events m \
         WHERE thread_id = $1 AND event_type = 'ChildThreadCompleted' \
           AND sequence > $2 \
           AND NOT EXISTS (SELECT 1 FROM events r WHERE r.thread_id = $1 \
             AND r.event_type = 'CodingAgentInputRead' AND r.payload->>'input_event_id' = m.id::text) \
         ORDER BY sequence",
    ).bind(thread_id).bind(since).fetch_all(pool).await?;
    let mut inputs: Vec<_> = crate::engine::replay_inputs::rebuilt_inputs(pool, rows)
        .await
        .into_iter()
        .map(|input| AgentUserInput {
            text: input.text,
            images: None,
            origin_event_id: Some(input.row.id),
            kind: AgentInputKind::ReentryFromEngine,
            completion: None,
        })
        .collect();
    inputs.extend(waits.into_iter().map(|wait| AgentUserInput {
        text: wait.text,
        images: None,
        origin_event_id: Some(wait.anchor_event_id),
        kind: AgentInputKind::ReentryFromEngine,
        completion: None,
    }));
    Ok(inputs)
}

pub fn queue_held_inputs(
    session: &mut crate::engine::AgentSession,
    held: Vec<crate::engine::AgentUserInput>,
    forwarded: impl Fn(Uuid) -> bool,
) {
    for input in held {
        let id = input
            .origin_event_id
            .expect("a held delivery has an anchor");
        if forwarded(id) || session.unforwarded_inputs.contains(&id) {
            continue;
        }
        if session.msg_tx.send(input).is_ok() {
            session.unforwarded_inputs.insert(id);
        } else {
            crate::log_legacy!(
                "[ThreadStop] Session closed before held delivery {id}; it remains unread"
            );
        }
    }
}

/// INVARIANT: Attach held deliveries after the resume's own words. Child reports are
/// already rebuilt in chat history and coding agents' turn-gap notes.
///
/// Only the first human resume after a Stop run carries them, so each held
/// wake arrives once. A wake that ran its own turn, or that a coding agent
/// read, was never held. A reply to the resume receipts what it carried, so
/// a carrier taken back, Stopped or failed leaves its wakes for the next one.
pub async fn resume_text(
    pool: &PgPool,
    thread_id: Uuid,
    text: &str,
    starter: Option<Uuid>,
) -> Result<String, sqlx::Error> {
    let Some(starter) = starter else {
        return Ok(text.into());
    };
    let started = std::time::Instant::now();
    let (resumed, held) = resume_with_held_waits(pool, thread_id, text, starter).await?;
    let elapsed = started.elapsed();
    if held > 0 || elapsed >= SLOW_RESUME_SCAN {
        crate::log_legacy!(
            "[ThreadStop] Thread {thread_id}: the resume scan took {} ms and attached {held} held wait(s)",
            elapsed.as_millis()
        );
    }
    Ok(resumed)
}

const SLOW_RESUME_SCAN: std::time::Duration = std::time::Duration::from_millis(250);

async fn resume_with_held_waits(
    pool: &PgPool,
    thread_id: Uuid,
    text: &str,
    starter: Uuid,
) -> Result<(String, usize), sqlx::Error> {
    let resume: Option<i64> = sqlx::query_scalar(
        "SELECT sequence FROM events WHERE id = $1 AND thread_id = $2 \
           AND (payload->'origin'->>'kind' = 'device' OR payload->'actor'->>'kind' = 'device')",
    )
    .bind(starter)
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some(resume) = resume else {
        return Ok((text.into(), 0));
    };
    let Some(first_stop) = lifted_stop(pool, thread_id, resume).await? else {
        return Ok((text.into(), 0));
    };
    let answered: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events WHERE thread_id = $1 \
           AND event_type = 'ResponseGenerated' AND payload->>'request_event_id' = $2::text)",
    )
    .bind(thread_id)
    .bind(starter)
    .fetch_one(pool)
    .await?;
    if answered {
        return Ok((text.into(), 0));
    }
    let since = read_through(pool, thread_id, first_stop).await?;
    let held: Vec<String> = sqlx::query_scalar(
        "WITH receipts AS MATERIALIZED ( \
           SELECT CASE r.event_type WHEN 'CodingAgentInputRead' THEN r.payload->>'input_event_id' \
                  ELSE r.payload->>'request_event_id' END AS read_event_id FROM events r \
            WHERE r.thread_id = $1 \
              AND r.event_type IN ('CodingAgentInputRead', 'ChatTurnSettingsResolved')) \
         SELECT COALESCE(p.payload->>'text', '') FROM events p \
         WHERE p.thread_id = $1 AND p.event_type = 'PromptInjected' \
           AND p.payload->'origin'->'reason'->>'kind' = $2 \
           AND p.sequence > $3 AND p.sequence < $4 \
           AND NOT EXISTS (SELECT 1 FROM receipts WHERE receipts.read_event_id = p.id::text) \
         ORDER BY p.sequence",
    )
    .bind(thread_id)
    .bind(crate::engine::thread_events::EVENT_WAIT_REASON_KIND)
    .bind(since)
    .bind(resume)
    .fetch_all(pool)
    .await?;
    let count = held.len();
    let resumed = std::iter::once(text.to_string())
        .chain(held)
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    Ok((resumed, count))
}
