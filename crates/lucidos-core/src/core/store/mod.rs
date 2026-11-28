mod conversation;
pub mod integrated;
pub mod messages;
pub mod thread_audience;
pub mod thread_hold;
pub mod thread_stop;
mod threads;
pub mod types;

use crate::core::EventRow;
use crate::engine::thread_events::ThreadEvent;
use crate::engine::thread_events::AUXILIARY_CAPTURE_SQL;
use chrono::{DateTime, Utc};
pub use messages::{
    format_child_report_filed_block, format_child_thread_completed_block, ChangeStatuses,
};
use sqlx::PgPool;
pub(crate) use threads::compose_report_cols;
pub(crate) use threads::pin_legacy_compose_agents;
pub(crate) use threads::restore_compose_slot;
pub use threads::{
    active_thread_statuses, attach_reader_fields, char_length, consume_compose_slot,
    fetch_thread_aggregate, has_draft, parse_status_filter_csv, parse_status_filter_values,
    status_value_list, still_working_sql, text_preview, thread_link, ComposeReport,
    ComposeReportRow, ComposeRestoration, DraftSummary, EventWaitSummary, FilterFacet,
    FilterFacets, LegacyInitiator, MessageMatch, StatusFilter, ThreadAggregate, ThreadSearchResult,
    ThreadSummary, ThreadSummaryFilters, COMPOSE_REPORT_COLS, HAS_DRAFT_SQL, MAX_COMPOSE_IMAGES,
    MAX_COMPOSE_SELECTION_BYTES, MAX_COMPOSE_TEXT_BYTES, PREVIEW_CHARS, UNTITLED_THREAD,
};
pub use types::*;
use uuid::Uuid;
pub use {
    messages::build_resume_tool_blocks_with_skip_ids, messages::find_orphan_tool_called_ids,
    messages::newest_conversation_summary, messages::parse_event_address,
    messages::synthesize_tool_use_id, messages::tool_called_event_id_of, messages::typed_on_device,
    messages::with_event_address, messages::CachedSummary, messages::RESUME_VERBATIM_TOOL_TAIL,
};
pub use {messages::collect_tool_pairs_chronological, messages::ResumeToolPair};
pub use {threads::format_display_title, threads::is_thread_ancestor};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// INVARIANT: User-facing step label for a `ThreadEvent::MemoryRecalled`.
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

/// INVARIANT: Escape SQL LIKE / ILIKE metacharacters so user-typed `\` `%` `_` match
/// literally. Backslash is escaped first because it's the ESCAPE character
/// — escaping it later would double-escape the inserted backslashes from
/// `%` / `_`. Pair with `LIKE/ILIKE ... ESCAPE '\'` (or rely on the default
/// backslash escape if no ESCAPE clause is present).
pub fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

const SCREENSHOT_PATH_DELIMS: &[char] = &['"', '\n', ' ', ')'];

pub(crate) fn extract_screenshot_path(result: &str) -> Option<String> {
    let start = result.find("screenshots/")?;
    let path_part = &result[start..];
    let end = path_part
        .find(SCREENSHOT_PATH_DELIMS)
        .unwrap_or(path_part.len());
    Some(path_part[..end].to_string())
}

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

#[derive(Debug, Default, Clone, Copy)]
pub struct EventQueryFilters<'a> {
    pub event_type: Option<&'a str>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub before_event_id: Option<Uuid>,
    pub after_event_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub event_id: Option<Uuid>,
}

#[derive(Debug)]
pub enum QueryEventsResult {
    Events(Vec<EventRow>),
    CursorNotFound,
}

enum CursorResolution {
    None,
    Found((DateTime<Utc>, Uuid)),
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

    /// INVARIANT: Query events with optional filters and a limit, no cursor paging.
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

    pub async fn tool_result_for_call(
        &self,
        tool_called_event_id: Uuid,
        thread_id: Option<Uuid>,
    ) -> Result<Option<EventRow>, sqlx::Error> {
        sqlx::query_as::<_, EventRow>(
            "SELECT id, event_type, payload, created, thread_id, sequence FROM events \
             WHERE event_type = 'ToolResult' \
             AND payload->>'tool_called_event_id' = $1 \
             AND ($2::uuid IS NULL OR thread_id = $2) \
             ORDER BY sequence ASC LIMIT 1",
        )
        .bind(tool_called_event_id.to_string())
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await
    }

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
        sqlx::query_as::<_, EventRow>(
            "SELECT id, event_type, payload, created, thread_id, sequence FROM events \
             WHERE ($1::text IS NULL OR event_type = $1) \
             AND ($2::timestamptz IS NULL OR created > $2) \
             AND ($3::timestamptz IS NULL OR created < $3) \
             AND ($5::timestamptz IS NULL OR created < $5 OR (created = $5 AND id < $6)) \
             AND ($7::timestamptz IS NULL OR created > $7 OR (created = $7 AND id > $8)) \
             AND ($9::uuid IS NULL OR thread_id = $9) \
             AND ($10::uuid IS NULL OR id = $10) \
             AND event_type <> ALL($11) \
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
        .bind(filters.event_id)
        .bind(ThreadEvent::SIDE_QUESTION_EVENT_TYPES)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn count_events(
        &self,
        event_type: Option<&str>,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        thread_id: Option<Uuid>,
    ) -> Result<(i64, i64), sqlx::Error> {
        let (count, bytes): (i64, Option<i64>) = sqlx::query_as(
            "SELECT COUNT(*)::bigint, SUM(octet_length(payload::text))::bigint FROM events \
             WHERE ($1::text IS NULL OR event_type = $1) \
             AND ($2::timestamptz IS NULL OR created > $2) \
             AND ($3::timestamptz IS NULL OR created < $3) \
             AND ($5::uuid IS NULL OR thread_id = $5) \
             AND event_type <> ALL($4)",
        )
        .bind(event_type)
        .bind(since)
        .bind(until)
        .bind(ThreadEvent::SIDE_QUESTION_EVENT_TYPES)
        .bind(thread_id)
        .fetch_one(&self.pool)
        .await?;
        Ok((count, bytes.unwrap_or(0)))
    }

    pub async fn count_events_by_type(
        &self,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        thread_id: Option<Uuid>,
    ) -> Result<Vec<(String, i64, i64)>, sqlx::Error> {
        let rows: Vec<(String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT event_type, COUNT(*)::bigint, SUM(octet_length(payload::text))::bigint \
             FROM events \
             WHERE ($1::timestamptz IS NULL OR created > $1) \
             AND ($2::timestamptz IS NULL OR created < $2) \
             AND ($4::uuid IS NULL OR thread_id = $4) \
             AND event_type <> ALL($3) \
             GROUP BY event_type \
             ORDER BY COUNT(*) DESC, event_type ASC",
        )
        .bind(since)
        .bind(until)
        .bind(ThreadEvent::SIDE_QUESTION_EVENT_TYPES)
        .bind(thread_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(et, count, bytes)| (et, count, bytes.unwrap_or(0)))
            .collect())
    }

    /// INVARIANT: Return all distinct event_type values, ordered alphabetically, less
    /// the side-question names no agent may see.
    ///
    /// A loose index scan, NOT `SELECT DISTINCT`, and the difference is the
    /// point. Postgres has no skip scan, so `SELECT DISTINCT` reads every row.
    /// On a 3.3M-event workspace that took 480ms and 177k buffers for 209
    /// names, and it grows with the table. This walks
    /// `idx_events_type_created` one descent per distinct value: 8ms and 972
    /// buffers for the same names. Cost now tracks the number of event types,
    /// not the number of events.
    ///
    /// The recursive term yields NULL when no greater value exists. That is a
    /// safe terminator only because `events.event_type` is NOT NULL, so no
    /// real row can produce it.
    pub async fn distinct_event_types(&self) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "WITH RECURSIVE t AS ( \
                 (SELECT event_type FROM events ORDER BY event_type LIMIT 1) \
                 UNION ALL \
                 SELECT (SELECT e.event_type FROM events e \
                         WHERE e.event_type > t.event_type \
                         ORDER BY e.event_type LIMIT 1) \
                 FROM t WHERE t.event_type IS NOT NULL \
             ) \
             SELECT event_type FROM t \
             WHERE event_type IS NOT NULL AND event_type <> ALL($1) \
             ORDER BY event_type",
        )
        .bind(ThreadEvent::SIDE_QUESTION_EVENT_TYPES)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

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

    /// INVARIANT: What a conversation snapshot at `until` reads, oldest first: one
    /// thread's events, plus every artifact write, which the snapshot takes
    /// its commit from. Artifact events belong to no thread. A `None` thread
    /// reads the rows that belong to no thread.
    pub async fn get_conversation_events_until(
        &self,
        thread_id: Option<Uuid>,
        until: DateTime<Utc>,
    ) -> Result<Vec<EventRow>, sqlx::Error> {
        sqlx::query_as::<_, EventRow>(
            r#"
            SELECT id, event_type, payload, created, thread_id, sequence
            FROM events
            WHERE created <= $2
              AND (thread_id IS NOT DISTINCT FROM $1
                   OR event_type IN ('ArtifactCreated', 'ArtifactUpdated'))
            ORDER BY created ASC, sequence ASC
            "#,
        )
        .bind(thread_id)
        .bind(until)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn events_of_types_chronological(
        &self,
        event_types: &[&str],
    ) -> Result<Vec<EventRow>, sqlx::Error> {
        sqlx::query_as::<_, EventRow>(
            "SELECT id, event_type, payload, created, thread_id, sequence FROM events \
             WHERE event_type = ANY($1) ORDER BY created ASC, sequence ASC",
        )
        .bind(event_types)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn memory_index_events(&self, types: &[&str]) -> Result<Vec<EventRow>, sqlx::Error> {
        use crate::engine::consumption_claim::{model_delivery_join, model_event_visible};
        let delivery = model_delivery_join();
        let visible = model_event_visible();
        sqlx::query_as(&format!(
            "SELECT candidate.* FROM events candidate {delivery}
             WHERE candidate.event_type = ANY($1) AND {visible}
             ORDER BY COALESCE(delivery.created, candidate.created),
                      COALESCE(delivery.sequence, candidate.sequence)"
        ))
        .bind(types)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn events_of_type_chronological(
        &self,
        event_type: &str,
    ) -> Result<Vec<EventRow>, sqlx::Error> {
        self.events_of_types_chronological(&[event_type]).await
    }

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

    /// INVARIANT: The NEWEST `limit` drawable events of a thread, older than `before`.
    ///
    /// Rows come back oldest-first, the order every reader already expects, so
    /// a page is prepended to what the client holds rather than reversed there.
    ///
    /// **A page holds drawable history.** It skips auxiliary captures
    /// ([`AUXILIARY_CAPTURE_SQL`]) and failed model calls, in its rows and in
    /// `has_more`. The transcript drops both, so a page of them draws nothing.
    /// The home thread records thousands, since background calls bill it. The
    /// unpaged and delta reads keep them.
    ///
    /// **The cursor is the pair, never the sequence alone.** Rows are ordered by
    /// `(created, sequence)` because a sequence is allocated globally and can
    /// run against the clock within one thread. Paging on `sequence` alone
    /// would therefore skip or repeat rows at a page edge.
    ///
    /// **One statement, so the page and the watermark share a snapshot.** Read
    /// separately, a drawable event landing between the two queries would put
    /// a sequence in `max_sequence` that the page should carry and does not.
    /// The client advances its forward mark to it, and that event is then
    /// skipped by every later delta.
    pub async fn get_thread_events_page(
        &self,
        thread_id: Uuid,
        before: Option<(DateTime<Utc>, i64)>,
        limit: i64,
    ) -> Result<ThreadEventPage, sqlx::Error> {
        let (before_created, before_seq) = match before {
            Some((created, seq)) => (Some(created), Some(seq)),
            None => (None, None),
        };
        // INVARIANT: One row beyond the page answers `has_more` without a second query,
        // and without a count that could disagree with the page it describes.
        // The watermark joins the page rather than riding on its rows, so a
        // page that skipped every row still carries it.
        let sql = format!(
            r#"WITH page AS (
                 SELECT sequence, event_type, payload, created, id
                 FROM events
                 WHERE thread_id = $1
                   AND ($2::timestamptz IS NULL OR (created, sequence) < ($2, $3))
                   AND NOT {AUXILIARY_CAPTURE_SQL}
                   AND event_type <> 'ModelCallFailed'
                 ORDER BY created DESC, sequence DESC
                 LIMIT $4
               ),
               watermark AS (
                 SELECT max(sequence) AS max_sequence FROM events WHERE thread_id = $1
               )
               SELECT page.sequence, page.event_type, page.payload, page.created,
                      page.id as event_id, watermark.max_sequence
               FROM watermark LEFT JOIN page ON true
               ORDER BY page.created ASC, page.sequence ASC"#
        );
        let rows = sqlx::query_as::<_, PagedEventRow>(&sql)
            .bind(thread_id)
            .bind(before_created)
            .bind(before_seq)
            .bind(limit.saturating_add(1))
            .fetch_all(&self.pool)
            .await?;

        let max_sequence = rows.first().and_then(|r| r.max_sequence);
        let mut events: Vec<ThreadEventRow> =
            rows.into_iter().filter_map(PagedEventRow::event).collect();
        let has_more = events.len() as i64 > limit;
        if has_more {
            // INVARIANT: The extra row is the OLDEST, because the page is ascending.
            events.remove(0);
        }
        Ok(ThreadEventPage {
            events,
            has_more,
            max_sequence,
        })
    }
}

#[derive(sqlx::FromRow)]
struct PagedEventRow {
    sequence: Option<i64>,
    event_type: Option<String>,
    payload: Option<serde_json::Value>,
    created: Option<DateTime<Utc>>,
    event_id: Option<Uuid>,
    max_sequence: Option<i64>,
}

impl PagedEventRow {
    fn event(self) -> Option<ThreadEventRow> {
        Some(ThreadEventRow {
            sequence: self.sequence?,
            event_type: self.event_type?,
            payload: self.payload?,
            created: self.created?,
            event_id: self.event_id?,
        })
    }
}

pub struct ThreadEventPage {
    pub events: Vec<ThreadEventRow>,
    pub has_more: bool,
    /// INVARIANT: The highest `sequence` the whole thread holds, or `None` when it holds
    /// nothing. NOT derivable from the page: see `get_thread_events_page`.
    pub max_sequence: Option<i64>,
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod mod_tests;

pub async fn memory_table_exists<'e, E>(executor: E) -> Result<bool, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query_scalar("SELECT to_regclass('public.memory_entries') IS NOT NULL")
        .fetch_one(executor)
        .await
}

pub async fn ensure_memory_entries_table(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    const VECTOR_DIM: i32 = 384;

    sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
        .execute(pool)
        .await?;

    sqlx::query(&format!(
        r#"
        CREATE TABLE IF NOT EXISTS memory_entries (
            id UUID PRIMARY KEY,
            source JSONB NOT NULL,
            topic TEXT NOT NULL,
            summary TEXT NOT NULL,
            importance REAL NOT NULL,
            entities JSONB NOT NULL DEFAULT '[]',
            embedding vector({}) NOT NULL,
            embedding_model TEXT NOT NULL,
            src_created_at TIMESTAMPTZ NOT NULL,
            created_at TIMESTAMPTZ NOT NULL,
            extractor_version INTEGER NOT NULL DEFAULT 0
        )
        "#,
        VECTOR_DIM
    ))
    .execute(pool)
    .await?;
    Ok(())
}
