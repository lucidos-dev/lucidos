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
/// and exposed on `ThreadSummary` for the frontend (`'user' | 'system'`).
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
pub struct ThreadSummary {
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
    /// Count of descendants (transitive) currently in a state that blocks this
    /// thread from being archived (per `is_blocking` in `thread_lifecycle.rs`).
    /// Maintained by EventBus on `thread_summaries.blocking_descendant_count`.
    /// Consumed by `resolve_actions` via `count > 0`; the raw count enables
    /// UI affordances like "3 sub-threads still busy".
    pub blocking_descendant_count: i64,
    /// Count of descendants (transitive) currently in a state that needs user
    /// attention (per `is_attention_needing` in `thread_lifecycle.rs`):
    /// WaitingForUserAnswer, or an in-workspace CC thread with pending changes.
    /// Strict subset of `blocking_descendant_count` — drops the `Running` case.
    /// Consumed by `display_section` via `count > 0` to bubble REVIEW to the
    /// ancestor chain even when sibling descendants are still running.
    pub attention_descendant_count: i64,
    /// Thread status: "idle", "running", or "waiting". Computed by the backend.
    pub status: String,
    /// Whether the CC branch has any diff against main on disk — pure git
    /// truth. Set by the projection on `ChangeProposed`, cleared on
    /// `ChangeApplied` / `ChangeDiscarded` / `ThreadArchived`, seeded at
    /// session bootstrap, and reconciled by the startup sweep against on-disk
    /// git state. Drives the WaitingBanner Diff button.
    pub coding_agent_has_diff: bool,
    /// CC's formal "ready for review" offer — set only by `ChangeProposed`,
    /// cleared on Apply/Discard/Archive. Drives the Apply / Discard buttons.
    /// Distinct from `coding_agent_has_diff` (the git fact): a thread can have
    /// a diff mid-session before CC has formally proposed.
    pub coding_agent_proposed: bool,
    /// Whether the proposed change requires an engine restart. Only meaningful
    /// when `coding_agent_proposed = true`; cleared together with it.
    pub coding_agent_requires_restart: bool,
    /// Whether the Claude Code session is bound to an external repo. External repos
    /// can't be Applied via the engine merge flow — the WaitingBanner shows
    /// Done/Archive instead, and `archive_thread` marks pending changes as
    /// applied so they don't sit forever.
    pub coding_agent_is_external_repo: bool,
    /// Whether a merge conflict is being resolved.
    pub coding_agent_applying: bool,
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
    /// Repository the Claude Code session bound to (only for `channel == "claude_code"`
    /// threads). Stored as TEXT on `thread_summaries.cc_repo_id`; matches a
    /// `repositories.id` UUID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_repo_id: Option<String>,
    /// Current repo display name resolved from the registry. NULL when the
    /// repo was deleted from `repositories` after the thread was bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_repo_name: Option<String>,
    /// Coding-agent thread flavor — `'lucidos' | 'app' | 'external'`. Stored
    /// on `thread_summaries.coding_agent_kind`. NULL for non-CC threads and
    /// legacy CC rows (consumers default NULL → `'lucidos'`). Frontend reads
    /// this to render the app-thread affordances (branch chip, WIP preview).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent_kind: Option<String>,
    /// Canonical folder the coding agent operates on — `<ws>/data/apps/<id>/`
    /// for App, the repo root for Lucidos and External. Stored on
    /// `thread_summaries.coding_agent_folder`. NULL for non-CC threads and
    /// legacy rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent_folder: Option<String>,
    /// Compose state machine (`composing` | `active` | `discarded`). Orthogonal
    /// to the archive flag (`archive_state` / wire field `section`): an archived
    /// thread carries `state='active'` plus `archive_state='archived'`.
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

/// SQL expression that extracts an app id from `{alias}.coding_agent_folder`.
/// App coding-agent threads operate on `<ws>/data/apps/<id>` — the id is the
/// segment immediately after `/data/apps/`. The folder resolver
/// (`coding_agent_kind::match_app_id`) refuses any deeper path, so the folder
/// always contains exactly one `/data/apps/<id>` with no sub-path; this first-
/// occurrence split therefore yields the same id the frontend `appIdFromFolder`
/// derives. Used by both the filter-facets query and the `app_ids` narrowing
/// branch of `get_older_threads` so the dropdown and the filter agree.
fn app_id_sql_expr(alias: &str) -> String {
    format!("split_part(split_part({alias}.coding_agent_folder, '/data/apps/', 2), '/', 1)")
}

/// One selectable filter facet (trigger / repo / app) with the timestamp of its
/// most-recent thread. `id` is non-null in practice (the queries filter
/// `… IS NOT NULL`) but stays `Option` to match the nullable columns.
#[derive(Serialize, sqlx::FromRow)]
pub struct FilterFacet {
    pub id: Option<String>,
    pub last_activity: Option<chrono::DateTime<chrono::Utc>>,
}

/// Complete set of selectable drawer filter facets — every trigger / repo / app
/// that has stamped at least one thread. Labels are resolved client-side.
#[derive(Serialize)]
pub struct FilterFacets {
    pub triggers: Vec<FilterFacet>,
    pub repos: Vec<FilterFacet>,
    pub apps: Vec<FilterFacet>,
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
    blocking_descendant_count: i64,
    attention_descendant_count: i64,
    status: String,
    /// Pure git truth — `git diff main..branch` is non-empty.
    coding_agent_has_diff: bool,
    /// CC's formal "ready for review" — set by `ChangeProposed` only.
    coding_agent_proposed: bool,
    /// Only meaningful when `coding_agent_proposed = true`.
    coding_agent_requires_restart: bool,
    coding_agent_is_external_repo: bool,
    coding_agent_applying: bool,
    last_revived_at: Option<chrono::DateTime<chrono::Utc>>,
    is_saved: bool,
    has_response: bool,
    parent_thread_id: Option<String>,
    parent_thread_title: Option<String>,
    trigger_id: Option<String>,
    trigger_name: Option<String>,
    cc_repo_id: Option<String>,
    cc_repo_name: Option<String>,
    coding_agent_kind: Option<String>,
    coding_agent_folder: Option<String>,
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
/// Those fields stay on `ThreadSummary` for the `/api/v1/threads` initial fetch
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
    /// Count of descendants (transitive) currently blocking this thread's
    /// archive. See `ThreadSummary::blocking_descendant_count`. Carried on the
    /// per-event SSE aggregate so the frontend can keep `meta` current
    /// without an extra GET after every relevant transition.
    pub blocking_descendant_count: i64,
    /// Count of descendants (transitive) currently in a state that needs user
    /// attention. See `ThreadSummary::attention_descendant_count`. Drives the
    /// REVIEW-bubble rule in `display_section`. Strict subset of
    /// `blocking_descendant_count` (drops the Running case). Carried on the
    /// per-event SSE aggregate alongside `blocking_descendant_count` so the
    /// frontend can recompute section without a refetch.
    pub attention_descendant_count: i64,
    /// Pure git truth — drives the Diff button. See `ThreadSummary::coding_agent_has_diff`.
    pub coding_agent_has_diff: bool,
    /// CC's formal "ready for review" offer — set by `ChangeProposed` only.
    /// Drives the Apply / Discard buttons.
    pub coding_agent_proposed: bool,
    /// Only meaningful when `coding_agent_proposed = true`.
    pub coding_agent_requires_restart: bool,
    pub coding_agent_is_external_repo: bool,
    pub coding_agent_applying: bool,
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
    /// Coding-agent thread flavor — see `ThreadSummary::coding_agent_kind`.
    /// Wire field: `codingAgentKind` (camelCase via the struct-level rename).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent_kind: Option<String>,
    /// Canonical folder — see `ThreadSummary::coding_agent_folder`.
    /// Wire field: `codingAgentFolder` (camelCase via the struct-level rename).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent_folder: Option<String>,
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
        THREAD_COLS.as_str(),
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
        blocking_descendant_count: r.blocking_descendant_count,
        attention_descendant_count: r.attention_descendant_count,
        coding_agent_has_diff: r.coding_agent_has_diff,
        coding_agent_proposed: r.coding_agent_proposed,
        coding_agent_requires_restart: r.coding_agent_requires_restart,
        coding_agent_is_external_repo: r.coding_agent_is_external_repo,
        coding_agent_applying: r.coding_agent_applying,
        is_saved: r.is_saved,
        has_response: r.has_response,
        last_revived_at: r.last_revived_at,
        parent_thread_id: r.parent_thread_id,
        parent_thread_title: r.parent_thread_title,
        trigger_id: r.trigger_id,
        trigger_name: r.trigger_name,
        cc_repo_id: r.cc_repo_id,
        cc_repo_name: r.cc_repo_name,
        coding_agent_kind: r.coding_agent_kind,
        coding_agent_folder: r.coding_agent_folder,
        state: r.state,
    })
}

/// SQL column list for thread summary queries, qualified with `alias` so the
/// same shape works whether the caller's FROM clause aliases the table as
/// `t` (the default — `THREAD_COLS` below) or `s` (the search query that
/// joins `best_scores b` against `thread_summaries s`).
///
/// Both correlated subqueries hit a PK b-tree (`thread_summaries.thread_id`,
/// `repositories.id`) and short-circuit when the source FK is NULL. The cast
/// is on the FK side (`{alias}.cc_repo_id::uuid`) — casting `r.id::text`
/// instead would prevent index use. Rows whose `cc_repo_id` no longer matches
/// a row in `repositories` get NULL `cc_repo_name` and the frontend renders
/// them as `(deleted)`.
fn thread_cols(alias: &str) -> String {
    format!(
        "{a}.thread_id::text, {a}.title, {a}.first_message, {a}.source, {a}.initiator, {a}.created_at, {a}.last_activity, \
        {a}.message_count::bigint, {a}.archive_state AS section, {a}.active_children_count::bigint, {a}.total_children_count::bigint, \
        {a}.blocking_descendant_count::bigint, {a}.attention_descendant_count::bigint, \
        {a}.status, {a}.coding_agent_has_diff, {a}.coding_agent_proposed, {a}.coding_agent_requires_restart, \
        {a}.coding_agent_is_external_repo, {a}.coding_agent_applying, {a}.last_revived_at, \
        {a}.is_saved, {a}.has_response, \
        {a}.parent_thread_id::text AS parent_thread_id, \
        (SELECT p.title FROM thread_summaries p WHERE p.thread_id = {a}.parent_thread_id) AS parent_thread_title, \
        {a}.trigger_id, {a}.trigger_name, \
        {a}.cc_repo_id, \
        (SELECT r.name FROM repositories r WHERE r.id = {a}.cc_repo_id::uuid) AS cc_repo_name, \
        {a}.coding_agent_kind, {a}.coding_agent_folder, \
        {a}.state, {a}.compose_text, {a}.compose_images, {a}.compose_mode",
        a = alias,
    )
}

/// Default `thread_cols("t")` for the common `FROM thread_summaries t` shape.
/// Cached because every list/get query path formats SQL with it.
static THREAD_COLS: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| thread_cols("t"));

impl EventStore {
    fn rows_to_thread_summaries(
        rows: Vec<ThreadRow>,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        rows.into_iter()
            .map(|r| {
                Ok(ThreadSummary {
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
                    blocking_descendant_count: r.blocking_descendant_count,
                    attention_descendant_count: r.attention_descendant_count,
                    status: r.status,
                    coding_agent_has_diff: r.coding_agent_has_diff,
                    coding_agent_proposed: r.coding_agent_proposed,
                    coding_agent_requires_restart: r.coding_agent_requires_restart,
                    coding_agent_is_external_repo: r.coding_agent_is_external_repo,
                    coding_agent_applying: r.coding_agent_applying,
                    last_revived_at: r.last_revived_at,
                    parent_thread_id: r.parent_thread_id,
                    parent_thread_title: r.parent_thread_title,
                    trigger_id: r.trigger_id,
                    trigger_name: r.trigger_name,
                    cc_repo_id: r.cc_repo_id,
                    cc_repo_name: r.cc_repo_name,
                    coding_agent_kind: r.coding_agent_kind,
                    coding_agent_folder: r.coding_agent_folder,
                    state: r.state,
                    compose_text: r.compose_text,
                    compose_images: r.compose_images,
                    compose_mode: r.compose_mode,
                })
            })
            .collect()
    }
}

/// Search result for a thread, with a relevance score.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadSearchResult {
    #[serde(flatten)]
    pub info: ThreadSummary,
    pub score: f64,
}

/// Filters for the script/trigger-facing `list`/`count` query paths.
/// Mirrors the wire query params of `GET /api/v1/threads/list`,
/// `lucidos threads list`, and the `list_threads` LLM tool.
///
/// `active` semantics — when `Some(true)`, restricts to statuses where
/// the agentic loop is mid-flow (`running`, `waiting_for_user_answer`).
/// `waiting` is *not* included: it means CC has stopped and proposed
/// changes the user must act on — work has paused, the loop isn't
/// running. `failed` is also excluded — the response is over.
/// `Some(false)` inverts; `None` is no filter.
pub struct ThreadSummaryFilters<'a> {
    pub active: Option<bool>,
    pub sources: Option<&'a [String]>,
    pub limit: i64,
}

/// Statuses considered "active" — the agentic loop is mid-flow.
pub fn active_thread_statuses() -> [&'static str; 2] {
    [
        ThreadStatus::Running.as_str(),
        ThreadStatus::WaitingForUserAnswer.as_str(),
    ]
}

/// A timeline event rendered inline in a thread view.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadTimelineEvent {
    pub event_type: String,
    pub timestamp: String,
    pub description: Option<String>,
    pub change_id: Option<String>,
}

mod backfill;
mod events;
mod search;
mod summaries;

#[cfg(test)]
#[path = "../threads_tests/helpers.rs"]
mod test_helpers;

#[cfg(test)]
#[path = "../threads_tests/summaries.rs"]
mod summaries_tests;

#[cfg(test)]
#[path = "../threads_tests/filtering.rs"]
mod filtering_tests;

#[cfg(test)]
#[path = "../threads_tests/backfill.rs"]
mod backfill_tests;

#[cfg(test)]
#[path = "../threads_tests/search.rs"]
mod search_tests;

#[cfg(test)]
#[path = "../threads_tests/extraction.rs"]
mod extraction_tests;
