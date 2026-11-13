//! The *agent session* as a unit of telemetry: which session of its thread an
//! event belongs to. A thread starts in session 1. Each `SessionStarted` that
//! carried no earlier context opens the next one: a fresh session, or a
//! *successor session*. A resume stays in the session it resumed.
//!
//! Plan: `docs/plans/2026-10-23-successor-session-in-the-same-thread.md`.

use uuid::Uuid;

use super::thread_events::SessionContinuity;

/// The `continuity.kind` wire names of a `SessionStarted` that opens a new
/// session. `the_sql_names_every_kind_that_opens_a_session` pins it to
/// [`SessionContinuity::opens_session`].
const OPENING_KINDS: [&str; 2] = ["fresh", "handoff"];

/// A SQL predicate over unaliased `events` columns: a `SessionStarted` that
/// opened a new session of its thread.
fn session_opened_sql() -> String {
    let kinds = OPENING_KINDS.map(|k| format!("'{k}'")).join(", ");
    format!("event_type = 'SessionStarted' AND payload->'continuity'->>'kind' IN ({kinds})")
}

/// A SQL scalar: the sequence that opened `thread`'s current session, or 0
/// in its first. `thread` is a SQL expression, such as `$1` or `ts.thread_id`.
pub(crate) fn current_session_start_sql(thread: &str) -> String {
    format!(
        "COALESCE((SELECT MAX(o.sequence) FROM events o \
                    WHERE o.thread_id = {thread} AND {}), 0)",
        session_opened_sql()
    )
}

/// A SQL predicate: the event at `sequence`, a column of the caller's row,
/// is in `thread`'s current session.
pub(crate) fn in_current_session_sql(thread: &str, sequence: &str) -> String {
    format!("{sequence} > {}", current_session_start_sql(thread))
}

/// The ordinal of the session `thread_id` is in now, counted from 1. Read it
/// under the thread's append lock, so no session opens in between.
pub(crate) async fn current_session_ordinal(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
) -> Result<u32, sqlx::Error> {
    let opened: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND {}",
        session_opened_sql()
    ))
    .bind(thread_id)
    .fetch_one(conn)
    .await?;
    Ok(u32::try_from(opened).unwrap_or(u32::MAX).saturating_add(1))
}

impl SessionContinuity {
    /// Whether a session starting with this continuity opens a new session
    /// of its thread, rather than staying in the one it resumed. The first
    /// session opens none: a thread is in session 1 from its start.
    pub fn opens_session(&self) -> bool {
        matches!(self, Self::Fresh { .. } | Self::Handoff { .. })
    }
}

#[cfg(test)]
#[path = "session_scope_tests.rs"]
mod tests;
