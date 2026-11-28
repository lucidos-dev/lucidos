use uuid::Uuid;

use super::thread_events::SessionContinuity;

const OPENING_KINDS: [&str; 2] = ["fresh", "handoff"];

pub fn session_opened_sql() -> String {
    let kinds = OPENING_KINDS.map(|k| format!("'{k}'")).join(", ");
    format!("event_type = 'SessionStarted' AND payload->'continuity'->>'kind' IN ({kinds})")
}

pub(crate) fn current_session_start_sql(thread: &str) -> String {
    format!(
        "COALESCE((SELECT MAX(o.sequence) FROM events o \
                    WHERE o.thread_id = {thread} AND {}), 0)",
        session_opened_sql()
    )
}

pub fn in_current_session_sql(thread: &str, sequence: &str) -> String {
    format!("{sequence} > {}", current_session_start_sql(thread))
}

/// INVARIANT: The ordinal of the session `thread_id` is in now, counted from 1. Read it
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
    pub fn opens_session(&self) -> bool {
        matches!(self, Self::Fresh { .. } | Self::Handoff { .. })
    }
}

#[cfg(test)]
#[path = "session_scope_tests.rs"]
mod tests;
