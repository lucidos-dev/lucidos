use super::messages::build_session_messages;
use super::types::SessionMessage;
use super::EventStore;
use crate::core::EventRow;
use crate::engine::thread_lifecycle::{ArchiveState, ThreadStatus};
use serde::Serialize;

/// Preference marker: set after `backfill_trigger_id_v5_to_config_id` runs
/// successfully so subsequent boots skip the events scan.
const BACKFILL_TRIGGER_ID_V5_MARKER: &str = "backfill_trigger_id_v5_to_config_id_done";

/// Preference marker: set after `backfill_trigger_id_from_events` runs
/// successfully so subsequent boots skip the events scan.
const BACKFILL_TRIGGER_ID_FROM_EVENTS_MARKER: &str = "backfill_trigger_id_from_events_done";

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
    /// Thread section: "archived" (history/saved), "inbox" (needs user attention).
    /// Stored in `thread_summaries.archive_state` column; aliased to `section` in
    /// SELECTs to keep the JSON wire format stable.
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
    /// Trigger that fired this thread (only for `channel == "trigger"` threads).
    /// Snapshotted by the projection on `TriggerStarted`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_id: Option<String>,
    /// Trigger name at fire-time (snapshot — falls back when the trigger is
    /// later renamed or deleted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_name: Option<String>,
    /// Repository the CC session bound to (only for `channel == "claude_code"`
    /// threads). Stored as TEXT on `thread_summaries.cc_repo_id`; matches a
    /// `repositories.id` UUID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_repo_id: Option<String>,
    /// Current repo display name resolved from the registry. NULL when the
    /// repo was deleted from `repositories` after the thread was bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_repo_name: Option<String>,
    /// Compose state machine (`composing` | `active` | `discarded` | `archived`).
    /// Frontends filter to render the drafts section as `state == 'composing'`.
    pub state: String,
    /// In-progress compose text. Empty string when the user has nothing typed.
    pub compose_text: String,
    /// Currently-attached compose image URLs. JSON array; empty when none.
    pub compose_images: serde_json::Value,
    /// User's mode preference while composing (`lucidos` | `claude_code`).
    /// `None` once the thread transitions out of `composing`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose_mode: Option<String>,
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
    is_saved: bool,
    has_response: bool,
    parent_thread_id: Option<String>,
    parent_thread_title: Option<String>,
    trigger_id: Option<String>,
    trigger_name: Option<String>,
    cc_repo_id: Option<String>,
    cc_repo_name: Option<String>,
    state: String,
    compose_text: String,
    compose_images: serde_json::Value,
    compose_mode: Option<String>,
}

/// Per-event projection snapshot carried on persisted thread events
/// (via `EmittedEvent.aggregate`) and on `fetchThreadEvents` HTTP responses
/// (via `currentAggregate`). Frontend overlays this onto `thread.meta` so
/// it never has to derive thread state from event-type lookups.
///
/// Excludes compose fields (`compose_text`, `compose_images`, `compose_mode`):
/// those have their own broadcast cadence (compose events) and including them
/// here would clobber the user's local draft on every event broadcast.
/// Those fields stay on `ThreadInfo` for the `/api/threads` initial fetch
/// where they represent the authoritative server-side compose snapshot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadAggregate {
    pub thread_id: String,
    pub title: String,
    pub channel: String,
    pub initiator: LegacyInitiator,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_activity: chrono::DateTime<chrono::Utc>,
    pub message_count: i64,
    pub section: String,
    pub status: String,
    pub active_children_count: i64,
    pub total_children_count: i64,
    pub cc_has_changes: bool,
    pub cc_requires_restart: bool,
    pub cc_is_external_repo: bool,
    pub cc_applying: bool,
    pub is_saved: bool,
    pub has_response: bool,
    pub last_revived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub parent_thread_id: Option<String>,
    pub parent_thread_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_repo_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_repo_name: Option<String>,
    pub state: String,
}

/// Fetch the projection snapshot for a thread. Polymorphic over executor —
/// callers pass either a `&PgPool` (HTTP handlers) or `&mut *tx` (inside an
/// open transaction, for read-your-write semantics in `EventBus::emit()`).
/// Returns `None` if the row doesn't exist.
pub async fn fetch_thread_aggregate<'e, E>(
    executor: E,
    thread_id: uuid::Uuid,
) -> Result<Option<ThreadAggregate>, Box<dyn std::error::Error + Send + Sync>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let sql = format!(
        "SELECT {} FROM thread_summaries t WHERE t.thread_id = $1",
        THREAD_COLS,
    );
    let row: Option<ThreadRow> = sqlx::query_as(&sql)
        .bind(thread_id)
        .fetch_optional(executor)
        .await?;
    row.map(row_to_thread_aggregate).transpose()
}

fn row_to_thread_aggregate(
    r: ThreadRow,
) -> Result<ThreadAggregate, Box<dyn std::error::Error + Send + Sync>> {
    let initiator = LegacyInitiator::from_db_str(r.initiator.as_str())?;
    Ok(ThreadAggregate {
        thread_id: r.thread_id,
        title: format_display_title(r.title, r.first_message),
        channel: r.source,
        initiator,
        created_at: r.created_at,
        last_activity: r.last_activity,
        message_count: r.message_count,
        section: r.section,
        status: r.status,
        active_children_count: r.active_children_count,
        total_children_count: r.total_children_count,
        cc_has_changes: r.cc_has_changes,
        cc_requires_restart: r.cc_requires_restart,
        cc_is_external_repo: r.cc_is_external_repo,
        cc_applying: r.cc_applying,
        is_saved: r.is_saved,
        has_response: r.has_response,
        last_revived_at: r.last_revived_at,
        parent_thread_id: r.parent_thread_id,
        parent_thread_title: r.parent_thread_title,
        trigger_id: r.trigger_id,
        trigger_name: r.trigger_name,
        cc_repo_id: r.cc_repo_id,
        cc_repo_name: r.cc_repo_name,
        state: r.state,
    })
}

/// SQL column list for thread summary queries. Callers MUST alias the outer
/// FROM as `t` (e.g. `FROM thread_summaries t` or `FROM (...) t`) — the
/// correlated subqueries reference `t.parent_thread_id` and `t.cc_repo_id`.
///
/// Both correlated subqueries hit a PK b-tree (`thread_summaries.thread_id`,
/// `repositories.id`) and short-circuit when the source FK is NULL. The cast
/// is on the FK side (`t.cc_repo_id::uuid`) — casting `r.id::text` instead
/// would prevent index use. Rows whose `cc_repo_id` no longer matches a row
/// in `repositories` get NULL `cc_repo_name` and the frontend renders them
/// as `(deleted)`.
const THREAD_COLS: &str =
    "t.thread_id::text, t.title, t.first_message, t.source, t.initiator, t.created_at, t.last_activity, \
    t.message_count::bigint, t.archive_state AS section, t.active_children_count::bigint, t.total_children_count::bigint, \
    t.status, t.cc_has_changes, t.cc_requires_restart, t.cc_is_external_repo, t.cc_applying, t.last_revived_at, \
    t.is_saved, t.has_response, \
    t.parent_thread_id::text AS parent_thread_id, \
    (SELECT p.title FROM thread_summaries p WHERE p.thread_id = t.parent_thread_id) AS parent_thread_title, \
    t.trigger_id, t.trigger_name, \
    t.cc_repo_id, \
    (SELECT r.name FROM repositories r WHERE r.id = t.cc_repo_id::uuid) AS cc_repo_name, \
    t.state, t.compose_text, t.compose_images, t.compose_mode";

impl EventStore {
    /// Get saved threads from the projection table.
    pub async fn get_saved_threads(
        &self,
    ) -> Result<Vec<ThreadInfo>, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "SELECT {} FROM thread_summaries t WHERE t.is_saved = TRUE ORDER BY t.last_activity DESC",
            THREAD_COLS,
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_infos(rows)
    }

    /// Get recent threads for the drawer.
    ///
    /// Returns every `archive_state='inbox'` thread (the REVIEW pile) and the
    /// top `per_source` archived threads per source (the History pile). Inbox
    /// is unbounded by design: an inbox row is one the user hasn't dismissed,
    /// so capping it would silently hide work the user expects to see —
    /// crashed CC sessions, idle chats they meant to come back to, and so on.
    /// History is capped because old archived threads aren't time-sensitive;
    /// the user can page back via `get_older_threads`.
    ///
    /// Also unconditionally includes active-status threads (`running`,
    /// `waiting_for_user_answer`) — a thread the user just started or that's
    /// blocked on user input must appear immediately, before any response
    /// arrives. And the actionable bypasses (`cc_has_changes=TRUE`,
    /// `status='failed'`, `status='waiting_for_user_answer'`) are preserved
    /// in case a future state lets one of those slip past `archive_state='inbox'`.
    pub async fn get_recent_threads(
        &self,
        per_source: i64,
    ) -> Result<Vec<ThreadInfo>, Box<dyn std::error::Error + Send + Sync>> {
        let active_statuses: &[&str] = &[
            ThreadStatus::Running.as_str(),
            ThreadStatus::WaitingForUserAnswer.as_str(),
        ];
        let actionable_statuses: &[&str] = &[
            ThreadStatus::WaitingForUserAnswer.as_str(),
            ThreadStatus::Failed.as_str(),
        ];
        let sql = format!(
            "SELECT {} FROM (\
                SELECT *, ROW_NUMBER() OVER (PARTITION BY source ORDER BY last_activity DESC) AS rn \
                FROM thread_summaries \
                WHERE has_response = TRUE OR status = ANY($1) OR cc_has_changes = TRUE\
            ) t \
            WHERE t.archive_state = '{}' \
               OR t.rn <= $2 \
               OR t.cc_has_changes = TRUE \
               OR t.status = ANY($3) \
            ORDER BY t.last_activity DESC",
            THREAD_COLS,
            ArchiveState::Inbox.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(active_statuses)
            .bind(per_source)
            .bind(actionable_statuses)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_infos(rows)
    }

    /// Older threads for infinite scroll. When `trigger_ids` or `repo_ids` is
    /// provided the SQL collapses to a narrowing branch keyed on those columns
    /// plus the pagination cursor — the dropdown advertises every trigger /
    /// repo that ever stamped a row (no `has_response` / `source` gate), so
    /// the filter must match the same set or the dropdown lies. The two ID
    /// arrays compose with OR so a user can expand both Trigger and Claude
    /// Code in the dropdown and see both narrowings together. Without either
    /// the normal history view applies (`has_response = TRUE` + optional
    /// channel filter).
    pub async fn get_older_threads(
        &self,
        before: chrono::DateTime<chrono::Utc>,
        limit: i64,
        sources: Option<&[String]>,
        trigger_ids: Option<&[String]>,
        repo_ids: Option<&[String]>,
    ) -> Result<Vec<ThreadInfo>, Box<dyn std::error::Error + Send + Sync>> {
        let rows = if trigger_ids.is_some() || repo_ids.is_some() {
            let trigger_ids_slice = trigger_ids.unwrap_or(&[]);
            let repo_ids_slice = repo_ids.unwrap_or(&[]);
            let sql = format!(
                "SELECT {} FROM thread_summaries t \
                 WHERE (t.trigger_id = ANY($1) OR t.cc_repo_id = ANY($2)) AND t.last_activity < $3 \
                 ORDER BY t.last_activity DESC LIMIT $4",
                THREAD_COLS,
            );
            sqlx::query_as::<_, ThreadRow>(&sql)
                .bind(trigger_ids_slice)
                .bind(repo_ids_slice)
                .bind(before)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
        } else {
            let sql = format!(
                "SELECT {} FROM thread_summaries t \
                 WHERE t.has_response = TRUE AND t.last_activity < $1 \
                 AND ($2::text[] IS NULL OR t.source = ANY($2)) \
                 ORDER BY t.last_activity DESC LIMIT $3",
                THREAD_COLS,
            );
            sqlx::query_as::<_, ThreadRow>(&sql)
                .bind(before)
                .bind(sources)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
        };

        Self::rows_to_thread_infos(rows)
    }

    /// Threads in `composing` state — the new "drafts" surface. The sidebar
    /// renders these in the Drafts section. Returned newest-first so the row
    /// the user just touched is at the top.
    pub async fn get_composing_threads(
        &self,
    ) -> Result<Vec<ThreadInfo>, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "SELECT {} FROM thread_summaries t \
             WHERE t.state = 'composing' \
             ORDER BY t.last_activity DESC",
            THREAD_COLS,
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .fetch_all(&self.pool)
            .await?;
        Self::rows_to_thread_infos(rows)
    }

    /// Recover `trigger_id`/`trigger_name` for trigger-source rows the
    /// `20260429214800_addtriggeridtothreadsummaries.sql` migration left NULL.
    ///
    /// Why: the SQL backfill in that migration only reads `payload->>'trigger_id'`,
    /// but legacy `TriggerStarted` events store the value under `task_id`
    /// (Rust deserializes both via `#[serde(alias = "task_id")]`, but Postgres
    /// doesn't honor serde aliases). Result: every pre-rename trigger thread
    /// shipped with NULL `trigger_id`, so the dropdown filter matched nothing
    /// and the historical-triggers list was empty.
    ///
    /// This function COALESCEs both shapes and writes the *first* TriggerStarted
    /// event's id/name per thread — same "first write wins" semantics as the
    /// runtime projection in `event_bus::apply_thread_event`. Runs before
    /// `backfill_trigger_id_v5_to_config_id` so the v5→config_id rewrite picks
    /// up the freshly recovered rows. Once-only — guarded by a marker.
    pub async fn backfill_trigger_id_from_events(
        &self,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        if crate::core::PreferenceStore::get(&self.pool, BACKFILL_TRIGGER_ID_FROM_EVENTS_MARKER)
            .await?
            .is_some()
        {
            return Ok(0);
        }

        let updated = sqlx::query(
            "UPDATE thread_summaries ts \
             SET trigger_id = first_evt.trigger_id, \
                 trigger_name = first_evt.trigger_name \
             FROM ( \
                 SELECT DISTINCT ON (e.aggregate_id) \
                     e.aggregate_id, \
                     COALESCE(e.payload->>'trigger_id', e.payload->>'task_id')     AS trigger_id, \
                     COALESCE(e.payload->>'trigger_name', e.payload->>'task_name') AS trigger_name \
                 FROM events e \
                 WHERE e.event_type = 'TriggerStarted' \
                 ORDER BY e.aggregate_id, e.created ASC \
             ) AS first_evt \
             WHERE first_evt.aggregate_id = ts.thread_id::text \
               AND ts.source = 'trigger' \
               AND ts.trigger_id IS NULL \
               AND first_evt.trigger_id IS NOT NULL",
        )
        .execute(&self.pool)
        .await?
        .rows_affected() as usize;

        crate::core::PreferenceStore::set(
            &self.pool,
            BACKFILL_TRIGGER_ID_FROM_EVENTS_MARKER,
            "1",
        )
        .await?;
        Ok(updated)
    }

    /// Rewrite legacy `thread_summaries.trigger_id` rows that hold the v5
    /// task UUID (`trigger_id_to_uuid(config.id)`) back to the raw `config.id`.
    ///
    /// Why: pre-fix, the scheduler passed the v5 task UUID into TriggerStarted,
    /// and the projection persisted it. The dropdown filter sends `config.id`
    /// (from `/api/v1/triggers`), so the SQL filter never matched and live
    /// triggers showed zero threads.
    ///
    /// Once-only — guarded by a marker in `preferences` so subsequent boots
    /// don't re-scan `events`. Builds the {v5_hash → config.id} map from
    /// `TriggerCreated` events and rewrites matching rows in a single UPDATE.
    /// Returns the number of rows updated (0 on subsequent runs).
    pub async fn backfill_trigger_id_v5_to_config_id(
        &self,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        if crate::core::PreferenceStore::get(&self.pool, BACKFILL_TRIGGER_ID_V5_MARKER)
            .await?
            .is_some()
        {
            return Ok(0);
        }

        let config_ids: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT payload->>'trigger_id' FROM events \
             WHERE event_type = 'TriggerCreated' AND payload->>'trigger_id' IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut v5_hashes: Vec<String> = Vec::with_capacity(config_ids.len());
        let mut config_ids_paired: Vec<String> = Vec::with_capacity(config_ids.len());
        for cid in config_ids {
            let v5 = crate::scheduler::trigger_id_to_uuid(&cid).to_string();
            if v5 != cid {
                v5_hashes.push(v5);
                config_ids_paired.push(cid);
            }
        }

        let updated = if v5_hashes.is_empty() {
            0
        } else {
            sqlx::query(
                "UPDATE thread_summaries ts \
                 SET trigger_id = m.config_id \
                 FROM (SELECT unnest($1::text[]) AS v5, unnest($2::text[]) AS config_id) AS m \
                 WHERE ts.trigger_id = m.v5",
            )
            .bind(&v5_hashes)
            .bind(&config_ids_paired)
            .execute(&self.pool)
            .await?
            .rows_affected() as usize
        };

        crate::core::PreferenceStore::set(&self.pool, BACKFILL_TRIGGER_ID_V5_MARKER, "1").await?;
        Ok(updated)
    }

    /// Distinct (trigger_id, trigger_name, last_activity) tuples from every
    /// thread ever spawned by a trigger. Name and last_activity are taken from
    /// the most-recent thread per trigger_id — `last_activity` lets the UI
    /// disambiguate when several deleted triggers share a name.
    pub async fn list_historical_triggers(
        &self,
    ) -> Result<
        Vec<(String, Option<String>, chrono::DateTime<chrono::Utc>)>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let rows: Vec<(String, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
            "SELECT DISTINCT ON (trigger_id) trigger_id, trigger_name, last_activity \
             FROM thread_summaries \
             WHERE trigger_id IS NOT NULL \
             ORDER BY trigger_id, last_activity DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
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
                    trigger_id: r.trigger_id,
                    trigger_name: r.trigger_name,
                    cc_repo_id: r.cc_repo_id,
                    cc_repo_name: r.cc_repo_name,
                    state: r.state,
                    compose_text: r.compose_text,
                    compose_images: r.compose_images,
                    compose_mode: r.compose_mode,
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
    /// Returns (text, image_description, image_count) for the first user message in a thread.
    pub async fn get_thread_first_message(
        &self,
        thread_id: &str,
    ) -> Result<Option<(String, Option<String>, usize)>, Box<dyn std::error::Error + Send + Sync>>
    {
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
            let image_count = payload
                .get("user_image_hashes")
                .and_then(|v| v.as_array())
                .map_or(0, |a| a.len());
            Some((text.to_string(), image_desc, image_count))
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

            // Look up the description from the originating ChangeProposed
            // event in the events table — same payload, no separate side table.
            let description = if let Some(ref cid) = change_id {
                sqlx::query_scalar::<_, Option<String>>(
                    "SELECT payload->>'description' FROM events \
                     WHERE event_type = 'ChangeProposed' AND payload->>'change_id' = $1 \
                     ORDER BY sequence DESC LIMIT 1",
                )
                .bind(cid)
                .fetch_optional(&self.pool)
                .await?
                .flatten()
                .map(|d| d.lines().next().unwrap_or(&d).to_string())
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
        let patterns: Vec<String> = query
            .split_whitespace()
            .map(|t| format!("%{}%", super::escape_like(t)))
            .collect();
        if patterns.is_empty() {
            return Ok(vec![]);
        }
        let token_count = patterns.len() as i64;

        /// SQL column list prefixed with table alias for joins.
        const THREAD_COLS_PREFIXED: &str = "s.thread_id::text, s.title, s.first_message, s.source, s.initiator, s.created_at, s.last_activity, \
            s.message_count::bigint, s.archive_state AS section, s.active_children_count::bigint, s.total_children_count::bigint, \
            s.status, s.cc_has_changes, s.cc_requires_restart, s.cc_is_external_repo, s.cc_applying, s.last_revived_at, \
            s.parent_thread_id::text AS parent_thread_id, \
            (SELECT p.title FROM thread_summaries p WHERE p.thread_id = s.parent_thread_id) AS parent_thread_title, \
            s.trigger_id, s.trigger_name, \
            s.cc_repo_id, \
            (SELECT r.name FROM repositories r WHERE r.id = s.cc_repo_id::uuid) AS cc_repo_name, \
            s.state, s.compose_text, s.compose_images, s.compose_mode";

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
            trigger_id: Option<String>,
            trigger_name: Option<String>,
            cc_repo_id: Option<String>,
            cc_repo_name: Option<String>,
            state: String,
            compose_text: String,
            compose_images: serde_json::Value,
            compose_mode: Option<String>,
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
                        trigger_id: r.trigger_id,
                        trigger_name: r.trigger_name,
                        cc_repo_id: r.cc_repo_id,
                        cc_repo_name: r.cc_repo_name,
                        state: r.state,
                        compose_text: r.compose_text,
                        compose_images: r.compose_images,
                        compose_mode: r.compose_mode,
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
#[path = "threads_tests.rs"]
mod tests;
