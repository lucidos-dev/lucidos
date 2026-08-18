mod conversation;
pub(crate) mod messages;
mod threads;
pub mod types;

use crate::core::EventRow;
use chrono::{DateTime, Utc};
pub use messages::format_child_thread_completed_block;
pub(crate) use messages::{
    build_resume_tool_blocks_with_skip_ids, build_session_messages,
    collect_tool_pairs_chronological, find_orphan_tool_called_ids, RESUME_VERBATIM_TOOL_TAIL,
};
use sqlx::PgPool;
pub use threads::{
    active_thread_statuses, fetch_thread_aggregate, parse_status_filter_csv,
    parse_status_filter_values, status_value_list, EventWaitSummary, FilterFacet, FilterFacets,
    LegacyInitiator, StatusFilter, ThreadAggregate, ThreadSearchResult, ThreadSummary,
    ThreadSummaryFilters,
};
pub use types::*;
use uuid::Uuid;

/// User-facing step label for a `ThreadEvent::MemoryRecalled`.
///
/// Deliberately verb-first and free of the word "search". The `memory` tool's
/// own step reads "Searching memory for ...", and until 2026-08-12 this one
/// read "Memory searched", so the engine's automatic pre-turn recall and the
/// agent's deliberate mid-turn lookup sat one word apart in the same step list
/// and read as the same thing happening twice.
///
/// Two Rust callers (conversation time-travel, the messages projection) plus
/// the frontend renderer in
/// `crates/lucidos-app/src/store/thread-events/exchange-render.ts` all build
/// this text. The Rust pair share this function; the frontend is pinned to the
/// same strings by its own tests. Change one, change all three.
pub(crate) fn memory_recalled_label(results: u64) -> String {
    match results {
        0 => "No memories recalled".to_string(),
        1 => "Recalled 1 memory".to_string(),
        n => format!("Recalled {} memories", n),
    }
}

/// Escape SQL LIKE / ILIKE metacharacters so user-typed `\` `%` `_` match
/// literally. Backslash is escaped first because it's the ESCAPE character
/// — escaping it later would double-escape the inserted backslashes from
/// `%` / `_`. Pair with `LIKE/ILIKE ... ESCAPE '\'` (or rely on the default
/// backslash escape if no ESCAPE clause is present).
pub fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Delimiters that terminate a screenshot path embedded in a tool result.
///
/// Includes `)` so that markdown link output like `[label](screenshots/foo.png)`
/// truncates at the closing paren. Screenshot filenames never contain any of
/// these characters, so widening the set is safe across the two call sites
/// (conversation time-travel, messages projection).
const SCREENSHOT_PATH_DELIMS: &[char] = &['"', '\n', ' ', ')'];

/// Extract the screenshot path from a `browser_screenshot` tool result.
///
/// Looks for the first `"screenshots/"` token in `result` and returns the
/// substring up to (but not including) the first occurrence of any
/// [`SCREENSHOT_PATH_DELIMS`] delimiter — or end-of-string if none is
/// present. Returns `None` if `"screenshots/"` is absent.
pub(crate) fn extract_screenshot_path(result: &str) -> Option<String> {
    let start = result.find("screenshots/")?;
    let path_part = &result[start..];
    let end = path_part
        .find(SCREENSHOT_PATH_DELIMS)
        .unwrap_or(path_part.len());
    Some(path_part[..end].to_string())
}

/// Generate a human-readable description for a tool call event.
/// Prefers the stored `description` field (new events); falls back to computing it (old events).
pub(super) fn describe_tool_event(event: &EventRow) -> (String, String) {
    let tool_name = event
        .payload
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let description = event
        .payload
        .get("description")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let empty = serde_json::Value::Object(serde_json::Map::new());
            let args = event.payload.get("args").unwrap_or(&empty);
            super::describe_tool(&tool_name, args)
        });
    (tool_name, description)
}

/// Filters for the paged event-query path, the events-side sibling of
/// [`ThreadSummaryFilters`].
///
/// A struct rather than seven positional parameters because they are all
/// optional and several share a type: `Option<Uuid>` appears three times
/// (`before_event_id`, `after_event_id`, `thread_id`), and at the call site
/// nothing but argument order would have distinguished them.
#[derive(Debug, Default, Clone, Copy)]
pub struct EventQueryFilters<'a> {
    /// Exact `event_type` match. `None` queries every type, which is worth
    /// avoiding on a busy workspace.
    pub event_type: Option<&'a str>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// Walk backward from this event, exclusive, under `(created, id)`
    /// lexicographic order.
    pub before_event_id: Option<Uuid>,
    /// Tail-follow forward from this event, exclusive.
    pub after_event_id: Option<Uuid>,
    /// Restrict to one thread. `None` is every thread, which is what every
    /// caller predating this field passes, so the filter can only narrow.
    pub thread_id: Option<Uuid>,
}

/// Outcome of [`EventStore::query_events_paged`]. Kept separate from
/// `sqlx::Error` so the HTTP layer can map a missing cursor to a 404 without
/// string-matching error messages.
#[derive(Debug)]
pub enum QueryEventsResult {
    Events(Vec<EventRow>),
    CursorNotFound,
}

/// Internal three-way result of resolving an optional cursor uuid.
enum CursorResolution {
    /// Caller didn't pass a cursor.
    None,
    /// Cursor exists; carries the `(created, id)` tuple used in WHERE clauses.
    Found((DateTime<Utc>, Uuid)),
    /// Cursor uuid was passed but doesn't resolve to any event.
    Missing,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ThreadEventRow {
    pub sequence: i64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created: DateTime<Utc>,
    pub event_id: uuid::Uuid,
}

impl crate::core::events::HasEventPayload for ThreadEventRow {
    fn event_type(&self) -> &str {
        &self.event_type
    }
    fn payload(&self) -> &serde_json::Value {
        &self.payload
    }
    fn payload_mut(&mut self) -> &mut serde_json::Value {
        &mut self.payload
    }
}

#[derive(Clone)]
pub struct EventStore {
    pool: PgPool,
}

impl EventStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Defensive double-write: the same CREATE TABLE / CREATE INDEX
    /// statements now live in
    /// `migrations/20260517160627_consolidate_init_schema_tables.sql`,
    /// which is the canonical home per `.claude/rules/rust.md`. This body
    /// is kept for one release cycle so existing installs that boot a
    /// pre-migration build still come up; slated for removal in
    /// `harden-init-schema-tables-vs-migrations-pattern-finish`.
    pub async fn init_schema(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS events (
                id UUID PRIMARY KEY,
                event_type TEXT NOT NULL,
                payload JSONB NOT NULL,
                created TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Composite index: event_type + created for event queries
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_events_type_created
            ON events (event_type, created ASC)
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Functional index on JSONB request_id for request lookups
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_events_request_id
            ON events ((payload->>'request_id'))
            WHERE payload->>'request_id' IS NOT NULL
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Legacy payload thread_id index — replaced by thread_id column + idx_events_thread_seq.
        // Drop if it still exists from older installations.
        sqlx::query("DROP INDEX IF EXISTS idx_events_thread_id")
            .execute(&self.pool)
            .await?;

        // Index on created for range queries (get_events_until, chronological loads)
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_events_created
            ON events (created ASC)
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Query events with optional filters and a limit, no cursor paging.
    /// Used by App UIs and the LLM `query_events` tool to fetch domain events
    /// (e.g. GoogleDocEdited). Newest-first; rows sharing one timestamp tie-
    /// break on `id DESC` so the order is deterministic across calls.
    ///
    /// Takes the whole [`EventQueryFilters`] rather than the three it used to
    /// spell out. A filter this signature does not name is a filter its caller
    /// silently drops, and that is not hypothetical: the LLM tool gained a
    /// `thread_id` and kept calling the three-argument form, so the model could
    /// ask for one thread's messages and be handed every thread's.
    pub async fn query_events(
        &self,
        filters: EventQueryFilters<'_>,
        limit: i64,
    ) -> Result<Vec<EventRow>, sqlx::Error> {
        self.fetch_events(filters, None, None, limit).await
    }

    /// Cursor-paged variant of [`Self::query_events`]. Pass `before_event_id`
    /// to walk backward (strictly older than the cursor under
    /// `(created, id)` lexicographic order) or `after_event_id` to
    /// tail-follow. The HTTP layer rejects "both set" with 400; this method
    /// AND's them together if both are passed. If a supplied cursor uuid
    /// doesn't exist, returns [`QueryEventsResult::CursorNotFound`] instead
    /// of silently returning the unfiltered history.
    pub async fn query_events_paged(
        &self,
        filters: EventQueryFilters<'_>,
        limit: i64,
    ) -> Result<QueryEventsResult, sqlx::Error> {
        let before_cursor = match self.resolve_cursor(filters.before_event_id).await? {
            CursorResolution::None => None,
            CursorResolution::Found(c) => Some(c),
            CursorResolution::Missing => return Ok(QueryEventsResult::CursorNotFound),
        };
        let after_cursor = match self.resolve_cursor(filters.after_event_id).await? {
            CursorResolution::None => None,
            CursorResolution::Found(c) => Some(c),
            CursorResolution::Missing => return Ok(QueryEventsResult::CursorNotFound),
        };
        let events = self
            .fetch_events(filters, before_cursor, after_cursor, limit)
            .await?;
        Ok(QueryEventsResult::Events(events))
    }

    /// Project `get_event_by_id` to the `(created, id)` cursor tuple, with a
    /// three-way result so the paged caller can distinguish "no cursor asked
    /// for" from "cursor asked for but missing".
    async fn resolve_cursor(&self, id: Option<Uuid>) -> Result<CursorResolution, sqlx::Error> {
        match id {
            None => Ok(CursorResolution::None),
            Some(id) => Ok(match self.get_event_by_id(id).await? {
                Some(row) => CursorResolution::Found((row.created, row.id)),
                None => CursorResolution::Missing,
            }),
        }
    }

    async fn fetch_events(
        &self,
        filters: EventQueryFilters<'_>,
        before_cursor: Option<(DateTime<Utc>, Uuid)>,
        after_cursor: Option<(DateTime<Utc>, Uuid)>,
        limit: i64,
    ) -> Result<Vec<EventRow>, sqlx::Error> {
        // `thread_id` is one more `IS NULL OR` in the same shape as every other
        // filter, so omitting it returns exactly what it always returned. That
        // matters: every existing caller (the CLI, triggers, the SDK) passes
        // `None`, and a filter that narrowed by default would silently change
        // all of them. It rides the `idx_events_thread_id_created_seq` index.
        sqlx::query_as::<_, EventRow>(
            "SELECT id, event_type, payload, created, thread_id, sequence FROM events \
             WHERE ($1::text IS NULL OR event_type = $1) \
             AND ($2::timestamptz IS NULL OR created > $2) \
             AND ($3::timestamptz IS NULL OR created < $3) \
             AND ($5::timestamptz IS NULL OR created < $5 OR (created = $5 AND id < $6)) \
             AND ($7::timestamptz IS NULL OR created > $7 OR (created = $7 AND id > $8)) \
             AND ($9::uuid IS NULL OR thread_id = $9) \
             ORDER BY created DESC, id DESC LIMIT $4",
        )
        .bind(filters.event_type)
        .bind(filters.since)
        .bind(filters.until)
        .bind(limit)
        .bind(before_cursor.map(|(c, _)| c))
        .bind(before_cursor.map(|(_, i)| i))
        .bind(after_cursor.map(|(c, _)| c))
        .bind(after_cursor.map(|(_, i)| i))
        .bind(filters.thread_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Count events matching the same filters as [`Self::query_events`],
    /// without materialising the payloads.
    ///
    /// Returns `(count, byte_total)` for the single-type filter; when
    /// `event_type` is `None`, the caller should use
    /// [`Self::count_events_by_type`] instead — this helper only collapses
    /// to a scalar.
    ///
    /// `byte_total` is `SUM(octet_length(payload::text))` — the on-disk
    /// JSON size of the payloads. It's a good proxy for token cost when
    /// the LLM is deciding whether a sweep will fit in its context budget.
    pub async fn count_events(
        &self,
        event_type: Option<&str>,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
    ) -> Result<(i64, i64), sqlx::Error> {
        let (count, bytes): (i64, Option<i64>) = sqlx::query_as(
            "SELECT COUNT(*)::bigint, SUM(octet_length(payload::text))::bigint FROM events \
             WHERE ($1::text IS NULL OR event_type = $1) \
             AND ($2::timestamptz IS NULL OR created > $2) \
             AND ($3::timestamptz IS NULL OR created < $3)",
        )
        .bind(event_type)
        .bind(since)
        .bind(until)
        .fetch_one(&self.pool)
        .await?;
        Ok((count, bytes.unwrap_or(0)))
    }

    /// Per-`event_type` breakdown across the same time window as
    /// [`Self::count_events`], ordered by count descending so the noisiest
    /// types surface first. Used by the `count_events` LLM tool and the
    /// `GET /api/v1/events/count` endpoint when no `event_type` filter is
    /// passed.
    pub async fn count_events_by_type(
        &self,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
    ) -> Result<Vec<(String, i64, i64)>, sqlx::Error> {
        let rows: Vec<(String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT event_type, COUNT(*)::bigint, SUM(octet_length(payload::text))::bigint \
             FROM events \
             WHERE ($1::timestamptz IS NULL OR created > $1) \
             AND ($2::timestamptz IS NULL OR created < $2) \
             GROUP BY event_type \
             ORDER BY COUNT(*) DESC, event_type ASC",
        )
        .bind(since)
        .bind(until)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(et, count, bytes)| (et, count, bytes.unwrap_or(0)))
            .collect())
    }

    /// Return all distinct event_type values, ordered alphabetically.
    pub async fn distinct_event_types(&self) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT DISTINCT event_type FROM events ORDER BY event_type")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Get a single event by its ID
    pub async fn get_event_by_id(&self, id: Uuid) -> Result<Option<EventRow>, sqlx::Error> {
        let event = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT id, event_type, payload, created, thread_id, sequence
            FROM events
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(event)
    }

    /// Get all events up to and including the given timestamp, ordered chronologically
    pub async fn get_events_until(
        &self,
        until: DateTime<Utc>,
    ) -> Result<Vec<EventRow>, sqlx::Error> {
        let events = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT id, event_type, payload, created, thread_id, sequence
            FROM events
            WHERE created <= $1
            ORDER BY created ASC
            "#,
        )
        .bind(until)
        .fetch_all(&self.pool)
        .await?;

        Ok(events)
    }

    /// Load all events in chronological order (oldest first).
    /// Used by rebuild_memory to reprocess the entire event history.
    pub async fn get_all_events_chronological(
        &self,
    ) -> Result<Vec<EventRow>, Box<dyn std::error::Error + Send + Sync>> {
        let events = sqlx::query_as::<_, EventRow>(
            "SELECT id, event_type, payload, created, thread_id, sequence FROM events ORDER BY created ASC"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(events)
    }

    /// Get thread events after a given sequence number (or all if None).
    /// Returns rows ordered by sequence ASC for replay.
    pub async fn get_thread_events_by_seq(
        &self,
        thread_id: Uuid,
        after_seq: Option<i64>,
    ) -> Result<Vec<ThreadEventRow>, sqlx::Error> {
        sqlx::query_as::<_, ThreadEventRow>(
            r#"SELECT sequence, event_type, payload, created, id as event_id
            FROM events
            WHERE thread_id = $1
              AND ($2::bigint IS NULL OR sequence > $2)
            ORDER BY created ASC, sequence ASC"#,
        )
        .bind(thread_id)
        .bind(after_seq)
        .fetch_all(&self.pool)
        .await
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod mod_tests;
