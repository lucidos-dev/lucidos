use super::thread_hold::ThreadHoldSummary;
use super::types::SessionMessage;
use super::EventStore;
use crate::core::event_subscription::EventSubscription;
use crate::core::EventRow;
use crate::engine::thread_events::CacheKeepAliveStatus;
use crate::engine::thread_lifecycle::{
    ArchiveState, ChangeStateKind, CodingAgentChangeState, ThreadStatus,
};
use crate::engine::thread_state::ThreadState;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventWaitSummary {
    pub wait_id: uuid::Uuid,
    pub on: Vec<EventSubscription>,
    pub reason: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_timing: Option<crate::engine::thread_events::BackgroundWaitTiming>,
}

pub fn format_display_title(title: Option<String>, first_message: Option<String>) -> String {
    title.unwrap_or_else(|| {
        let msg = first_message.unwrap_or_default();
        if msg.chars().count() > 40 {
            format!("{}...", msg.chars().take(37).collect::<String>())
        } else {
            msg
        }
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ThreadSummary {
    pub thread_id: String,
    pub title: String,
    pub channel: String,
    pub initiator: LegacyInitiator,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_activity: chrono::DateTime<chrono::Utc>,
    pub last_user_action: chrono::DateTime<chrono::Utc>,
    pub last_agent_action: chrono::DateTime<chrono::Utc>,
    pub message_count: i64,
    pub saved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin_folder: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub home: bool,
    pub section: String,
    pub active_children_count: i64,
    pub awaited_children_count: i64,
    pub awaited_by_parent: bool,
    /// INVARIANT: Total number of child threads (active + completed).
    pub total_children_count: i64,
    pub live_event_wait_count: i64,
    /// INVARIANT: The same waits the count counts, spelled out: one entry per unresolved
    /// wait, oldest first. This is what the waiting indicator renders, so
    /// the count says how many and this says which.
    ///
    /// Carried on the summary for one reason: the client used to build this
    /// list ONLY by folding a thread's own `EventWait*` events, and nothing
    /// reconciled it against the server. A single missed `EventWaitDelivered`
    /// stranded a resolved wait on screen with a live countdown, permanently.
    /// The two columns are written by one `UPDATE` per projection arm, and the
    /// count is derived from this array's length, so they cannot disagree.
    pub live_event_waits: Vec<EventWaitSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_keep_alive_status: Option<CacheKeepAliveStatus>,
    pub blocking_descendant_count: i64,
    pub attention_descendant_count: i64,
    pub archivable_sub_thread_count: i64,
    pub is_stopped_child: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<ThreadHoldSummary>,
    pub read_requested: bool,
    pub reply_seen_devices: Vec<String>,
    pub apply_in_flight: bool,
    pub status: ThreadStatus,
    /// INVARIANT: The row's version, one higher after every change to it. A trigger owns
    /// the column, so two reads with the same version saw the same row. The
    /// client never applies a summary older than the one it holds.
    pub summary_version: i64,
    pub coding_agent_change_state: CodingAgentChangeState,
    /// INVARIANT: Whether the coding-agent thread is bound to an external repo. External repos
    /// can't be Applied via the engine merge flow — the WaitingBanner shows
    /// Done/Archive instead, and `archive_thread` marks pending changes as
    /// applied so they don't sit forever.
    pub coding_agent_is_external_repo: bool,
    /// INVARIANT: When the thread last entered the 'running' state (for IN PROGRESS sort order).
    pub last_revived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub parent_thread_id: Option<String>,
    pub parent_thread_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub successor: Option<SuccessorLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub successor_of: Option<SuccessorLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_repo_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_repo_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent_folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent: Option<String>,
    pub coding_agents: Vec<String>,
    pub state: ThreadState,
    pub compose_text: String,
    pub compose_images: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose_selection: Option<serde_json::Value>,
    /// INVARIANT: The thread's *compose epoch* (`docs/glossary.md`): the generation advanced by
    /// each submission or Stop restoration. The client echoes it on every
    /// compose PUT so the engine can refuse a write composed before a
    /// submission that has since landed. Carried on the summary because a
    /// reload is one of the two places a device learns it (the other is the
    /// `ThreadComposeChanged` broadcast).
    pub compose_epoch: i64,
    pub compose_restorations: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_sub_thread_change_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_draft: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_length: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub liveness: Option<crate::engine::thread_events::liveness::ThreadLiveness>,
}

/// INVARIANT: SQL expression that extracts an app id from `{alias}.coding_agent_folder`.
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

/// INVARIANT: SQL boolean mirroring the frontend `threadPassesChannelFilter` — the single
/// source of truth for "does this thread match the active drawer filter". A
/// channel gate (`sources`) ANDed with per-channel facet narrowing: `trigger`
/// rows by `trigger_ids`, coding-agent rows (persisted as source `claude_code`)
/// by `repo_ids`/`app_ids` (the two unioned). A `chat` row — or any `trigger` /
/// coding-agent row whose facet axis isn't sub-selected — passes on the
/// channel gate alone.
///
/// `s` / `t` / `r` / `a` are the 1-based bind positions of the `sources`,
/// `trigger_ids`, `repo_ids`, `app_ids` params (each `text[]`, bound as
/// `Option<&[String]>`). A NULL or empty array on an axis means "no narrowing
/// there": `array_length(.., 1) IS NULL` is true for both NULL and `'{}'`, so an
/// empty selection is a no-op instead of `= ANY('{}')`, which would match
/// nothing. (The `sources` gate uses `IS NULL` directly — the frontend sends it
/// absent, never empty, when every channel is on.)
///
/// Shared verbatim by `count_archived_threads` (the collapsed Archive badge) and
/// `get_older_threads` (scroll pagination) so the badge total and the rows it
/// scrolls through can never disagree — the drift that made the badge undercount
/// when whole channels were combined with one facet (chat + coding-agent + a
/// single trigger counted only the trigger's threads, dropping the rest).
fn channel_facet_filter_sql(alias: &str, s: usize, t: usize, r: usize, a: usize) -> String {
    let app = app_id_sql_expr(alias);
    format!(
        "(${s}::text[] IS NULL OR {alias}.source = ANY(${s})) \
         AND ({alias}.source <> 'trigger' \
              OR array_length(${t}::text[], 1) IS NULL \
              OR {alias}.trigger_id = ANY(${t})) \
         AND ({alias}.source <> 'claude_code' \
              OR (array_length(${r}::text[], 1) IS NULL AND array_length(${a}::text[], 1) IS NULL) \
              OR {alias}.cc_repo_id = ANY(${r}) \
              OR ({alias}.coding_agent_kind = 'app' AND {app} = ANY(${a})))"
    )
}

#[derive(Serialize, sqlx::FromRow)]
pub struct FilterFacet {
    pub id: Option<String>,
    pub name: Option<String>,
    pub last_activity: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
pub struct AppFilterFacet {
    pub id: String,
    pub name: Option<String>,
    pub kind: Option<crate::core::AppKind>,
    pub deleted: bool,
    pub last_activity: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
pub struct FilterFacets {
    pub triggers: Vec<FilterFacet>,
    pub repos: Vec<FilterFacet>,
    pub apps: Vec<AppFilterFacet>,
}

#[derive(sqlx::FromRow)]
struct ThreadRow {
    thread_id: String,
    title: Option<String>,
    first_message: Option<String>,
    source: String,
    initiator: String,
    created_at: chrono::DateTime<chrono::Utc>,
    last_activity: chrono::DateTime<chrono::Utc>,
    last_user_action: chrono::DateTime<chrono::Utc>,
    last_agent_action: chrono::DateTime<chrono::Utc>,
    message_count: i64,
    section: String,
    active_children_count: i64,
    awaited_children_count: i64,
    awaited_by_parent: bool,
    total_children_count: i64,
    blocking_descendant_count: i64,
    attention_descendant_count: i64,
    archivable_sub_thread_count: i64,
    is_stopped_child: bool,
    hold: Option<sqlx::types::Json<ThreadHoldSummary>>,
    read_requested: bool,
    reply_seen_devices: Vec<String>,
    apply_in_flight: bool,
    live_event_wait_count: i64,
    live_event_waits: sqlx::types::Json<Vec<EventWaitSummary>>,
    cache_keep_alive_status: Option<sqlx::types::Json<CacheKeepAliveStatus>>,
    status: String,
    summary_version: i64,
    coding_agent_change_state: String,
    coding_agent_unproposed_reason: Option<String>,
    coding_agent_requires_restart: bool,
    coding_agent_is_external_repo: bool,
    last_revived_at: Option<chrono::DateTime<chrono::Utc>>,
    is_saved: bool,
    pin_folder: Option<String>,
    is_home: bool,
    has_response: bool,
    parent_thread_id: Option<String>,
    parent_thread_title: Option<String>,
    successor_thread_id: Option<String>,
    successor_thread_title: Option<String>,
    successor_of: Option<String>,
    successor_of_title: Option<String>,
    trigger_id: Option<String>,
    trigger_name: Option<String>,
    cc_repo_id: Option<String>,
    cc_repo_name: Option<String>,
    coding_agent_kind: Option<String>,
    coding_agent_folder: Option<String>,
    coding_agent: Option<String>,
    coding_agents: Vec<String>,
    state: String,
    compose_text: String,
    compose_images: serde_json::Value,
    compose_mode: Option<String>,
    compose_selection: Option<serde_json::Value>,
    compose_epoch: i64,
    compose_restorations: serde_json::Value,
}

/// INVARIANT: Per-event projection snapshot carried on persisted thread events
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
    pub last_user_action: chrono::DateTime<chrono::Utc>,
    pub last_agent_action: chrono::DateTime<chrono::Utc>,
    pub message_count: i64,
    pub section: String,
    pub status: ThreadStatus,
    pub summary_version: i64,
    pub active_children_count: i64,
    pub awaited_children_count: i64,
    pub awaited_by_parent: bool,
    pub total_children_count: i64,
    pub blocking_descendant_count: i64,
    pub attention_descendant_count: i64,
    pub archivable_sub_thread_count: i64,
    pub is_stopped_child: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<ThreadHoldSummary>,
    pub read_requested: bool,
    pub reply_seen_devices: Vec<String>,
    pub apply_in_flight: bool,
    pub live_event_wait_count: i64,
    /// INVARIANT: The waits themselves. See `ThreadSummary::live_event_waits`.
    ///
    /// Carried here as well as on the summary, and that is what makes the
    /// subscription panel self-healing. `applyAggregateToMeta` runs BEFORE
    /// `handleEvent`'s seq-dedup guard, so even a re-delivered event repairs a
    /// stranded list. That is the one path that could not repair it before.
    pub live_event_waits: Vec<EventWaitSummary>,
    pub cache_keep_alive_status: Option<CacheKeepAliveStatus>,
    pub coding_agent_change_state: CodingAgentChangeState,
    pub coding_agent_is_external_repo: bool,
    pub is_saved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin_folder: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub home: bool,
    pub has_response: bool,
    pub last_revived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub parent_thread_id: Option<String>,
    pub parent_thread_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub successor: Option<SuccessorLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub successor_of: Option<SuccessorLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_repo_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_repo_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent_folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent: Option<String>,
    pub coding_agents: Vec<String>,
    pub state: ThreadState,
}

/// INVARIANT: Whether `ancestor` sits above `thread` in the thread tree, at any depth.
/// A thread is not its own ancestor.
///
/// Mirrors the `ancestors` CTE in `engine::event_bus_projection_propagation`,
/// including its lack of a depth cap: `parent_thread_id` is stamped once at
/// spawn from an already-existing thread, so the graph is a forest and the
/// walk terminates.
pub async fn is_thread_ancestor(
    pool: &sqlx::PgPool,
    ancestor: uuid::Uuid,
    thread: uuid::Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "WITH RECURSIVE ancestors AS ( \
            SELECT parent_thread_id AS thread_id \
            FROM thread_summaries \
            WHERE thread_id = $1 AND parent_thread_id IS NOT NULL \
            UNION ALL \
            SELECT t.parent_thread_id AS thread_id \
            FROM thread_summaries t \
            JOIN ancestors a ON t.thread_id = a.thread_id \
            WHERE t.parent_thread_id IS NOT NULL \
         ) \
         SELECT EXISTS (SELECT 1 FROM ancestors WHERE thread_id = $2)",
    )
    .bind(thread)
    .bind(ancestor)
    .fetch_one(pool)
    .await
}

/// INVARIANT: Fetch the projection snapshot for a thread. Polymorphic over executor —
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
        last_user_action: r.last_user_action,
        last_agent_action: r.last_agent_action,
        message_count: r.message_count,
        section: r.section,
        status: ThreadStatus::parse(&r.status),
        summary_version: r.summary_version,
        active_children_count: r.active_children_count,
        awaited_children_count: r.awaited_children_count,
        awaited_by_parent: r.awaited_by_parent,
        total_children_count: r.total_children_count,
        blocking_descendant_count: r.blocking_descendant_count,
        attention_descendant_count: r.attention_descendant_count,
        archivable_sub_thread_count: r.archivable_sub_thread_count,
        is_stopped_child: r.is_stopped_child,
        hold: r.hold.map(|hold| hold.0),
        read_requested: r.read_requested,
        reply_seen_devices: r.reply_seen_devices,
        apply_in_flight: r.apply_in_flight,
        live_event_wait_count: r.live_event_wait_count,
        live_event_waits: r.live_event_waits.0,
        cache_keep_alive_status: r.cache_keep_alive_status.map(|s| s.0),
        coding_agent_change_state: CodingAgentChangeState::from_columns(
            &r.coding_agent_change_state,
            r.coding_agent_unproposed_reason.as_deref(),
            r.coding_agent_requires_restart,
        ),
        coding_agent_is_external_repo: r.coding_agent_is_external_repo,
        is_saved: r.is_saved,
        pin_folder: r.pin_folder,
        home: r.is_home,
        has_response: r.has_response,
        last_revived_at: r.last_revived_at,
        parent_thread_id: r.parent_thread_id,
        parent_thread_title: r.parent_thread_title,
        successor: SuccessorLink::from_columns(r.successor_thread_id, r.successor_thread_title),
        successor_of: SuccessorLink::from_columns(r.successor_of, r.successor_of_title),
        trigger_id: r.trigger_id,
        trigger_name: r.trigger_name,
        cc_repo_id: r.cc_repo_id,
        cc_repo_name: r.cc_repo_name,
        coding_agent_kind: r.coding_agent_kind,
        coding_agent_folder: r.coding_agent_folder,
        coding_agent: r.coding_agent,
        coding_agents: r.coding_agents,
        state: ThreadState::from_db_str(&r.state)?,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SuccessorLink {
    pub thread_id: String,
    pub title: Option<String>,
}

impl SuccessorLink {
    fn from_columns(thread_id: Option<String>, title: Option<String>) -> Option<Self> {
        thread_id.map(|thread_id| Self { thread_id, title })
    }
}

/// INVARIANT: SQL column list for thread summary queries, qualified with `alias` so the
/// same shape works whether the caller's FROM clause aliases the table as
/// `t` (the default — `THREAD_COLS` below) or `s` (the search query that
/// joins `best_scores b` against `thread_summaries s`).
///
/// All correlated subqueries hit a PK b-tree (`thread_summaries.thread_id`,
/// `repositories.id`, `repo_names.id`) and short-circuit when the source FK is
/// NULL. The cast is on the FK side (`{alias}.cc_repo_id::uuid`) — casting
/// `r.id::text` instead would prevent index use.
///
/// `cc_repo_name` resolves the live registry first, then the durable
/// `repo_names` projection (`repo_name_expr`): a registered repo shows its
/// current name; a removed repo falls back to its last recorded name instead of
/// going NULL. NULL survives only for a repo whose name was never recorded
/// anywhere — the frontend keys deleted-vs-live off the live registry, not off
/// this absence.
fn thread_cols(alias: &str) -> String {
    format!(
        "{a}.thread_id::text, {a}.title, {a}.first_message, {a}.source, {a}.initiator, {a}.created_at, {a}.last_activity, \
        {a}.last_user_action, {a}.last_agent_action, \
        {a}.message_count::bigint, {a}.archive_state AS section, {a}.active_children_count::bigint, {a}.awaited_children_count::bigint, {a}.awaited_by_parent, {a}.total_children_count::bigint, \
        {a}.blocking_descendant_count::bigint, {a}.attention_descendant_count::bigint, {a}.archivable_sub_thread_count::bigint, {a}.is_stopped_child, {a}.hold, {a}.read_requested, {a}.reply_seen_devices, {a}.apply_in_flight, \
        {a}.live_event_wait_count::bigint, {a}.live_event_waits, {a}.cache_keep_alive_status, \
        {a}.status, {a}.summary_version, {a}.coding_agent_change_state, {a}.coding_agent_unproposed_reason, {a}.coding_agent_requires_restart, \
        {a}.coding_agent_is_external_repo, {a}.last_revived_at, \
        {a}.is_saved, {a}.pin_folder, {a}.is_home, {a}.has_response, \
        {a}.parent_thread_id::text AS parent_thread_id, \
        (SELECT p.title FROM thread_summaries p WHERE p.thread_id = {a}.parent_thread_id) AS parent_thread_title, \
        {a}.successor_thread_id::text AS successor_thread_id, \
        (SELECT n.title FROM thread_summaries n WHERE n.thread_id = {a}.successor_thread_id) AS successor_thread_title, \
        {a}.successor_of::text AS successor_of, \
        (SELECT o.title FROM thread_summaries o WHERE o.thread_id = {a}.successor_of) AS successor_of_title, \
        {a}.trigger_id, {a}.trigger_name, \
        {a}.cc_repo_id, \
        {repo_name} AS cc_repo_name, \
        {a}.coding_agent_kind, {a}.coding_agent_folder, {a}.coding_agent, {a}.coding_agents, \
        {a}.state, {a}.compose_text, {a}.compose_images, {a}.compose_mode, {a}.compose_selection, \
        {a}.compose_epoch, {a}.compose_restorations",
        a = alias,
        repo_name = repo_name_expr(&format!("{alias}.cc_repo_id")),
    )
}

fn repo_name_expr(id_text_expr: &str) -> String {
    format!(
        "COALESCE(\
            (SELECT r.name FROM repositories r WHERE r.id = ({id})::uuid), \
            (SELECT rn.name FROM repo_names rn WHERE rn.id = ({id})::uuid))",
        id = id_text_expr,
    )
}

static THREAD_COLS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| thread_cols("t"));

impl EventStore {
    pub async fn thread_display_title(&self, thread_id: uuid::Uuid) -> Option<String> {
        let row = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT title, first_message FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await
        .unwrap_or_else(|e| {
            log_legacy!("[ThreadStore] thread_display_title query failed: {}", e);
            None
        });
        let (title, first_message) = row?;
        Some(format_display_title(title, first_message))
    }

    fn rows_to_thread_summaries(
        rows: Vec<ThreadRow>,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        rows.into_iter().map(row_to_thread_summary).collect()
    }
}

fn row_to_thread_summary(
    r: ThreadRow,
) -> Result<ThreadSummary, Box<dyn std::error::Error + Send + Sync>> {
    Ok(ThreadSummary {
        thread_id: r.thread_id,
        title: format_display_title(r.title, r.first_message),
        channel: r.source,
        initiator: LegacyInitiator::from_db_str(r.initiator.as_str())?,
        created_at: r.created_at,
        last_activity: r.last_activity,
        last_user_action: r.last_user_action,
        last_agent_action: r.last_agent_action,
        message_count: r.message_count,
        saved: r.is_saved,
        pin_folder: r.pin_folder,
        home: r.is_home,
        section: r.section,
        active_children_count: r.active_children_count,
        awaited_children_count: r.awaited_children_count,
        awaited_by_parent: r.awaited_by_parent,
        total_children_count: r.total_children_count,
        blocking_descendant_count: r.blocking_descendant_count,
        attention_descendant_count: r.attention_descendant_count,
        archivable_sub_thread_count: r.archivable_sub_thread_count,
        is_stopped_child: r.is_stopped_child,
        hold: r.hold.map(|hold| hold.0),
        read_requested: r.read_requested,
        reply_seen_devices: r.reply_seen_devices,
        apply_in_flight: r.apply_in_flight,
        live_event_wait_count: r.live_event_wait_count,
        live_event_waits: r.live_event_waits.0,
        cache_keep_alive_status: r.cache_keep_alive_status.map(|s| s.0),
        status: ThreadStatus::parse(&r.status),
        summary_version: r.summary_version,
        coding_agent_change_state: CodingAgentChangeState::from_columns(
            &r.coding_agent_change_state,
            r.coding_agent_unproposed_reason.as_deref(),
            r.coding_agent_requires_restart,
        ),
        coding_agent_is_external_repo: r.coding_agent_is_external_repo,
        last_revived_at: r.last_revived_at,
        parent_thread_id: r.parent_thread_id,
        parent_thread_title: r.parent_thread_title,
        successor: SuccessorLink::from_columns(r.successor_thread_id, r.successor_thread_title),
        successor_of: SuccessorLink::from_columns(r.successor_of, r.successor_of_title),
        trigger_id: r.trigger_id,
        trigger_name: r.trigger_name,
        cc_repo_id: r.cc_repo_id,
        cc_repo_name: r.cc_repo_name,
        coding_agent_kind: r.coding_agent_kind,
        coding_agent_folder: r.coding_agent_folder,
        coding_agent: r.coding_agent,
        coding_agents: r.coding_agents,
        state: ThreadState::from_db_str(&r.state)?,
        compose_text: r.compose_text,
        compose_images: r.compose_images,
        compose_mode: r.compose_mode,
        compose_selection: r.compose_selection,
        compose_epoch: r.compose_epoch,
        compose_restorations: r.compose_restorations,
        pending_sub_thread_change_count: None,
        has_draft: None,
        draft_preview: None,
        draft_length: None,
        link: None,
        liveness: None,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ThreadSearchResult {
    #[serde(flatten)]
    pub info: ThreadSummary,
    pub score: f64,
}

pub struct ThreadSummaryFilters<'a> {
    pub status: StatusFilter<'a>,
    pub sources: Option<&'a [String]>,
    pub parent: Option<uuid::Uuid>,
    pub has_draft: Option<bool>,
    pub change_state: Option<ChangeStateKind>,
    pub limit: i64,
}

static SUMMARY_FILTER_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "($3::text[] IS NULL OR t.source = ANY($3)) \
         AND ($4::uuid IS NULL OR t.parent_thread_id = $4) \
         AND ($5::bool IS NULL OR ({has_draft}) = $5) \
         AND ($6::text IS NULL OR t.coding_agent_change_state = $6)",
        has_draft = HAS_DRAFT_SQL,
    )
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter<'a> {
    Any,
    Active(bool),
    OneOf(&'a [ThreadStatus]),
}

impl StatusFilter<'_> {
    pub(crate) fn binds(&self) -> (Option<Vec<&'static str>>, bool) {
        match self {
            Self::Any => (None, false),
            Self::Active(want) => (Some(active_thread_statuses().to_vec()), !want),
            Self::OneOf(statuses) => (
                Some(statuses.iter().map(ThreadStatus::as_str).collect()),
                false,
            ),
        }
    }
}

pub(crate) static STATUS_FILTER_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let held_question = format!(
        "(t.status = {} AND t.hold IS NOT NULL)",
        ThreadStatus::WaitingForUserAnswer.sql_literal()
    );
    format!(
        "($1::text[] IS NULL \
             OR ($2 = FALSE AND t.status = ANY($1) AND NOT {held_question}) \
             OR ($2 = TRUE AND (NOT (t.status = ANY($1)) OR {held_question})))"
    )
});

/// INVARIANT: What a list of thread names calls a thread with no name at all.
///
/// The last resort, after [`format_display_title`] has tried the title and the
/// first message. One word, because the two voice lines that need it read out
/// next to each other.
///
/// It is a name rather than a gap: a thread is often busiest before it has a
/// title, so dropping it would report the workspace as quieter than it is.
pub const UNTITLED_THREAD: &str = "Untitled";

/// INVARIANT: Statuses considered "active": the agentic loop is mid-flow.
///
/// A thread merely holding an *event wait* does NOT count: a subscription does
/// not hold its thread's turn, so a thread watching for a release has genuinely
/// finished its turn and is idle. It surfaces through the subscription
/// indicator instead.
///
/// This is a UNION of two states that answer different questions, which is why
/// [`StatusFilter::OneOf`] exists: `running` is the workspace working, while
/// `waiting_for_user_answer` is the workspace stopped and waiting on a person.
/// A caller asking "is anything still busy?" wants `running` alone.
pub fn active_thread_statuses() -> [&'static str; 2] {
    [
        ThreadStatus::Running.as_str(),
        ThreadStatus::WaitingForUserAnswer.as_str(),
    ]
}

pub fn still_working_sql(alias: &str, statuses_param: &str) -> String {
    format!("({alias}.status = ANY({statuses_param}) OR {alias}.live_event_wait_count > 0)")
}

pub fn parse_status_filter_values<S: AsRef<str>>(raw: &[S]) -> Result<Vec<ThreadStatus>, String> {
    let mut out = Vec::with_capacity(raw.len());
    for value in raw {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return Err(format!(
                "status has an empty value (a stray comma?). Valid statuses: {}.",
                status_value_list()
            ));
        }
        match ThreadStatus::try_parse(value) {
            Some(status) => out.push(status),
            None => return Err(unknown_status_message(value)),
        }
    }
    if out.is_empty() {
        return Err(format!(
            "status needs at least one value. Valid statuses: {}.",
            status_value_list()
        ));
    }
    Ok(out)
}

pub fn parse_status_filter_csv(raw: &str) -> Result<Vec<ThreadStatus>, String> {
    parse_status_filter_values(&raw.split(',').collect::<Vec<_>>())
}

pub fn status_value_list() -> String {
    ThreadStatus::ALL
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn unknown_status_message(value: &str) -> String {
    format!(
        "'{value}' is not a thread status. Valid statuses: {}. \
         For 'is the workspace busy?' use status=running: a thread \
         awaiting a user answer is blocked on the human, not working.",
        status_value_list()
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct ThreadTimelineEvent {
    pub event_type: String,
    pub timestamp: String,
    pub description: Option<String>,
    pub change_id: Option<String>,
}

mod backfill;
mod compose;
mod drafts;
mod events;

pub(crate) use compose::compose_report_cols;
pub use compose::{
    consume_compose_slot, ComposeReport, ComposeReportRow, ComposeRestoration, COMPOSE_REPORT_COLS,
    MAX_COMPOSE_IMAGES, MAX_COMPOSE_SELECTION_BYTES, MAX_COMPOSE_TEXT_BYTES,
};
pub(crate) use compose::{pin_legacy_compose_agents, restore_compose_slot};

pub use drafts::{
    attach_reader_fields, char_length, has_draft, text_preview, thread_link, DraftSummary,
    HAS_DRAFT_SQL, PREVIEW_CHARS,
};
mod search;
pub use search::MessageMatch;
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
#[path = "../threads_tests/drafts.rs"]
mod drafts_tests;

#[cfg(test)]
#[path = "../threads_tests/compose.rs"]
mod compose_tests;

#[cfg(test)]
#[path = "../threads_tests/extraction.rs"]
mod extraction_tests;

#[cfg(test)]
#[path = "../threads_tests/turn_clock.rs"]
mod turn_clock_tests;

#[cfg(test)]
#[path = "../threads_tests/wire_fixture.rs"]
mod wire_fixture_tests;

#[cfg(test)]
#[path = "../threads_tests/summary_version.rs"]
mod summary_version_tests;
