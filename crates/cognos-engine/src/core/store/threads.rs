use super::messages::build_session_messages;
use super::types::SessionMessage;
use super::EventStore;
use crate::core::EventRow;
use crate::engine::thread_lifecycle::ThreadStatus;
use serde::Serialize;

/// Two-state initiator stored in the `thread_summaries.initiator` text column
/// and exposed on `ThreadInfo` for the frontend (`'user' | 'system'`).
///
/// This is intentionally separate from `ActorMode` (Human / Agent / Engine):
/// the column has only ever held the binary user-vs-system distinction (the
/// frontend renders a "system" badge on rows where `initiator == "system"`).
/// The mapping at the wire boundary is `Human → user`, `Agent | Engine → system`.
/// Promoting the column to a tri-state would require a DB migration and a
/// frontend type change; defer until there is a UI need.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LegacyInitiator {
    User,
    System,
}

impl LegacyInitiator {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::System => "system",
        }
    }

    /// Parse the legacy DB column value. Fails loud on unknown strings (per the
    /// "no silent defaults" rule in CLAUDE.md) — a row with anything other than
    /// `"user"` or `"system"` indicates corrupted state we want surfaced rather
    /// than masked.
    pub fn from_db_str(s: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        match s {
            "user" => Ok(Self::User),
            "system" => Ok(Self::System),
            other => Err(format!(
                "thread_summaries.initiator has unexpected value '{}' (expected 'user' or 'system')",
                other
            )
            .into()),
        }
    }
}

/// Format a display title from optional title and first_message fields.
/// Falls back to truncated first_message if title is None.
fn format_display_title(title: Option<String>, first_message: Option<String>) -> String {
    title.unwrap_or_else(|| {
        let msg = first_message.unwrap_or_default();
        if msg.chars().count() > 40 {
            format!("{}...", msg.chars().take(37).collect::<String>())
        } else {
            msg
        }
    })
}

/// Summary info about an active thread.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadInfo {
    pub thread_id: String,
    pub title: String,
    pub channel: String,
    pub initiator: LegacyInitiator,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_activity: chrono::DateTime<chrono::Utc>,
    pub message_count: i64,
    /// Thread section: "default" (history/pinned), "unread" (needs user attention).
    pub section: String,
    /// Number of child threads still active (non-zero means parent is "ongoing").
    pub active_children_count: i64,
    /// Total number of child threads (active + completed).
    pub total_children_count: i64,
    /// Thread status: "idle", "running", or "waiting". Computed by the backend.
    pub status: String,
    /// Whether the CC session has proposed changes.
    pub cc_has_changes: bool,
    /// Whether the proposed changes require an engine restart.
    pub cc_requires_restart: bool,
    /// Whether the CC session is working on an external repo.
    pub cc_is_external_repo: bool,
    /// Whether a merge conflict is being resolved.
    pub cc_applying: bool,
    /// When the thread last entered the 'running' state (for IN PROGRESS sort order).
    pub last_revived_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Parent thread that spawned this one (for sub-thread navigation).
    pub parent_thread_id: Option<String>,
    /// Cached title of the parent thread — saves an extra round-trip when the
    /// route panel renders "Parent thread · <title>" links.
    pub parent_thread_title: Option<String>,
}

/// Row type for thread summary queries — all columns selected from thread_summaries.
/// Uses a named struct because sqlx only implements FromRow for tuples up to 16 elements.
#[derive(sqlx::FromRow)]
struct ThreadRow {
    thread_id: String,
    title: Option<String>,
    first_message: Option<String>,
    source: String,
    initiator: String,
    created_at: chrono::DateTime<chrono::Utc>,
    last_activity: chrono::DateTime<chrono::Utc>,
    message_count: i64,
    section: String,
    active_children_count: i64,
    total_children_count: i64,
    status: String,
    cc_has_changes: bool,
    cc_requires_restart: bool,
    cc_is_external_repo: bool,
    cc_applying: bool,
    last_revived_at: Option<chrono::DateTime<chrono::Utc>>,
    parent_thread_id: Option<String>,
    parent_thread_title: Option<String>,
}

/// SQL column list for thread summary queries. Callers MUST alias the outer
/// FROM as `t` (e.g. `FROM thread_summaries t` or `FROM (...) t`) — the
/// correlated subquery for `parent_thread_title` references `t.parent_thread_id`.
///
/// `parent_thread_title` is a correlated subquery on `thread_summaries.thread_id`
/// (the PK). For rows without a parent, `parent_thread_id IS NULL` and the
/// subquery returns NULL without scanning. For rows with a parent, the lookup
/// is a single PK index hit.
const THREAD_COLS: &str =
    "t.thread_id::text, t.title, t.first_message, t.source, t.initiator, t.created_at, t.last_activity, \
    t.message_count::bigint, t.section, t.active_children_count::bigint, t.total_children_count::bigint, \
    t.status, t.cc_has_changes, t.cc_requires_restart, t.cc_is_external_repo, t.cc_applying, t.last_revived_at, \
    t.parent_thread_id::text AS parent_thread_id, \
    (SELECT p.title FROM thread_summaries p WHERE p.thread_id = t.parent_thread_id) AS parent_thread_title";

impl EventStore {
    /// Get pinned threads from the projection table.
    pub async fn get_pinned_threads(
        &self,
    ) -> Result<Vec<ThreadInfo>, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "SELECT {} FROM thread_summaries t WHERE t.is_pinned = TRUE ORDER BY t.last_activity DESC",
            THREAD_COLS,
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_infos(rows)
    }

    /// Get recent threads for the History section — returns up to `per_source` threads
    /// for each source type, ensuring every category is pre-loaded for instant filter switching.
    /// Includes active-status threads (`running`, `waiting_for_user_answer`) even without a
    /// response yet — a thread the user just started or that's blocked on user input should
    /// appear immediately in the drawer, before any response arrives.
    pub async fn get_recent_threads(
        &self,
        per_source: i64,
    ) -> Result<Vec<ThreadInfo>, Box<dyn std::error::Error + Send + Sync>> {
        let active_statuses: &[&str] = &[
            ThreadStatus::Running.as_str(),
            ThreadStatus::WaitingForUserAnswer.as_str(),
        ];
        let sql = format!(
            "SELECT {} FROM (\
                SELECT *, ROW_NUMBER() OVER (PARTITION BY source ORDER BY last_activity DESC) AS rn \
                FROM thread_summaries WHERE has_response = TRUE OR status = ANY($1)\
            ) t WHERE t.rn <= $2 ORDER BY t.last_activity DESC",
            THREAD_COLS,
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(active_statuses)
            .bind(per_source)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_infos(rows)
    }

    /// Get older threads for infinite scroll pagination.
    /// When `sources` is provided, only threads matching one of the given sources are returned.
    pub async fn get_older_threads(
        &self,
        before: chrono::DateTime<chrono::Utc>,
        limit: i64,
        sources: Option<&[String]>,
    ) -> Result<Vec<ThreadInfo>, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "SELECT {} FROM thread_summaries t \
             WHERE t.has_response = TRUE AND t.last_activity < $1 \
             AND ($2::text[] IS NULL OR t.source = ANY($2)) \
             ORDER BY t.last_activity DESC LIMIT $3",
            THREAD_COLS,
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(before)
            .bind(sources)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_infos(rows)
    }

    /// Look up a thread's generated title from the projection. Returns None
    /// when the thread doesn't exist or hasn't been titled yet.
    pub async fn get_thread_title(
        &self,
        thread_id: uuid::Uuid,
    ) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT title FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(t,)| t))
    }

    /// Check if a thread already has a generated title.
    pub async fn thread_has_title(
        &self,
        thread_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let thread_uuid = uuid::Uuid::parse_str(thread_id)?;
        Ok(self.get_thread_title(thread_uuid).await?.is_some())
    }

    fn rows_to_thread_infos(
        rows: Vec<ThreadRow>,
    ) -> Result<Vec<ThreadInfo>, Box<dyn std::error::Error + Send + Sync>> {
        rows.into_iter()
            .map(|r| {
                Ok(ThreadInfo {
                    thread_id: r.thread_id,
                    title: format_display_title(r.title, r.first_message),
                    channel: r.source,
                    initiator: LegacyInitiator::from_db_str(r.initiator.as_str())?,
                    created_at: r.created_at,
                    last_activity: r.last_activity,
                    message_count: r.message_count,
                    section: r.section,
                    active_children_count: r.active_children_count,
                    total_children_count: r.total_children_count,
                    status: r.status,
                    cc_has_changes: r.cc_has_changes,
                    cc_requires_restart: r.cc_requires_restart,
                    cc_is_external_repo: r.cc_is_external_repo,
                    cc_applying: r.cc_applying,
                    last_revived_at: r.last_revived_at,
                    parent_thread_id: r.parent_thread_id,
                    parent_thread_title: r.parent_thread_title,
                })
            })
            .collect()
    }

    /// Get thread info for specific thread IDs (used for active threads).
    pub async fn get_threads_by_ids(
        &self,
        thread_ids: &[String],
    ) -> Result<Vec<ThreadInfo>, Box<dyn std::error::Error + Send + Sync>> {
        if thread_ids.is_empty() {
            return Ok(vec![]);
        }

        let uuids: Vec<uuid::Uuid> = thread_ids
            .iter()
            .filter_map(|s| uuid::Uuid::parse_str(s).ok())
            .collect();

        let sql = format!(
            "SELECT {} FROM thread_summaries t WHERE t.thread_id = ANY($1::uuid[])",
            THREAD_COLS,
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(&uuids)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_infos(rows)
    }

    /// Get all events for a specific thread, ordered chronologically.
    pub async fn get_thread_events(
        &self,
        thread_id: &str,
    ) -> Result<Vec<EventRow>, Box<dyn std::error::Error + Send + Sync>> {
        let thread_uuid = uuid::Uuid::parse_str(thread_id)?;
        let events = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT id, event_type, payload, created, thread_id, sequence
            FROM events
            WHERE thread_id = $1
            ORDER BY created ASC
            "#,
        )
        .bind(thread_uuid)
        .fetch_all(&self.pool)
        .await?;

        Ok(events)
    }

    /// Get the content of the first user message in a thread (for title generation).
    /// Returns (text, image_description) for the first user message in a thread.
    pub async fn get_thread_first_message(
        &self,
        thread_id: &str,
    ) -> Result<Option<(String, Option<String>)>, Box<dyn std::error::Error + Send + Sync>> {
        let thread_uuid = uuid::Uuid::parse_str(thread_id)?;
        let row = sqlx::query_as::<_, (serde_json::Value,)>(
            r#"
            SELECT payload
            FROM events
            WHERE event_type = 'MessageReceived'
              AND thread_id = $1
            ORDER BY created ASC
            LIMIT 1
            "#,
        )
        .bind(thread_uuid)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.and_then(|(payload,)| {
            let text = payload
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("content").and_then(|v| v.as_str()))?;
            let image_desc = payload
                .get("image_description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Some((text.to_string(), image_desc))
        }))
    }

    /// Returns recent `MessageReceived` / `ResponseGenerated` events from a thread,
    /// formatted as oldest-first labeled lines for use as Gemini extraction context.
    ///
    /// Each line is `"User: <text>"` or `"Assistant: <text>"`. Lines are capped at
    /// 500 chars to avoid blowing up the system prompt for long messages. Returns
    /// empty string if the thread has no such events. The `exclude_event_id` is
    /// for the live consumer / rebuild path: skip the event being extracted so it
    /// doesn't appear in its own context.
    pub async fn recent_thread_messages_for_extraction(
        &self,
        thread_id: uuid::Uuid,
        limit: i64,
        exclude_event_id: Option<uuid::Uuid>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
            r#"
            SELECT event_type,
                   payload->>'text' AS text,
                   payload->>'content' AS content
            FROM events
            WHERE thread_id = $1
              AND event_type IN ('MessageReceived', 'ResponseGenerated')
              AND ($3::uuid IS NULL OR id <> $3)
            ORDER BY created DESC
            LIMIT $2
            "#,
        )
        .bind(thread_id)
        .bind(limit)
        .bind(exclude_event_id)
        .fetch_all(&self.pool)
        .await?;

        let lines: Vec<String> = rows
            .into_iter()
            .rev()
            .filter_map(|(event_type, text, content)| {
                let body = text.or(content)?;
                if body.trim().is_empty() {
                    return None;
                }
                let trimmed: String = body.chars().take(500).collect();
                let role = match event_type.as_str() {
                    "MessageReceived" => "User",
                    "ResponseGenerated" => "Assistant",
                    _ => return None,
                };
                Some(format!("{}: {}", role, trimmed))
            })
            .collect();

        Ok(lines.join("\n"))
    }

    /// Get all messages for a specific thread, built from its events.
    pub async fn get_thread_messages(
        &self,
        thread_id: &str,
    ) -> Result<Vec<SessionMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let events = self.get_thread_events(thread_id).await?;
        Ok(build_session_messages(&events))
    }

    /// Get timeline events for a thread (session lifecycle + change actions).
    pub async fn get_thread_timeline_events(
        &self,
        thread_id: &str,
    ) -> Result<Vec<ThreadTimelineEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let thread_uuid = uuid::Uuid::parse_str(thread_id)?;
        let rows = sqlx::query_as::<_, (String, chrono::DateTime<chrono::Utc>, serde_json::Value)>(
            r#"
            SELECT event_type, created, payload
            FROM events
            WHERE thread_id = $1
              AND event_type IN ('SessionStarted', 'SessionEnded', 'ChangeApplied', 'ChangeDiscarded', 'ChangeReverted')
            ORDER BY created ASC
            "#,
        )
        .bind(thread_uuid)
        .fetch_all(&self.pool)
        .await?;

        let mut result = Vec::new();
        for (event_type, created, payload) in rows {
            let change_id = payload
                .get("change_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // For change events, look up the description from the changes table
            let description = if let Some(ref cid) = change_id {
                if let Ok(uuid) = uuid::Uuid::parse_str(cid) {
                    crate::core::changes::get_by_id(&self.pool, uuid)
                        .await
                        .ok()
                        .flatten()
                        .map(|c| {
                            c.description
                                .lines()
                                .next()
                                .unwrap_or(&c.description)
                                .to_string()
                        })
                } else {
                    None
                }
            } else {
                None
            };

            // Convert PascalCase to snake_case for frontend
            let snake_type = match event_type.as_str() {
                "SessionStarted" => "session_started",
                "SessionEnded" => "session_ended",
                "ChangeApplied" => "change_applied",
                "ChangeDiscarded" => "change_discarded",
                "ChangeReverted" => "change_reverted",
                _ => continue,
            };

            result.push(ThreadTimelineEvent {
                event_type: snake_type.to_string(),
                timestamp: created.to_rfc3339(),
                description,
                change_id,
            });
        }
        Ok(result)
    }
}

/// Search result for a thread, with a relevance score.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadSearchResult {
    #[serde(flatten)]
    pub info: ThreadInfo,
    pub score: f64,
}

impl EventStore {
    /// Search threads by text query (ILIKE on titles and message content).
    /// Multi-token queries match per-token: every whitespace-separated token must
    /// appear (case-insensitive) somewhere in the thread — title or any event
    /// payload — but they need not appear together as a phrase. Title-only
    /// matches score 1.0; content matches score 0.7.
    pub async fn search_threads_by_text(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<ThreadSearchResult>, Box<dyn std::error::Error + Send + Sync>> {
        // Escape LIKE metacharacters (\ % _) so a token like "50%" matches the
        // literal substring, not "anything containing 50". Backslash is LIKE's
        // default escape character, so no ESCAPE clause is needed.
        let escape_like = |t: &str| {
            t.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        };
        let patterns: Vec<String> = query
            .split_whitespace()
            .map(|t| format!("%{}%", escape_like(t)))
            .collect();
        if patterns.is_empty() {
            return Ok(vec![]);
        }
        let token_count = patterns.len() as i64;

        /// SQL column list prefixed with table alias for joins.
        const THREAD_COLS_PREFIXED: &str = "s.thread_id::text, s.title, s.first_message, s.source, s.initiator, s.created_at, s.last_activity, \
            s.message_count::bigint, s.section, s.active_children_count::bigint, s.total_children_count::bigint, \
            s.status, s.cc_has_changes, s.cc_requires_restart, s.cc_is_external_repo, s.cc_applying, s.last_revived_at, \
            s.parent_thread_id::text AS parent_thread_id, \
            (SELECT p.title FROM thread_summaries p WHERE p.thread_id = s.parent_thread_id) AS parent_thread_title";

        let sql = format!(
            "WITH title_matches AS (\
                SELECT thread_id, 1.0::float8 AS match_score FROM thread_summaries WHERE title ILIKE ALL($1::text[])\
            ), content_matches AS (\
                SELECT m.thread_id, 0.7::float8 AS match_score FROM (\
                    SELECT e.thread_id, t.pattern \
                    FROM events e CROSS JOIN unnest($1::text[]) AS t(pattern) \
                    WHERE e.event_type IN ('MessageReceived', 'ResponseGenerated') \
                      AND e.thread_id IS NOT NULL \
                      AND (e.payload->>'text' ILIKE t.pattern OR e.payload->>'content' ILIKE t.pattern)\
                ) m \
                GROUP BY m.thread_id \
                HAVING COUNT(DISTINCT m.pattern) = $3\
            ), entity_matches AS (\
                /* Drive from memory_entries (the small side) and join into events; \
                   joining the other way produces a ~58M-row nested loop on personal. */\
                SELECT m.thread_id, 0.7::float8 AS match_score FROM (\
                    SELECT e.thread_id, t.pattern \
                    FROM memory_entries me \
                    JOIN events e ON e.id::text = me.source->>'id' \
                    CROSS JOIN unnest($1::text[]) AS t(pattern) \
                    WHERE me.source->>'type' = 'event' \
                      AND e.event_type IN ('MessageReceived', 'ResponseGenerated') \
                      AND e.thread_id IS NOT NULL \
                      AND EXISTS (\
                          SELECT 1 FROM jsonb_array_elements_text(me.entities) AS ent \
                          WHERE ent ILIKE t.pattern\
                      )\
                ) m \
                GROUP BY m.thread_id \
                HAVING COUNT(DISTINCT m.pattern) = $3\
            ), scored AS (\
                SELECT thread_id, match_score FROM title_matches \
                UNION ALL \
                SELECT thread_id, match_score FROM content_matches \
                UNION ALL \
                SELECT thread_id, match_score FROM entity_matches\
            ), best_scores AS (\
                SELECT thread_id, MAX(match_score) AS score FROM scored GROUP BY thread_id\
            ) \
            SELECT {}, b.score \
            FROM best_scores b JOIN thread_summaries s ON s.thread_id = b.thread_id \
            ORDER BY b.score DESC, s.last_activity DESC LIMIT $2",
            THREAD_COLS_PREFIXED,
        );

        // Row type: ThreadRow fields + score. Struct needed because >16 columns exceeds sqlx tuple limit.
        #[derive(sqlx::FromRow)]
        struct SearchRow {
            thread_id: String,
            title: Option<String>,
            first_message: Option<String>,
            source: String,
            initiator: String,
            created_at: chrono::DateTime<chrono::Utc>,
            last_activity: chrono::DateTime<chrono::Utc>,
            message_count: i64,
            section: String,
            active_children_count: i64,
            total_children_count: i64,
            status: String,
            cc_has_changes: bool,
            cc_requires_restart: bool,
            cc_is_external_repo: bool,
            cc_applying: bool,
            last_revived_at: Option<chrono::DateTime<chrono::Utc>>,
            parent_thread_id: Option<String>,
            parent_thread_title: Option<String>,
            score: f64,
        }

        let rows = sqlx::query_as::<_, SearchRow>(&sql)
            .bind(&patterns)
            .bind(limit)
            .bind(token_count)
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter()
            .map(|r| {
                Ok(ThreadSearchResult {
                    info: ThreadInfo {
                        thread_id: r.thread_id,
                        title: format_display_title(r.title, r.first_message),
                        channel: r.source,
                        initiator: LegacyInitiator::from_db_str(r.initiator.as_str())?,
                        created_at: r.created_at,
                        last_activity: r.last_activity,
                        message_count: r.message_count,
                        section: r.section,
                        active_children_count: r.active_children_count,
                        total_children_count: r.total_children_count,
                        status: r.status,
                        cc_has_changes: r.cc_has_changes,
                        cc_requires_restart: r.cc_requires_restart,
                        cc_is_external_repo: r.cc_is_external_repo,
                        cc_applying: r.cc_applying,
                        last_revived_at: r.last_revived_at,
                        parent_thread_id: r.parent_thread_id,
                        parent_thread_title: r.parent_thread_title,
                    },
                    score: r.score,
                })
            })
            .collect()
    }

    /// Search threads semantically using memory_entries vector search.
    /// Accepts event IDs paired with their similarity scores.
    pub async fn search_threads_by_memory(
        &self,
        scored_event_ids: &[(uuid::Uuid, f64)],
        limit: i64,
    ) -> Result<Vec<ThreadSearchResult>, Box<dyn std::error::Error + Send + Sync>> {
        if scored_event_ids.is_empty() {
            return Ok(vec![]);
        }

        let event_ids: Vec<uuid::Uuid> = scored_event_ids.iter().map(|(id, _)| *id).collect();

        // Build a map of event_id → best similarity score
        let mut event_scores: std::collections::HashMap<uuid::Uuid, f64> =
            std::collections::HashMap::new();
        for (id, score) in scored_event_ids {
            let entry = event_scores.entry(*id).or_insert(0.0);
            if *score > *entry {
                *entry = *score;
            }
        }

        // Find which thread each event belongs to
        let thread_event_rows = sqlx::query_as::<_, (String, uuid::Uuid)>(
            r#"
            SELECT DISTINCT thread_id::text, id
            FROM events
            WHERE id = ANY($1::uuid[])
              AND thread_id IS NOT NULL
            "#,
        )
        .bind(&event_ids)
        .fetch_all(&self.pool)
        .await?;

        // Build thread_id → best score
        let mut thread_scores: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        for (thread_id, event_id) in &thread_event_rows {
            if let Some(&score) = event_scores.get(event_id) {
                let entry = thread_scores.entry(thread_id.clone()).or_insert(0.0);
                if score > *entry {
                    *entry = score;
                }
            }
        }

        let thread_uuids: Vec<uuid::Uuid> = thread_scores
            .keys()
            .filter_map(|id| uuid::Uuid::parse_str(id).ok())
            .collect();

        let sql = format!(
            "SELECT {} FROM thread_summaries t WHERE t.thread_id = ANY($1::uuid[]) LIMIT $2",
            THREAD_COLS,
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(&thread_uuids)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        let infos = Self::rows_to_thread_infos(rows)?;
        let mut results: Vec<ThreadSearchResult> = infos
            .into_iter()
            .map(|info| {
                let score = thread_scores.get(&info.thread_id).copied().unwrap_or(0.5);
                ThreadSearchResult { info, score }
            })
            .collect();
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(results)
    }
}

/// A timeline event rendered inline in a thread view.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadTimelineEvent {
    pub event_type: String,
    pub timestamp: String,
    pub description: Option<String>,
    pub change_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use sqlx::PgPool;
    use uuid::Uuid;

    /// Insert a parent thread and a child thread referencing it. Returns
    /// `(parent_id, child_id)`. `child_pinned` controls whether the child
    /// is_pinned (the parent is never pinned). Both have has_response=TRUE
    /// so they show up in get_recent_threads / get_older_threads.
    async fn insert_parent_child(pool: &PgPool, child_pinned: bool) -> (Uuid, Uuid) {
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_pinned, parent_thread_id) \
             VALUES ($1, 'Parent thread', 'chat', 1, NOW(), TRUE, FALSE, NULL), \
                    ($2, 'Child thread',  'chat', 1, NOW(), TRUE, $3,   $1)"
        )
        .bind(parent)
        .bind(child)
        .bind(child_pinned)
        .execute(pool)
        .await
        .expect("insert thread_summaries");
        (parent, child)
    }

    #[tokio::test]
    async fn get_pinned_threads_resolves_parent_title() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());
        let (parent, child) = insert_parent_child(&pool, true).await;

        let pinned = store
            .get_pinned_threads()
            .await
            .expect("get_pinned_threads");

        let row = pinned
            .iter()
            .find(|t| t.thread_id == child.to_string())
            .expect("child thread should appear in pinned");
        assert_eq!(
            row.parent_thread_id.as_deref(),
            Some(parent.to_string().as_str())
        );
        assert_eq!(row.parent_thread_title.as_deref(), Some("Parent thread"));

        teardown_test_db(&db).await;
    }

    /// Regression test for fe5212ea: `get_recent_threads` wraps thread_summaries
    /// in a derived table, so the parent_thread_title subquery must reference
    /// the outer alias `t`, not the inner table name. Pre-fix code aliased the
    /// outer as `ranked` but the subquery hardcoded `thread_summaries`, and
    /// /api/threads 500'd with "invalid reference to FROM-clause entry".
    #[tokio::test]
    async fn get_recent_threads_resolves_parent_title() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());
        let (_parent, child) = insert_parent_child(&pool, false).await;

        let recent = store
            .get_recent_threads(10)
            .await
            .expect("get_recent_threads");

        let row = recent
            .iter()
            .find(|t| t.thread_id == child.to_string())
            .expect("child thread should appear in recent");
        assert_eq!(row.parent_thread_title.as_deref(), Some("Parent thread"));

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn get_older_threads_resolves_parent_title() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());
        let (_parent, child) = insert_parent_child(&pool, false).await;

        let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
        let older = store
            .get_older_threads(cutoff, 10, None)
            .await
            .expect("get_older_threads");

        let row = older
            .iter()
            .find(|t| t.thread_id == child.to_string())
            .expect("child thread should appear in older");
        assert_eq!(row.parent_thread_title.as_deref(), Some("Parent thread"));

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn get_threads_by_ids_resolves_parent_title() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());
        let (_parent, child) = insert_parent_child(&pool, false).await;

        let infos = store
            .get_threads_by_ids(&[child.to_string()])
            .await
            .expect("get_threads_by_ids");

        assert_eq!(infos.len(), 1);
        assert_eq!(
            infos[0].parent_thread_title.as_deref(),
            Some("Parent thread")
        );

        teardown_test_db(&db).await;
    }

    async fn insert_thread(pool: &PgPool, id: Uuid, title: &str) {
        sqlx::query(
            "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response) \
             VALUES ($1, $2, 'chat', 0, NOW(), TRUE)"
        )
        .bind(id)
        .bind(title)
        .execute(pool)
        .await
        .expect("insert thread_summaries");
    }

    async fn insert_message(pool: &PgPool, thread_id: Uuid, event_type: &str, text: &str) {
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES ($1, $2, $3, $4, 'thread', $4::text)",
        )
        .bind(Uuid::new_v4())
        .bind(event_type)
        .bind(serde_json::json!({ "text": text }))
        .bind(thread_id)
        .execute(pool)
        .await
        .expect("insert event");
    }

    /// `memory_entries` is created by `PgVectorIndex::new` rather than a migration,
    /// so any test that joins it must initialize the index first.
    async fn ensure_memory_entries_table(pool: &PgPool) {
        crate::memory::pgvector::PgVectorIndex::new(pool.clone())
            .await
            .expect("init pgvector schema");
    }

    /// Multi-token text queries must match threads where every token appears
    /// somewhere — title or any event payload — even if no single string contains
    /// the full phrase.
    #[tokio::test]
    async fn search_threads_by_text_matches_per_token_across_events() {
        let (pool, db) = setup_test_db().await;
        ensure_memory_entries_table(&pool).await;
        let store = EventStore::new(pool.clone());

        let phrase_in_title = Uuid::new_v4();
        insert_thread(&pool, phrase_in_title, "Pappa øyeoperasjon").await;

        let split_across_events = Uuid::new_v4();
        insert_thread(&pool, split_across_events, "Fars fødselsnummer").await;
        insert_message(&pool, split_across_events, "MessageReceived", "pappas fødselsnr").await;
        insert_message(
            &pool,
            split_across_events,
            "MessageReceived",
            "tlf vestre viken sykehus, øyeavdeling",
        )
        .await;

        let only_one_token = Uuid::new_v4();
        insert_thread(&pool, only_one_token, "Pappa Losjeplassen dokumenter").await;
        insert_message(&pool, only_one_token, "MessageReceived", "fant fram dokumentene").await;

        let irrelevant = Uuid::new_v4();
        insert_thread(&pool, irrelevant, "Varmepumpe logging").await;
        insert_message(&pool, irrelevant, "MessageReceived", "varmepumpe styring").await;

        let results = store
            .search_threads_by_text("pappa øye", 20)
            .await
            .expect("search");

        let ids: Vec<&str> = results.iter().map(|r| r.info.thread_id.as_str()).collect();
        let phrase = phrase_in_title.to_string();
        let split = split_across_events.to_string();
        let one = only_one_token.to_string();
        let bad = irrelevant.to_string();

        assert!(
            ids.contains(&phrase.as_str()),
            "thread with both tokens in title must match. ids={:?}",
            ids
        );
        assert!(
            ids.contains(&split.as_str()),
            "thread with tokens split across separate events must match. ids={:?}",
            ids
        );
        assert!(
            !ids.contains(&one.as_str()),
            "thread missing the 'øye' token must NOT match. ids={:?}",
            ids
        );
        assert!(
            !ids.contains(&bad.as_str()),
            "thread with neither token must NOT match. ids={:?}",
            ids
        );

        let phrase_pos = ids.iter().position(|id| *id == phrase.as_str()).unwrap();
        let split_pos = ids.iter().position(|id| *id == split.as_str()).unwrap();
        assert!(
            phrase_pos < split_pos,
            "title-token match must rank above content-token match. ids={:?}",
            ids
        );

        teardown_test_db(&db).await;
    }

    /// LIKE metacharacters in the query must be escaped, otherwise a token like
    /// `foo_bar` matches `fooXbar` and `50%` matches everything starting with 50.
    #[tokio::test]
    async fn search_threads_by_text_treats_wildcards_as_literals() {
        let (pool, db) = setup_test_db().await;
        ensure_memory_entries_table(&pool).await;
        let store = EventStore::new(pool.clone());

        let literal_match = Uuid::new_v4();
        insert_thread(&pool, literal_match, "foo_bar exact title").await;

        let wildcard_trap = Uuid::new_v4();
        insert_thread(&pool, wildcard_trap, "fooXbar should not match").await;

        let results = store
            .search_threads_by_text("foo_bar", 20)
            .await
            .expect("search");
        let ids: Vec<&str> = results.iter().map(|r| r.info.thread_id.as_str()).collect();
        let literal = literal_match.to_string();
        let trap = wildcard_trap.to_string();
        assert!(ids.contains(&literal.as_str()), "literal match required");
        assert!(
            !ids.contains(&trap.as_str()),
            "underscore must not act as a wildcard. ids={:?}",
            ids
        );

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn recent_thread_messages_for_extraction_returns_oldest_first() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let thread = Uuid::new_v4();
        insert_thread(&pool, thread, "Test").await;
        insert_message(&pool, thread, "MessageReceived", "fødselsnr pappa").await;
        insert_message(&pool, thread, "ResponseGenerated", "Alf Tiller (pappa)").await;
        insert_message(&pool, thread, "MessageReceived", "tlf til øyeavdelingen").await;

        let ctx = store
            .recent_thread_messages_for_extraction(thread, 5, None)
            .await
            .expect("get context");

        assert!(ctx.contains("fødselsnr pappa"), "ctx={}", ctx);
        assert!(ctx.contains("Alf Tiller (pappa)"), "ctx={}", ctx);
        let pappa_pos = ctx.find("fødselsnr pappa").unwrap();
        let alf_pos = ctx.find("Alf Tiller").unwrap();
        assert!(pappa_pos < alf_pos, "oldest first; ctx={}", ctx);

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn recent_thread_messages_for_extraction_empty_thread() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let thread = Uuid::new_v4();
        insert_thread(&pool, thread, "Empty").await;

        let ctx = store
            .recent_thread_messages_for_extraction(thread, 5, None)
            .await
            .expect("get context");
        assert_eq!(ctx, "");

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn recent_thread_messages_for_extraction_excludes_event() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let thread = Uuid::new_v4();
        insert_thread(&pool, thread, "Test").await;
        insert_message(&pool, thread, "MessageReceived", "first message").await;

        let target_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES ($1, 'MessageReceived', $2, $3, 'thread', $3::text)",
        )
        .bind(target_id)
        .bind(serde_json::json!({ "text": "EXCLUDE_ME" }))
        .bind(thread)
        .execute(&pool)
        .await
        .expect("insert event");

        let ctx = store
            .recent_thread_messages_for_extraction(thread, 5, Some(target_id))
            .await
            .expect("get context");

        assert!(ctx.contains("first message"), "ctx={}", ctx);
        assert!(!ctx.contains("EXCLUDE_ME"), "should exclude target; ctx={}", ctx);

        teardown_test_db(&db).await;
    }

    /// A thread should match a multi-token query when its `memory_entries.entities`
    /// cover all tokens, even when no event payload text contains them — entity
    /// extraction adds linkages the raw payload doesn't have.
    #[tokio::test]
    async fn search_threads_by_text_matches_via_entities() {
        let (pool, db) = setup_test_db().await;
        ensure_memory_entries_table(&pool).await;
        let store = EventStore::new(pool.clone());

        let thread = Uuid::new_v4();
        insert_thread(&pool, thread, "Some title").await;
        let event_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES ($1, 'MessageReceived', $2, $3, 'thread', $3::text)"
        )
        .bind(event_id)
        .bind(serde_json::json!({"text": "ringer sykehuset om operasjonen i morgen"}))
        .bind(thread)
        .execute(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO memory_entries (id, source, topic, summary, importance, entities, embedding, embedding_model, src_created_at, created_at) \
             VALUES ($1, $2::jsonb, 'father health', 'Pappa har øyeoperasjon', 0.9, $3::jsonb, $4::vector, 'multilingual-e5-small', NOW(), NOW())"
        )
        .bind(Uuid::new_v4())
        .bind(serde_json::json!({"type": "event", "id": event_id}))
        .bind(serde_json::json!(["pappa", "øye", "operasjon"]))
        .bind(format!("[{}]", vec!["0"; 384].join(",")))
        .execute(&pool).await.unwrap();

        let results = store.search_threads_by_text("pappa øye", 20).await.unwrap();
        let ids: Vec<&str> = results.iter().map(|r| r.info.thread_id.as_str()).collect();
        let target = thread.to_string();
        assert!(
            ids.contains(&target.as_str()),
            "thread should match via entity tokens. ids={:?}",
            ids
        );

        teardown_test_db(&db).await;
    }
}
