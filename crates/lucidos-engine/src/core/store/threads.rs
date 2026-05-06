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
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use sqlx::PgPool;
    use uuid::Uuid;

    /// Insert a parent thread and a child thread referencing it. Returns
    /// `(parent_id, child_id)`. `child_saved` controls whether the child
    /// is_saved (the parent is never saved). Both have has_response=TRUE
    /// so they show up in get_recent_threads / get_older_threads.
    async fn insert_parent_child(pool: &PgPool, child_saved: bool) -> (Uuid, Uuid) {
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries (thread_id, title, source, message_count, last_activity, has_response, is_saved, parent_thread_id) \
             VALUES ($1, 'Parent thread', 'chat', 1, NOW(), TRUE, FALSE, NULL), \
                    ($2, 'Child thread',  'chat', 1, NOW(), TRUE, $3,   $1)"
        )
        .bind(parent)
        .bind(child)
        .bind(child_saved)
        .execute(pool)
        .await
        .expect("insert thread_summaries");
        (parent, child)
    }

    #[tokio::test]
    async fn get_saved_threads_resolves_parent_title() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());
        let (parent, child) = insert_parent_child(&pool, true).await;

        let saved = store
            .get_saved_threads()
            .await
            .expect("get_saved_threads");

        let row = saved
            .iter()
            .find(|t| t.thread_id == child.to_string())
            .expect("child thread should appear in saved");
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
            .get_older_threads(cutoff, 10, None, None, None)
            .await
            .expect("get_older_threads");

        let row = older
            .iter()
            .find(|t| t.thread_id == child.to_string())
            .expect("child thread should appear in older");
        assert_eq!(row.parent_thread_title.as_deref(), Some("Parent thread"));

        teardown_test_db(&db).await;
    }

    /// Insert a trigger-source thread with the given trigger_id/trigger_name and
    /// last_activity offset. Returns the new thread id. has_response=TRUE so the
    /// thread surfaces in `get_older_threads`.
    async fn insert_trigger_thread(
        pool: &PgPool,
        trigger_id: &str,
        trigger_name: &str,
        minutes_ago: i64,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, is_saved, trigger_id, trigger_name) \
             VALUES ($1, 'T', 'trigger', 1, NOW() - ($2 || ' minutes')::interval, TRUE, FALSE, $3, $4)",
        )
        .bind(id)
        .bind(minutes_ago.to_string())
        .bind(trigger_id)
        .bind(trigger_name)
        .execute(pool)
        .await
        .expect("insert trigger thread");
        id
    }

    /// `list_historical_triggers` returns one entry per distinct trigger_id with
    /// the most-recent thread's snapshot name and last_activity (covers the
    /// trigger-rename case and powers the dropdown's `(until <date>)` suffix).
    #[tokio::test]
    async fn list_historical_triggers_dedupes_and_takes_most_recent_name() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        insert_trigger_thread(&pool, "trig-a", "Apple (old name)", 60).await;
        let trig_a_recent = insert_trigger_thread(&pool, "trig-a", "Apple", 1).await;
        let trig_b_recent = insert_trigger_thread(&pool, "trig-b", "Banana", 30).await;

        let mut historical = store
            .list_historical_triggers()
            .await
            .expect("list_historical_triggers");
        historical.sort_by(|a, b| a.0.cmp(&b.0));

        let names: Vec<_> = historical
            .iter()
            .map(|(id, name, _)| (id.clone(), name.clone()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("trig-a".to_string(), Some("Apple".to_string())),
                ("trig-b".to_string(), Some("Banana".to_string())),
            ]
        );

        let last_a = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT last_activity FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(trig_a_recent)
        .fetch_one(&pool)
        .await
        .expect("fetch last_activity for trig-a");
        let last_b = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT last_activity FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(trig_b_recent)
        .fetch_one(&pool)
        .await
        .expect("fetch last_activity for trig-b");
        assert_eq!(historical[0].2, last_a);
        assert_eq!(historical[1].2, last_b);

        teardown_test_db(&db).await;
    }

    /// When `trigger_ids` is provided, `get_older_threads` returns only matching threads.
    #[tokio::test]
    async fn get_older_threads_filters_by_trigger_ids() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let a1 = insert_trigger_thread(&pool, "trig-a", "Apple", 60).await;
        let _a2 = insert_trigger_thread(&pool, "trig-a", "Apple", 30).await;
        let _b1 = insert_trigger_thread(&pool, "trig-b", "Banana", 20).await;

        let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
        let only_a = store
            .get_older_threads(cutoff, 10, None, Some(&["trig-a".to_string()]), None)
            .await
            .expect("get_older_threads filtered");

        assert_eq!(only_a.len(), 2);
        assert!(only_a.iter().all(|t| t.trigger_id.as_deref() == Some("trig-a")));
        assert!(only_a.iter().any(|t| t.thread_id == a1.to_string()));

        teardown_test_db(&db).await;
    }

    /// Trigger-id filter returns matches regardless of `has_response`. The
    /// dropdown advertises every trigger that ever stamped a row, with no
    /// `has_response` gate; the filter must honor the same contract.
    #[tokio::test]
    async fn get_older_threads_returns_trigger_threads_with_no_response() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, is_saved, trigger_id, trigger_name) \
             VALUES ($1, 'orphan', 'trigger', 1, NOW() - INTERVAL '60 minutes', FALSE, FALSE, 'trig-orphan', 'Orphan')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .expect("insert no-response trigger thread");

        let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
        let hits = store
            .get_older_threads(cutoff, 10, None, Some(&["trig-orphan".to_string()]), None)
            .await
            .expect("get_older_threads filtered");

        assert_eq!(
            hits.len(),
            1,
            "dropdown advertised trig-orphan; filter must return its thread regardless of has_response"
        );
        assert_eq!(hits[0].thread_id, id.to_string());

        teardown_test_db(&db).await;
    }

    /// Insert a CC-source thread bound to the given repo UUID with the given
    /// last_activity offset. Returns the new thread id.
    async fn insert_cc_repo_thread(
        pool: &PgPool,
        repo_id: &str,
        minutes_ago: i64,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, is_saved, cc_repo_id) \
             VALUES ($1, 'CC', 'claude_code', 1, NOW() - ($2 || ' minutes')::interval, TRUE, FALSE, $3)",
        )
        .bind(id)
        .bind(minutes_ago.to_string())
        .bind(repo_id)
        .execute(pool)
        .await
        .expect("insert cc repo thread");
        id
    }

    /// Register a repo in the `repositories` table so `cc_repo_name` resolves.
    async fn insert_repository(pool: &PgPool, repo_id: Uuid, name: &str, path: &str) {
        sqlx::query(
            "INSERT INTO repositories (id, name, path) VALUES ($1, $2, $3)",
        )
        .bind(repo_id)
        .bind(name)
        .bind(path)
        .execute(pool)
        .await
        .expect("insert repository");
    }

    /// `repo_ids` narrows `get_older_threads` to CC threads bound to those
    /// repos and projects `cc_repo_name` from the `repositories` registry.
    #[tokio::test]
    async fn get_older_threads_filters_by_repo_ids() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let repo_a = Uuid::new_v4();
        let repo_b = Uuid::new_v4();
        insert_repository(&pool, repo_a, "Apple", "/tmp/apple").await;
        insert_repository(&pool, repo_b, "Banana", "/tmp/banana").await;

        let a1 = insert_cc_repo_thread(&pool, &repo_a.to_string(), 60).await;
        let _a2 = insert_cc_repo_thread(&pool, &repo_a.to_string(), 30).await;
        let _b1 = insert_cc_repo_thread(&pool, &repo_b.to_string(), 20).await;

        let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
        let only_a = store
            .get_older_threads(cutoff, 10, None, None, Some(&[repo_a.to_string()]))
            .await
            .expect("get_older_threads filtered");

        assert_eq!(only_a.len(), 2);
        assert!(only_a.iter().all(|t| t.cc_repo_id.as_deref() == Some(repo_a.to_string().as_str())));
        assert!(only_a.iter().all(|t| t.cc_repo_name.as_deref() == Some("Apple")));
        assert!(only_a.iter().any(|t| t.thread_id == a1.to_string()));

        teardown_test_db(&db).await;
    }

    /// When the registered repo is later deleted, threads bound to its UUID
    /// keep `cc_repo_id` but `cc_repo_name` resolves to NULL — the frontend
    /// uses that absence to render the row as `(deleted)`.
    #[tokio::test]
    async fn get_older_threads_returns_null_repo_name_for_deleted_repo() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let orphan_repo = Uuid::new_v4();
        insert_cc_repo_thread(&pool, &orphan_repo.to_string(), 60).await;

        let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
        let hits = store
            .get_older_threads(cutoff, 10, None, None, Some(&[orphan_repo.to_string()]))
            .await
            .expect("get_older_threads filtered");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].cc_repo_id.as_deref(), Some(orphan_repo.to_string().as_str()));
        assert_eq!(hits[0].cc_repo_name, None, "deleted repo must yield NULL name");

        teardown_test_db(&db).await;
    }

    /// `trigger_ids` and `repo_ids` compose with OR — a user with both
    /// filters expanded sees the union.
    #[tokio::test]
    async fn get_older_threads_combines_trigger_and_repo_ids_with_or() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let repo_a = Uuid::new_v4();
        insert_repository(&pool, repo_a, "Apple", "/tmp/apple").await;

        let cc_thread = insert_cc_repo_thread(&pool, &repo_a.to_string(), 30).await;
        let trig_thread = insert_trigger_thread(&pool, "trig-a", "Trig A", 60).await;
        insert_trigger_thread(&pool, "trig-other", "Other", 90).await;

        let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
        let hits = store
            .get_older_threads(
                cutoff,
                10,
                None,
                Some(&["trig-a".to_string()]),
                Some(&[repo_a.to_string()]),
            )
            .await
            .expect("get_older_threads combined");

        assert_eq!(hits.len(), 2);
        let returned: std::collections::HashSet<&str> =
            hits.iter().map(|t| t.thread_id.as_str()).collect();
        let cc = cc_thread.to_string();
        let trig = trig_thread.to_string();
        assert!(returned.contains(cc.as_str()));
        assert!(returned.contains(trig.as_str()));

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn backfill_trigger_id_rewrites_v5_hashes_to_config_ids() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let live_config_id = "5633f3e1-110c-4df4-a6fc-c0df8fd36df4";
        let v5_hash = crate::scheduler::trigger_id_to_uuid(live_config_id).to_string();
        let untouched_config_id = "08f22aed-ab0f-498d-83d7-2d7e420141ff";

        sqlx::query(
            "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
             VALUES ($1, 'TriggerCreated', $2, 'trigger', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(serde_json::json!({"trigger_id": live_config_id, "name": "Job Listing Check"}))
        .bind(live_config_id)
        .execute(&pool)
        .await
        .expect("insert TriggerCreated");

        let legacy = insert_trigger_thread(&pool, &v5_hash, "Job Listing Check", 60).await;
        let already_correct =
            insert_trigger_thread(&pool, untouched_config_id, "Check Bank Balance", 60).await;
        let orphan_v5 = insert_trigger_thread(
            &pool,
            "deadbeef-dead-5eed-dead-deaddeaddead",
            "Some deleted trigger",
            60,
        )
        .await;

        let updated = store
            .backfill_trigger_id_v5_to_config_id()
            .await
            .expect("backfill");
        assert_eq!(updated, 1, "exactly one row had a known v5 hash");

        let legacy_after: String =
            sqlx::query_scalar("SELECT trigger_id FROM thread_summaries WHERE thread_id = $1")
                .bind(legacy)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(legacy_after, live_config_id);

        let untouched_after: String =
            sqlx::query_scalar("SELECT trigger_id FROM thread_summaries WHERE thread_id = $1")
                .bind(already_correct)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(untouched_after, untouched_config_id);

        let orphan_after: String =
            sqlx::query_scalar("SELECT trigger_id FROM thread_summaries WHERE thread_id = $1")
                .bind(orphan_v5)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            orphan_after, "deadbeef-dead-5eed-dead-deaddeaddead",
            "v5 hash with no matching TriggerCreated stays as-is"
        );

        let second = store
            .backfill_trigger_id_v5_to_config_id()
            .await
            .expect("idempotent");
        assert_eq!(second, 0, "second run touches nothing");

        teardown_test_db(&db).await;
    }

    /// Insert a trigger-source thread row with NULL trigger_id/trigger_name —
    /// the state every legacy thread is in after the broken
    /// `20260429214800_addtriggeridtothreadsummaries.sql` backfill, which only
    /// reads `payload->>'trigger_id'` and skips the legacy `task_id`/`task_name`
    /// pair. Returns the new thread id.
    async fn insert_null_trigger_thread(pool: &PgPool, minutes_ago: i64) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries \
             (thread_id, title, source, message_count, last_activity, has_response, is_saved, trigger_id, trigger_name) \
             VALUES ($1, 'T', 'trigger', 1, NOW() - ($2 || ' minutes')::interval, TRUE, FALSE, NULL, NULL)",
        )
        .bind(id)
        .bind(minutes_ago.to_string())
        .execute(pool)
        .await
        .expect("insert null-trigger thread");
        id
    }

    /// Insert a `TriggerStarted` event for the given thread with a raw payload.
    /// Lets the test mimic legacy (`task_id`) vs modern (`trigger_id`) shapes.
    async fn insert_trigger_started_event(
        pool: &PgPool,
        thread_id: Uuid,
        payload: serde_json::Value,
    ) {
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id, thread_id) \
             VALUES ($1, 'TriggerStarted', $2, 'thread', $3, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(payload)
        .bind(thread_id.to_string())
        .bind(thread_id)
        .execute(pool)
        .await
        .expect("insert TriggerStarted");
    }

    /// Regression for the work-workspace bug where every trigger thread
    /// rendered with NULL `trigger_id` because the
    /// `20260429214800_addtriggeridtothreadsummaries.sql` backfill only read
    /// `payload->>'trigger_id'` and ignored legacy events that stored the id
    /// under `task_id`. The runtime backfill below recovers the value from
    /// `events`, COALESCEing both shapes.
    #[tokio::test]
    async fn backfill_trigger_id_from_events_reads_legacy_task_id() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let legacy = insert_null_trigger_thread(&pool, 90).await;
        insert_trigger_started_event(
            &pool,
            legacy,
            serde_json::json!({
                "task_id": "364d689e-0620-5712-9739-c9ceb1d12fe1",
                "task_name": "Legacy Trigger",
                "channel": "trigger",
            }),
        )
        .await;

        let modern = insert_null_trigger_thread(&pool, 60).await;
        insert_trigger_started_event(
            &pool,
            modern,
            serde_json::json!({
                "trigger_id": "a969c963-dbc0-4f5f-8ebb-58c7f2b80c96",
                "trigger_name": "Modern Trigger",
                "channel": "trigger",
            }),
        )
        .await;

        // Already-set trigger_id must NOT be overwritten — the runtime
        // projection populated it, so events would just confirm what's there.
        let preset = insert_trigger_thread(&pool, "preset-id", "Preset Name", 30).await;
        insert_trigger_started_event(
            &pool,
            preset,
            serde_json::json!({
                "trigger_id": "different-id-should-be-ignored",
                "trigger_name": "Different Name",
            }),
        )
        .await;

        // Trigger-source thread with no TriggerStarted event in `events`
        // (corruption / lost event). Must stay NULL — never invent values.
        let orphan = insert_null_trigger_thread(&pool, 20).await;

        let updated = store
            .backfill_trigger_id_from_events()
            .await
            .expect("backfill_trigger_id_from_events");
        assert_eq!(updated, 2, "two NULL-trigger rows had matching events");

        let (legacy_id, legacy_name) = fetch_trigger_pair(&pool, legacy).await;
        assert_eq!(
            legacy_id.as_deref(),
            Some("364d689e-0620-5712-9739-c9ceb1d12fe1"),
            "legacy task_id must be COALESCEd into trigger_id"
        );
        assert_eq!(legacy_name.as_deref(), Some("Legacy Trigger"));

        let (modern_id, modern_name) = fetch_trigger_pair(&pool, modern).await;
        assert_eq!(
            modern_id.as_deref(),
            Some("a969c963-dbc0-4f5f-8ebb-58c7f2b80c96")
        );
        assert_eq!(modern_name.as_deref(), Some("Modern Trigger"));

        let (preset_id, preset_name) = fetch_trigger_pair(&pool, preset).await;
        assert_eq!(
            preset_id.as_deref(),
            Some("preset-id"),
            "row that already had trigger_id must not be overwritten"
        );
        assert_eq!(preset_name.as_deref(), Some("Preset Name"));

        let (orphan_id, orphan_name) = fetch_trigger_pair(&pool, orphan).await;
        assert_eq!(orphan_id, None, "no event = no value invented");
        assert_eq!(orphan_name, None);

        let second = store
            .backfill_trigger_id_from_events()
            .await
            .expect("idempotent");
        assert_eq!(second, 0, "second run touches nothing (marker set)");

        teardown_test_db(&db).await;
    }

    async fn fetch_trigger_pair(pool: &PgPool, thread_id: Uuid) -> (Option<String>, Option<String>) {
        sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT trigger_id, trigger_name FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("fetch trigger pair")
    }

    /// Reproduce the original work-workspace bug end-to-end: a legacy event
    /// with `task_id` set to the v5 hash of `config.id`, and a NULL row in
    /// `thread_summaries`. After both backfills run in startup order the
    /// dropdown filter (which sends `config.id`) must match.
    #[tokio::test]
    async fn both_backfills_compose_legacy_task_id_to_config_id() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let config_id = "a969c963-dbc0-4f5f-8ebb-58c7f2b80c96";
        let v5_hash = crate::scheduler::trigger_id_to_uuid(config_id).to_string();

        sqlx::query(
            "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
             VALUES ($1, 'TriggerCreated', $2, 'trigger', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(serde_json::json!({"trigger_id": config_id, "name": "UA Analysis Runner"}))
        .bind(config_id)
        .execute(&pool)
        .await
        .expect("insert TriggerCreated");

        let thread = insert_null_trigger_thread(&pool, 60).await;
        insert_trigger_started_event(
            &pool,
            thread,
            serde_json::json!({
                "task_id": v5_hash,
                "task_name": "UA Analysis Runner",
            }),
        )
        .await;

        let from_events = store.backfill_trigger_id_from_events().await.expect("step 1");
        assert_eq!(from_events, 1);
        let v5_to_cfg = store
            .backfill_trigger_id_v5_to_config_id()
            .await
            .expect("step 2");
        assert_eq!(v5_to_cfg, 1);

        let (final_id, _) = fetch_trigger_pair(&pool, thread).await;
        assert_eq!(
            final_id.as_deref(),
            Some(config_id),
            "legacy task_id (v5 hash) must end up as the live config.id so the dropdown filter matches"
        );

        teardown_test_db(&db).await;
    }

    /// `get_recent_threads` must surface every thread that NEEDS user action
    /// (`cc_has_changes=TRUE`, `status='waiting_for_user_answer'`, `status='failed'`)
    /// even when the per-source `rn <= per_source` window would otherwise drop it.
    ///
    /// REVIEW is a "needs attention" pile. Without this guarantee, a CC thread
    /// pushed past the per-source window vanishes from the drawer entirely —
    /// the user has no way to Apply/Discard the changes, no way to see them in
    /// REVIEW, no Diff button. The `changes` data still exists in the DB but
    /// the thread carrying it is invisible until the user manually scrolls
    /// far enough to trigger `get_older_threads`.
    ///
    /// Regression: 2026-04-25 dev workspace had four CC threads with pending
    /// changes at rn=17, 18, 19, 40 — all hidden from /api/threads.
    #[tokio::test]
    async fn get_recent_threads_always_includes_actionable_threads_beyond_window() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        // 18 CC threads with descending last_activity. The three at i=15..17
        // carry actionable signals — each picks an inert status so the only
        // thing that lets it bypass the rn<=15 cap is the predicate under
        // test: cc_has_changes (#15), waiting_for_user_answer (#16),
        // failed (#17). One distinct second per row stabilizes the ranking.
        let now = chrono::Utc::now();
        let mut ids = Vec::with_capacity(18);
        for i in 0..18 {
            let id = Uuid::new_v4();
            ids.push(id);
            let last_activity = now - chrono::Duration::seconds(i as i64);
            let (status, cc_has_changes, section) = match i {
                15 => (ThreadStatus::Idle.as_str(), true, "inbox"),
                16 => (ThreadStatus::WaitingForUserAnswer.as_str(), false, "inbox"),
                17 => (ThreadStatus::Failed.as_str(), false, "inbox"),
                _ => (ThreadStatus::Idle.as_str(), false, "archived"),
            };
            sqlx::query(
                "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, cc_has_changes, archive_state) \
                 VALUES ($1, $2, 'claude_code', 1, $3, TRUE, $4, $5, $6)",
            )
            .bind(id)
            .bind(format!("Thread {}", i))
            .bind(last_activity)
            .bind(status)
            .bind(cc_has_changes)
            .bind(section)
            .execute(&pool)
            .await
            .expect("insert thread_summaries");
        }
        let pending_changes = ids[15];
        let needs_answer = ids[16];
        let failed = ids[17];

        let recent = store
            .get_recent_threads(15)
            .await
            .expect("get_recent_threads");

        let returned: std::collections::HashSet<&str> =
            recent.iter().map(|t| t.thread_id.as_str()).collect();
        let pending = pending_changes.to_string();
        let answer = needs_answer.to_string();
        let fail = failed.to_string();
        assert!(
            returned.contains(pending.as_str()),
            "thread with cc_has_changes=TRUE at rn>per_source must surface (Apply/Discard buttons live here); returned {} entries",
            recent.len()
        );
        assert!(
            returned.contains(answer.as_str()),
            "thread with status=waiting_for_user_answer at rn>per_source must surface (Question card lives here); returned {} entries",
            recent.len()
        );
        assert!(
            returned.contains(fail.as_str()),
            "thread with status=failed at rn>per_source must surface (error indicator lives here); returned {} entries",
            recent.len()
        );

        teardown_test_db(&db).await;
    }

    /// REVIEW must contain every inbox thread, not just the top-N per source.
    /// An inbox row is one the user hasn't dismissed; capping it would silently
    /// hide work — e.g. a CC thread whose subprocess crashed mid-flow without
    /// emitting a terminal event keeps `cc_has_changes=false` and would be
    /// gated out solely by recency.
    #[tokio::test]
    async fn get_recent_threads_returns_all_inbox_threads_beyond_window() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        // 20 inert idle inbox CC threads. None carry an actionable signal,
        // so the only thing that can surface row 19 (rn=20, past the window
        // of 15) is the inbox bypass under test.
        let now = chrono::Utc::now();
        let mut ids = Vec::with_capacity(20);
        for i in 0..20 {
            let id = Uuid::new_v4();
            ids.push(id);
            let last_activity = now - chrono::Duration::seconds(i as i64);
            sqlx::query(
                "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, cc_has_changes, archive_state) \
                 VALUES ($1, $2, 'claude_code', 1, $3, TRUE, 'idle', FALSE, 'inbox')",
            )
            .bind(id)
            .bind(format!("Inbox thread {}", i))
            .bind(last_activity)
            .execute(&pool)
            .await
            .expect("insert thread_summaries");
        }
        let furthest_back = ids[19];

        let recent = store
            .get_recent_threads(15)
            .await
            .expect("get_recent_threads");

        let returned: std::collections::HashSet<&str> =
            recent.iter().map(|t| t.thread_id.as_str()).collect();
        let needed = furthest_back.to_string();
        assert!(
            returned.contains(needed.as_str()),
            "inbox thread at rn>per_source must surface; got {} entries",
            recent.len()
        );
        assert_eq!(
            recent.len(),
            20,
            "all 20 inbox threads must appear; got {}",
            recent.len()
        );

        teardown_test_db(&db).await;
    }

    /// History (archived threads) stays capped per source so the drawer
    /// doesn't load the whole archive on refresh; `get_older_threads` pages
    /// backward through what this omits.
    #[tokio::test]
    async fn get_recent_threads_caps_archived_threads_at_per_source() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        // 20 archived idle chats with no actionable signal — only the top 15
        // per source should come back.
        let now = chrono::Utc::now();
        for i in 0..20 {
            let last_activity = now - chrono::Duration::seconds(i as i64);
            sqlx::query(
                "INSERT INTO thread_summaries \
                 (thread_id, title, source, message_count, last_activity, has_response, \
                  status, cc_has_changes, archive_state) \
                 VALUES ($1, $2, 'chat', 1, $3, TRUE, 'idle', FALSE, 'archived')",
            )
            .bind(Uuid::new_v4())
            .bind(format!("Archived chat {}", i))
            .bind(last_activity)
            .execute(&pool)
            .await
            .expect("insert thread_summaries");
        }

        let recent = store
            .get_recent_threads(15)
            .await
            .expect("get_recent_threads");
        let chat_count = recent
            .iter()
            .filter(|t| t.channel == "chat")
            .count();
        assert_eq!(
            chat_count, 15,
            "archived threads must stay capped at per_source; got {}",
            chat_count
        );

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
        insert_thread(&pool, phrase_in_title, "Bil reparasjon").await;

        let split_across_events = Uuid::new_v4();
        insert_thread(&pool, split_across_events, "Verkstedet timeavtale").await;
        insert_message(&pool, split_across_events, "MessageReceived", "min bil til service").await;
        insert_message(
            &pool,
            split_across_events,
            "MessageReceived",
            "reparasjonen er ferdig om to dager",
        )
        .await;

        let only_one_token = Uuid::new_v4();
        insert_thread(&pool, only_one_token, "Bil mappa dokumenter").await;
        insert_message(&pool, only_one_token, "MessageReceived", "fant fram dokumentene").await;

        let irrelevant = Uuid::new_v4();
        insert_thread(&pool, irrelevant, "Varmepumpe logging").await;
        insert_message(&pool, irrelevant, "MessageReceived", "varmepumpe styring").await;

        let results = store
            .search_threads_by_text("bil reparasjon", 20)
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
            "thread missing the 'reparasjon' token must NOT match. ids={:?}",
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
        insert_message(&pool, thread, "MessageReceived", "regnr bil").await;
        insert_message(&pool, thread, "ResponseGenerated", "Ola Hansen (eier)").await;
        insert_message(&pool, thread, "MessageReceived", "tlf til verkstedet").await;

        let ctx = store
            .recent_thread_messages_for_extraction(thread, 5, None)
            .await
            .expect("get context");

        assert!(ctx.contains("regnr bil"), "ctx={}", ctx);
        assert!(ctx.contains("Ola Hansen (eier)"), "ctx={}", ctx);
        let first_pos = ctx.find("regnr bil").unwrap();
        let second_pos = ctx.find("Ola Hansen").unwrap();
        assert!(first_pos < second_pos, "oldest first; ctx={}", ctx);

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
        .bind(serde_json::json!({"text": "ringer verkstedet om servicen i morgen"}))
        .bind(thread)
        .execute(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO memory_entries (id, source, topic, summary, importance, entities, embedding, embedding_model, src_created_at, created_at) \
             VALUES ($1, $2::jsonb, 'vehicle service', 'Bilen trenger service', 0.9, $3::jsonb, $4::vector, 'multilingual-e5-small', NOW(), NOW())"
        )
        .bind(Uuid::new_v4())
        .bind(serde_json::json!({"type": "event", "id": event_id}))
        .bind(serde_json::json!(["bil", "service", "verksted"]))
        .bind(format!("[{}]", vec!["0"; 384].join(",")))
        .execute(&pool).await.unwrap();

        let results = store.search_threads_by_text("bil service", 20).await.unwrap();
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
