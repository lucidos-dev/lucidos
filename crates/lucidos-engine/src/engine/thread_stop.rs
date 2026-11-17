//! A human Stop holds automatic work until a human resumes the thread.
//! State is read from events, never from a live session or an in-memory flag.

use sqlx::PgPool;
use uuid::Uuid;

/// Unknown state holds: a delayed wake is recoverable, an unwanted turn is not.
pub(crate) async fn thread_is_stopped(pool: &PgPool, thread_id: Uuid) -> bool {
    match Transitions::before(pool, thread_id, i64::MAX, &TRANSITION_EVENTS)
        .next()
        .await
    {
        Ok(latest) => latest.is_some_and(|(_, stopped)| stopped),
        Err(e) => {
            crate::log_legacy!(
                "[ThreadStop] State of {thread_id} unreadable, holding automatic work: {e}"
            );
            true
        }
    }
}

/// Every event type [`transition`] can read as a Stop or a resume.
const TRANSITION_EVENTS: [&str; 7] = [
    "ResponseCanceled",
    "MessageReceived",
    "ContinuationRequested",
    "ContinuationStarted",
    "UserQuestionAnswered",
    "ApplyNowStarted",
    "ChangeApplied",
];

/// The event type [`transition`] can read as a Stop.
const STOP_EVENTS: [&str; 1] = ["ResponseCanceled"];

/// A thread's Stops (`true`) and resumes (`false`) before a sequence, newest
/// first. It reads a page at a time, so a walk that ends near the latest
/// event never reads the thread's whole history.
struct Transitions<'a> {
    pool: &'a PgPool,
    thread_id: Uuid,
    event_types: &'a [&'a str],
    before: i64,
    page: std::vec::IntoIter<(i64, bool)>,
    exhausted: bool,
}

impl<'a> Transitions<'a> {
    const PAGE: i64 = 32;

    fn before(pool: &'a PgPool, thread_id: Uuid, before: i64, event_types: &'a [&'a str]) -> Self {
        Self {
            pool,
            thread_id,
            event_types,
            before,
            page: Vec::new().into_iter(),
            exhausted: false,
        }
    }

    async fn next(&mut self) -> Result<Option<(i64, bool)>, sqlx::Error> {
        loop {
            if let Some(transition) = self.page.next() {
                return Ok(Some(transition));
            }
            if self.exhausted {
                return Ok(None);
            }
            // Only the fields `transition` reads, never a message's text. A
            // message the user took back never ran, so it resumes nothing.
            let rows: Vec<(i64, String, serde_json::Value)> = sqlx::query_as(
                "SELECT e.sequence, e.event_type, jsonb_build_object( \
                   'actor', e.payload->'actor', 'cause', e.payload->'cause', \
                   'origin', e.payload->'origin', 'mode', e.payload->'mode', \
                   'device_id', e.payload->'device_id') \
                 FROM events e WHERE e.thread_id = $1 AND e.event_type = ANY($2) \
                   AND e.sequence < $3 \
                   AND NOT (e.event_type = 'MessageReceived' AND EXISTS ( \
                     SELECT 1 FROM events r WHERE r.thread_id = $1 \
                       AND r.event_type = 'QueuedMessageRemoved' \
                       AND r.payload->>'removed_message_id' = e.id::text)) \
                 ORDER BY e.sequence DESC LIMIT $4",
            )
            .bind(self.thread_id)
            .bind(self.event_types)
            .bind(self.before)
            .bind(Self::PAGE)
            .fetch_all(self.pool)
            .await?;
            self.exhausted = rows.len() < Self::PAGE as usize;
            if let Some((oldest, _, _)) = rows.last() {
                self.before = *oldest;
            }
            self.page = rows
                .into_iter()
                .filter_map(|(sequence, kind, payload)| {
                    transition(&kind, &payload).map(|stopped| (sequence, stopped))
                })
                .collect::<Vec<_>>()
                .into_iter();
        }
    }
}

/// The first Stop of the run a resume at `before` lifts. `None` unless the
/// latest transition before `before` is a Stop, so only the first resume after
/// a Stop finds one.
async fn lifted_stop(
    pool: &PgPool,
    thread_id: Uuid,
    before: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let mut walk = Transitions::before(pool, thread_id, before, &TRANSITION_EVENTS);
    let mut first_stop = None;
    while let Some((sequence, stopped)) = walk.next().await? {
        if !stopped {
            break;
        }
        first_stop = Some(sequence);
    }
    Ok(first_stop)
}

/// The first Stop of the thread's latest run of Stops, which no resume came
/// between. Repeated Stops keep what the first one held.
async fn latest_stop_run(pool: &PgPool, thread_id: Uuid) -> Result<Option<i64>, sqlx::Error> {
    match Transitions::before(pool, thread_id, i64::MAX, &STOP_EVENTS)
        .next()
        .await?
    {
        Some((latest_stop, _)) => lifted_stop(pool, thread_id, latest_stop + 1).await,
        None => Ok(None),
    }
}

/// A transition is deliberately narrower than Human mode: an agent submitting
/// a saved draft is human content, but cannot lift the owner's Stop.
fn transition(event: &str, payload: &serde_json::Value) -> Option<bool> {
    let device_actor = payload["actor"]["kind"] == "device";
    match event {
        "ResponseCanceled" if device_actor && payload["cause"] == "user_stop" => Some(true),
        "MessageReceived"
            if payload["origin"]["kind"] == "device"
                || (payload["mode"] == "human" && payload["device_id"].is_string()) =>
        {
            Some(false)
        }
        "ContinuationRequested"
        | "ContinuationStarted"
        | "UserQuestionAnswered"
        | "ApplyNowStarted"
        | "ChangeApplied"
            if device_actor =>
        {
            Some(false)
        }
        _ => None,
    }
}

/// Ordinary messages, including a parent's follow-up, may run while stopped.
/// Redelivered engine messages remain engine input even though they render as
/// messages. A Continue checks its durable actor separately at the start gate.
pub(crate) fn is_automatic_input(kind: crate::engine::AgentInputKind) -> bool {
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

/// Read event-backed waits, including a queued delivery from before Stop.
/// Coding agents deliver each with its own anchor and read receipt.
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
    crate::engine::automation_pause::withheld_wait_reentries_since(pool, thread_id, since).await
}

/// Receipts retire deliveries, not subsequent sessions or repeated Stops. Two
/// sources may still owe one: the session that ran when the latest Stop run
/// began, and an earlier Stop run that no human turn answered.
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

/// The latest reply to a human turn before `before`, or 0. That turn read
/// every wake recorded before its reply: it carried the held ones, and the
/// rest ran on their own or drained into a live turn.
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

/// Deliver unread child wake cards as well as waits. Filed reports already
/// reach the agent through the turn-gap note and are not sent twice.
pub(crate) async fn held_inputs(
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
    let mut inputs: Vec<_> =
        crate::engine::agent_session::stranded_follow_ups::rebuilt_inputs(pool, rows)
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

pub(crate) fn queue_held_inputs(
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

/// Attach held deliveries after the resume's own words. Child reports are
/// already rebuilt in chat history and coding agents' turn-gap notes.
///
/// Only the first human resume after a Stop run carries them, so each held
/// wake arrives once. A wake that ran its own turn, or that a coding agent
/// read, was never held. A reply to the resume receipts what it carried, so
/// a carrier taken back, Stopped or failed leaves its wakes for the next one.
pub(crate) async fn resume_text(
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

/// A resume scan this slow is logged even when it attaches nothing, so a
/// stalled message names its cause.
const SLOW_RESUME_SCAN: std::time::Duration = std::time::Duration::from_millis(250);

/// `text` with the held waits a resume at `starter` lifts, and their count.
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

#[cfg(test)]
#[path = "thread_stop_tests.rs"]
mod tests;
