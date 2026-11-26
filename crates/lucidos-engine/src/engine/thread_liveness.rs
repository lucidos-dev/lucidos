//! *Thread liveness*: whether a thread is working, quiet, parked, owed a
//! session, or at rest, as the engine reports it (ADR 0679).
//!
//! [`derive`] is the one definition. SQL gathers its facts, and every surface
//! (agent tools, HTTP, SDK, CLI, the UI) reads the result rather than mapping
//! a status and timestamps itself. Reading it writes nothing.

use std::collections::HashMap;

use serde::Deserialize;
use sqlx::{types::Json, PgPool};
use uuid::Uuid;

use crate::core::store::thread_hold::{thread_hold, turn_hold, TurnHold};
use crate::core::store::{format_display_title, EventWaitSummary, UNTITLED_THREAD};

use super::thread_events::SessionStart;
use super::thread_lifecycle::ThreadStatus;

pub use crate::engine::thread_events::liveness::*;

#[derive(sqlx::FromRow)]
struct LivenessRow {
    thread_id: Uuid,
    status: String,
    live_event_waits: Json<Vec<EventWaitSummary>>,
    awaited_children: Json<Vec<AwaitedChildRow>>,
    expected_wake: Option<Json<LivenessExpectedWake>>,
    last_activity_age_secs: Option<i64>,
    sessions_started: i64,
    sessions_opened: i64,
    opening_kind: Option<String>,
    session_age_secs: Option<i64>,
    handoff_pending: bool,
    between_turns: bool,
    queued: bool,
    on_hold: bool,
    as_of: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
struct AwaitedChildRow {
    thread_id: Uuid,
    title: Option<String>,
    first_message: Option<String>,
}

impl LivenessRow {
    fn facts(&self, pending_handoff: Option<SessionStart>) -> LivenessFacts {
        LivenessFacts {
            status: ThreadStatus::parse(&self.status),
            last_activity_age_secs: self.last_activity_age_secs,
            pending_handoff,
            waits: !self.live_event_waits.is_empty() || !self.awaited_children.is_empty(),
            between_turns: self.between_turns,
            queued: self.queued,
            held: false,
            on_hold: self.on_hold,
        }
    }

    fn into_liveness(self, mut facts: LivenessFacts, hold: Option<TurnHold>) -> ThreadLiveness {
        facts.held = hold.is_some();
        let continuity = match self.opening_kind.as_deref() {
            Some("handoff") => SessionOpening::Handoff,
            Some(_) => SessionOpening::Fresh,
            None => SessionOpening::First,
        };
        let ordinal = u32::try_from(self.sessions_opened)
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        let state = derive(&facts);
        ThreadLiveness {
            state,
            status: facts.status,
            last_activity_age_secs: self.last_activity_age_secs,
            session: (self.sessions_started > 0).then(|| LivenessSession {
                ordinal,
                continuity,
                age_secs: self.session_age_secs.unwrap_or(0),
            }),
            handoff: self.handoff_pending.then_some(LivenessHandoff::Pending),
            live_event_waits: self
                .live_event_waits
                .0
                .into_iter()
                .map(|wait| LivenessEventWait {
                    reason: wait.reason,
                    on: wait.on,
                    expires_at: wait.expires_at,
                    expires_in_secs: (wait.expires_at - self.as_of).num_seconds().max(0),
                    background_timing: wait.background_timing,
                })
                .collect(),
            awaited_children: self
                .awaited_children
                .0
                .into_iter()
                .map(|child| {
                    let title = format_display_title(child.title, child.first_message);
                    LivenessAwaitedChild {
                        thread_id: child.thread_id,
                        title: if title.is_empty() {
                            UNTITLED_THREAD.into()
                        } else {
                            title
                        },
                    }
                })
                .collect(),
            expected_wake: self
                .expected_wake
                .filter(|_| matches!(state, LivenessState::Waiting | LivenessState::Held))
                .map(|wake| wake.0),
            hold: hold
                .filter(|_| state == LivenessState::Held)
                .map(LivenessHold::from),
            as_of: self.as_of,
        }
    }
}

/// One statement for any number of threads. The lists and the facts that
/// determine state share a snapshot.
static LIVENESS_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let pending = crate::engine::handoff_sql::pending_handoff_rows_sql(
        "DISTINCT ON (pending_handoff.thread_id) pending_handoff.thread_id",
    );
    let opened = super::session_scope::session_opened_sql();
    let between_turns = super::event_bus::between_turns_sql("ts.thread_id");
    let newest_start = super::event_bus::newest_turn_start_sequence_sql("ts.thread_id");
    format!(
        "WITH pending AS ({pending} \
             AND pending_handoff.thread_id = ANY($1) \
             ORDER BY pending_handoff.thread_id, pending_handoff.sequence DESC) \
         SELECT ts.thread_id, ts.status, \
                ts.live_event_waits, \
                COALESCE((SELECT jsonb_agg(jsonb_build_object( \
                         'thread_id', c.thread_id, 'title', c.title, 'first_message', c.first_message) \
                         ORDER BY c.created_at, c.thread_id) \
                   FROM thread_summaries c WHERE c.parent_thread_id = ts.thread_id \
                    AND c.awaited_by_parent), '[]'::jsonb) AS awaited_children, \
                (SELECT wake.payload FROM events wake WHERE wake.thread_id = ts.thread_id \
                    AND wake.event_type = 'WakeExpected' \
                    AND wake.sequence > COALESCE({newest_start}, 0) \
                  ORDER BY wake.sequence DESC LIMIT 1) AS expected_wake, \
                (SELECT EXTRACT(EPOCH FROM now() - newest.created)::bigint \
                   FROM events newest WHERE newest.thread_id = ts.thread_id \
                  ORDER BY newest.sequence DESC LIMIT 1) AS last_activity_age_secs, \
                sessions.started AS sessions_started, \
                sessions.opened AS sessions_opened, \
                sessions.opening_kind, \
                sessions.age_secs AS session_age_secs, \
                pending.thread_id IS NOT NULL AS handoff_pending, \
                {between_turns} AS between_turns, \
                EXISTS (SELECT 1 FROM thread_queue q \
                         WHERE q.thread_id = ts.thread_id AND q.status = 'queued') AS queued, \
                ts.hold IS NOT NULL AS on_hold, \
                date_trunc('second', now()) AS as_of \
           FROM thread_summaries ts \
           LEFT JOIN pending ON pending.thread_id = ts.thread_id \
           CROSS JOIN LATERAL ( \
               SELECT count(*) AS started, \
                      count(*) FILTER (WHERE {opened}) AS opened, \
                      (array_agg(payload->'continuity'->>'kind' ORDER BY sequence DESC) \
                         FILTER (WHERE {opened}))[1] AS opening_kind, \
                      EXTRACT(EPOCH FROM now() - COALESCE( \
                          MAX(created) FILTER (WHERE {opened}), MIN(created)))::bigint AS age_secs \
                 FROM events WHERE thread_id = ts.thread_id AND event_type = 'SessionStarted' \
           ) sessions \
          WHERE ts.thread_id = ANY($1)"
    )
});

/// The liveness of each of `thread_ids` that has a summary row.
pub async fn liveness_for_threads(
    pool: &PgPool,
    thread_ids: &[Uuid],
) -> Result<HashMap<Uuid, ThreadLiveness>, sqlx::Error> {
    if thread_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<LivenessRow> = sqlx::query_as(&LIVENESS_SQL)
        .bind(thread_ids)
        .fetch_all(pool)
        .await?;
    let mut result = HashMap::new();
    for row in rows {
        // Admission accounts for the handoff's own Stop and pause exceptions.
        let successor = if row.handoff_pending {
            crate::engine::pending_handoff::successor_wake(pool, row.thread_id).await?
        } else {
            None
        };
        let facts = row.facts(successor.as_ref().map(|wake| wake.handoff.starts));
        let hold = match successor.and_then(|wake| wake.hold) {
            None => match derive(&facts) {
                LivenessState::Waiting => turn_hold(pool, row.thread_id).await?,
                // Only a hold on a thread parked on a question derives
                // `held` before the wake holds are read.
                LivenessState::Held => thread_hold(pool, row.thread_id)
                    .await?
                    .map(|hold| TurnHold::Hold(Box::new(hold))),
                _ => None,
            },
            hold => hold,
        };
        result.insert(row.thread_id, row.into_liveness(facts, hold));
    }
    Ok(result)
}

/// The liveness of `thread_id`, or `None` when it has no summary row.
pub async fn liveness_for_thread(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<ThreadLiveness>, sqlx::Error> {
    Ok(liveness_for_threads(pool, &[thread_id])
        .await?
        .remove(&thread_id))
}

/// Fill `liveness` on summary rows an agent, the CLI or a script reads, with
/// one batch query plus checks for wakes that may be held. A failed read
/// leaves the rows without liveness.
pub async fn attach<'a>(
    pool: &PgPool,
    summaries: impl IntoIterator<Item = &'a mut crate::core::store::ThreadSummary>,
) {
    let mut rows: Vec<(Uuid, &mut crate::core::store::ThreadSummary)> = summaries
        .into_iter()
        .filter_map(|s| Some((Uuid::parse_str(&s.thread_id).ok()?, s)))
        .collect();
    let ids: Vec<Uuid> = rows.iter().map(|(id, _)| *id).collect();
    match liveness_for_threads(pool, &ids).await {
        Ok(mut found) => {
            for (id, summary) in &mut rows {
                summary.liveness = found.remove(id);
            }
        }
        Err(e) => crate::log_legacy!(
            "[ThreadLiveness] reading {} threads failed: {}",
            ids.len(),
            e
        ),
    }
}

/// [`liveness_for_thread`] for a header beside a thread-scoped read. A failed
/// read logs and yields `None`, so the read itself still answers.
pub async fn header(pool: &PgPool, thread_id: Option<Uuid>) -> Option<ThreadLiveness> {
    let thread_id = thread_id?;
    liveness_for_thread(pool, thread_id)
        .await
        .unwrap_or_else(|e| {
            crate::log_legacy!("[ThreadLiveness] reading {} failed: {}", thread_id, e);
            None
        })
}

/// Returns true iff `thread_summaries.status` is currently `'running'`.
/// Best-effort gate — caller and a concurrent settler can still both pass
/// this check before either's emit lands; treat as a duplicate-reducer,
/// not a guarantee.
pub async fn thread_is_running(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await?;
    Ok(status.as_deref() == Some(crate::engine::thread_lifecycle::ThreadStatus::Running.as_str()))
}
