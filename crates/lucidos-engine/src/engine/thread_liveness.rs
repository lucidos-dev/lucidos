//! *Thread liveness*: whether a thread is working, quiet, parked, owed a
//! session, or at rest, as the engine reports it (ADR 0679).
//!
//! [`derive`] is the one definition. SQL gathers its facts, and every surface
//! (agent tools, HTTP, SDK, CLI, the UI) reads the result rather than mapping
//! a status and timestamps itself. Reading it writes nothing.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::thread_events::SessionStart;
use super::thread_lifecycle::ThreadStatus;

/// How long a turn in flight may go without an event before it reads `quiet`.
pub const QUIET_AFTER: Duration = Duration::from_secs(10 * 60);

/// How long a pending handoff may go unstarted before it reads `stalled`.
/// The engine starts a successor within seconds of the old turn's end.
pub const HANDOFF_START_GRACE: Duration = Duration::from_secs(2 * 60);

/// A thread's liveness state. First match wins, in [`derive`]'s order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LivenessState {
    /// A turn is in flight and recently active, or a successor session is
    /// about to start.
    Working,
    /// A turn is in flight and silent for [`QUIET_AFTER`]. Not a verdict.
    Quiet,
    /// Parked on a question card.
    WaitingForUserAnswer,
    /// A version switch interrupted it, and the engine resumes it.
    Paused,
    /// The last turn failed, and nothing started since.
    Failed,
    /// The engine owes a successor session and has not started it.
    Stalled,
    /// No turn, but it wakes itself: live event waits or awaited sub-threads.
    Waiting,
    /// Nothing in flight, nothing owed.
    Idle,
}

impl LivenessState {
    /// Every state, in [`derive`]'s order. Feeds the generated TS union.
    pub const ALL: [Self; 8] = [
        Self::Working,
        Self::Quiet,
        Self::WaitingForUserAnswer,
        Self::Paused,
        Self::Failed,
        Self::Stalled,
        Self::Waiting,
        Self::Idle,
    ];

    /// Adding a variant breaks this match. Add it to [`Self::ALL`] too.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Quiet => "quiet",
            Self::WaitingForUserAnswer => "waiting_for_user_answer",
            Self::Paused => "paused",
            Self::Failed => "failed",
            Self::Stalled => "stalled",
            Self::Waiting => "waiting",
            Self::Idle => "idle",
        }
    }
}

/// What [`derive`] decides from. Ages come from the database clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivenessFacts {
    pub status: ThreadStatus,
    /// Seconds since the thread's newest event. `None` for a thread with none.
    pub last_activity_age_secs: Option<i64>,
    /// When the pending handoff's session starts, if one is pending.
    pub pending_handoff: Option<SessionStart>,
    /// Live event waits or awaited sub-threads.
    pub waits: bool,
}

fn at_least(age_secs: Option<i64>, limit: Duration) -> bool {
    age_secs.is_some_and(|age| age >= limit.as_secs() as i64)
}

/// The one liveness definition. A next-input handoff owes nothing until an
/// input arrives, so it never makes a thread `stalled`. The grace counts from
/// the newest event, so it starts when the turn that handed off ended.
pub fn derive(facts: &LivenessFacts) -> LivenessState {
    match facts.status {
        ThreadStatus::Running if at_least(facts.last_activity_age_secs, QUIET_AFTER) => {
            LivenessState::Quiet
        }
        ThreadStatus::Running => LivenessState::Working,
        ThreadStatus::WaitingForUserAnswer => LivenessState::WaitingForUserAnswer,
        ThreadStatus::Paused => LivenessState::Paused,
        ThreadStatus::Failed => LivenessState::Failed,
        ThreadStatus::Idle | ThreadStatus::Waiting => match facts.pending_handoff {
            Some(starts) if starts.is_unprompted() => {
                if at_least(facts.last_activity_age_secs, HANDOFF_START_GRACE) {
                    LivenessState::Stalled
                } else {
                    LivenessState::Working
                }
            }
            _ if facts.waits => LivenessState::Waiting,
            _ => LivenessState::Idle,
        },
    }
}

/// How the thread's current agent session began.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOpening {
    First,
    Fresh,
    Handoff,
}

/// The thread's current agent session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LivenessSession {
    /// Counted from 1, like `session_scope::current_session_ordinal`.
    pub ordinal: u32,
    pub continuity: SessionOpening,
    pub age_secs: i64,
}

/// Present only while a handoff is pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LivenessHandoff {
    Pending,
}

/// The wire object every surface carries, about 200 bytes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreadLiveness {
    pub state: LivenessState,
    pub status: ThreadStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_age_secs: Option<i64>,
    /// Absent on a thread with no agent session, such as a chat thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<LivenessSession>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handoff: Option<LivenessHandoff>,
    /// The database's `now()`, to the second, so a later turn can tell how old
    /// the observation is.
    pub as_of: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow)]
struct LivenessRow {
    thread_id: Uuid,
    status: String,
    waits: bool,
    last_activity_age_secs: Option<i64>,
    sessions_started: i64,
    sessions_opened: i64,
    opening_kind: Option<String>,
    session_age_secs: Option<i64>,
    handoff_pending: bool,
    as_of: chrono::DateTime<chrono::Utc>,
}

impl LivenessRow {
    fn into_liveness(self, pending_handoff: Option<SessionStart>) -> ThreadLiveness {
        let facts = LivenessFacts {
            status: ThreadStatus::parse(&self.status),
            last_activity_age_secs: self.last_activity_age_secs,
            pending_handoff,
            waits: self.waits,
        };
        let continuity = match self.opening_kind.as_deref() {
            Some("handoff") => SessionOpening::Handoff,
            Some(_) => SessionOpening::Fresh,
            None => SessionOpening::First,
        };
        let ordinal = u32::try_from(self.sessions_opened)
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        ThreadLiveness {
            state: derive(&facts),
            status: facts.status,
            last_activity_age_secs: self.last_activity_age_secs,
            session: (self.sessions_started > 0).then(|| LivenessSession {
                ordinal,
                continuity,
                age_secs: self.session_age_secs.unwrap_or(0),
            }),
            handoff: self.handoff_pending.then_some(LivenessHandoff::Pending),
            as_of: self.as_of,
        }
    }
}

/// One statement for any number of threads. Per thread it costs one probe
/// for the newest event and one partial-index scan of its `SessionStarted`
/// rows. Pending handoffs come from the index-backed set, narrowed to `$1`.
static LIVENESS_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let pending = super::successor::pending_handoff_rows_sql(
        "DISTINCT ON (pending_handoff.thread_id) pending_handoff.thread_id",
    );
    let opened = super::session_scope::session_opened_sql();
    format!(
        "WITH pending AS ({pending} \
             AND pending_handoff.thread_id = ANY($1) \
             ORDER BY pending_handoff.thread_id, pending_handoff.sequence DESC) \
         SELECT ts.thread_id, ts.status, \
                (ts.live_event_wait_count > 0 OR ts.awaited_children_count > 0) AS waits, \
                (SELECT EXTRACT(EPOCH FROM now() - newest.created)::bigint \
                   FROM events newest WHERE newest.thread_id = ts.thread_id \
                  ORDER BY newest.sequence DESC LIMIT 1) AS last_activity_age_secs, \
                sessions.started AS sessions_started, \
                sessions.opened AS sessions_opened, \
                sessions.opening_kind, \
                sessions.age_secs AS session_age_secs, \
                pending.thread_id IS NOT NULL AS handoff_pending, \
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
        // Eligibility belongs to the start path, including its closed-lane
        // guard. A pending handoff alone does not mean a session is owed.
        let owed = if row.handoff_pending {
            super::successor::successor_to_start(pool, row.thread_id)
                .await?
                .map(|handoff| handoff.starts)
        } else {
            None
        };
        result.insert(row.thread_id, row.into_liveness(owed));
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
/// one batch query plus eligibility checks for pending handoffs. A failed
/// read leaves the rows without it:
/// an absent header claims nothing.
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
        Err(e) => crate::log!(
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
            crate::log!("[ThreadLiveness] reading {} failed: {}", thread_id, e);
            None
        })
}

#[cfg(test)]
#[path = "thread_liveness_tests.rs"]
mod tests;
