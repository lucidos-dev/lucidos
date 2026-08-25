use super::messages::build_session_messages;
use super::types::SessionMessage;
use super::EventStore;
use crate::core::event_subscription::EventSubscription;
use crate::core::EventRow;
use crate::engine::thread_lifecycle::{ArchiveState, ThreadStatus};
use serde::{Deserialize, Serialize};

/// Preference marker: set after `backfill_trigger_id_v5_to_config_id` runs
/// successfully so subsequent boots skip the events scan.
const BACKFILL_TRIGGER_ID_V5_MARKER: &str = "backfill_trigger_id_v5_to_config_id_done";

/// Preference marker: set after `backfill_trigger_id_from_events` runs
/// successfully so subsequent boots skip the events scan.
const BACKFILL_TRIGGER_ID_FROM_EVENTS_MARKER: &str = "backfill_trigger_id_from_events_done";

/// Preference marker: set after `backfill_repo_names_from_changes` runs
/// successfully so subsequent boots skip the `changes` scan.
const BACKFILL_REPO_NAMES_FROM_CHANGES_MARKER: &str = "backfill_repo_names_from_changes_done";

/// Preference marker: set after `backfill_cc_repo_id_to_deterministic` runs
/// successfully so subsequent boots don't re-point thread bindings.
const BACKFILL_CC_REPO_ID_DETERMINISTIC_MARKER: &str = "backfill_cc_repo_id_to_deterministic_done";

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

/// One live *event wait*, as the client needs to render it.
///
/// Stored in the `thread_summaries.live_event_waits` JSONB column and carried
/// verbatim on [`ThreadSummary`] and [`ThreadAggregate`]. Every field copies
/// the wait's `EventWaitStarted` payload. So this projects the event rather
/// than inventing a second notion of what a wait is (ADR 0047).
///
/// Deliberately NOT [`crate::engine::event_wait::LiveWait`], which is the
/// dispatcher's runtime cache and carries what *matching* needs (`thread_id`,
/// `tool_use_id`, `armed_at`, `watermark`). This carries what the subscription
/// indicator draws, and nothing else: `reason` is the sentence, `on` is the
/// chips, `expires_at` is the countdown, and `wait_id` is what the Stop
/// waiting button cancels.
///
/// The TS mirror is `EventWaitSummary` in
/// `crates/lucidos-app/src/store/thread-events/thread-event-types.ts`. Same
/// name, same snake_case field names, so the column deserializes straight into
/// `meta.liveEventWaits` with no mapping layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventWaitSummary {
    pub wait_id: uuid::Uuid,
    pub on: Vec<EventSubscription>,
    pub reason: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
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
    /// When the USER last drove this thread forward (message sent, question
    /// answered, permission resolved, change applied/discarded). The drawer
    /// sorts by this so background agent churn no longer reshuffles the list.
    /// NOT NULL — defaults to the row's creation time for never-acted threads.
    pub last_user_action: chrono::DateTime<chrono::Utc>,
    /// When the AGENT (or trigger) last did something — streaming, a terminal
    /// response, an idle, a trigger fire/complete, or asking the user. Drives
    /// the thread-row tooltip's "Agent ·" line, distinct from `last_user_action`
    /// so the tooltip is accurate even right after the user acts.
    pub last_agent_action: chrono::DateTime<chrono::Utc>,
    pub message_count: i64,
    /// Whether the user parked this thread in the Saved section
    /// (`thread_summaries.is_saved`).
    ///
    /// `GET /api/v1/threads` also conveys this structurally, by returning saved
    /// threads in its own `saved` array, and the drawer still reads it from
    /// there. The field exists because the by-id read paths have no such array:
    /// `GET /api/v1/threads/:id` returns one bare summary, and a client
    /// bootstrapping a single thread from it (a notification tap landing on a
    /// thread outside the loaded window) would otherwise have to either guess
    /// `false` or re-fetch the whole grouped payload. Both sources read this same
    /// column, so they cannot disagree.
    pub saved: bool,
    /// Thread section: "archived" (history/saved), "inbox" (needs user attention).
    /// Stored in `thread_summaries.archive_state` column; aliased to `section` in
    /// SELECTs to keep the JSON wire format stable.
    pub section: String,
    /// Number of child threads still active (non-zero means parent is "ongoing").
    pub active_children_count: i64,
    /// Total number of child threads (active + completed).
    pub total_children_count: i64,
    /// Number of *event waits* this thread currently holds unresolved. Read as
    /// `count > 0` by the frontend's `resolveVisualStatus` to paint the same
    /// Waiting dot `active_children_count` paints, and by nothing else: a
    /// subscription holds no turn, so no backend predicate consumes it
    /// (ADR 0049). Its own thread's waits only, never a descendant roll-up.
    pub live_event_wait_count: i64,
    /// The same waits the count counts, spelled out: one entry per unresolved
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
    /// Count of descendants (transitive) currently in a state that blocks this
    /// thread from being archived (per `is_blocking` in `thread_lifecycle.rs`).
    /// Maintained by EventBus on `thread_summaries.blocking_descendant_count`.
    /// Consumed by `resolve_actions` via `count > 0`; the raw count enables
    /// UI affordances like "3 sub-threads still busy".
    pub blocking_descendant_count: i64,
    /// Count of descendants (transitive) currently in a state that needs user
    /// attention (per `is_attention_needing` in `thread_lifecycle.rs`):
    /// WaitingForUserAnswer, or an in-workspace coding-agent thread with pending changes.
    /// Strict subset of `blocking_descendant_count` — drops the `Running` case.
    /// Consumed by `display_section` via `count > 0` to bubble Current to the
    /// ancestor chain even when sibling descendants are still running.
    pub attention_descendant_count: i64,
    /// Thread status, computed by the backend. One of the six
    /// [`ThreadStatus`] values as written by `as_str`: `idle`, `running`,
    /// `waiting`, `waiting_for_user_answer`, `paused`, `failed`. Nothing
    /// writes `waiting` any more; it survives on historical rows.
    pub status: String,
    /// Whether the coding-agent branch has any diff against main on disk — pure git
    /// truth. Set by the projection on `ChangeProposed`, cleared on
    /// `ChangeApplied` / `ChangeDiscarded` / `ThreadArchived`, seeded at
    /// session bootstrap, and reconciled by the startup sweep against on-disk
    /// git state. Drives the WaitingBanner Diff button.
    pub coding_agent_has_diff: bool,
    /// Coding-agent thread's formal "ready for review" offer — set only by `ChangeProposed`,
    /// cleared on Apply/Discard/Archive. Drives the Apply / Discard buttons.
    /// Distinct from `coding_agent_has_diff` (the git fact): a thread can have
    /// a diff mid-session before the coding agent has formally proposed.
    pub coding_agent_proposed: bool,
    /// Whether the proposed change requires an engine restart. Only meaningful
    /// when `coding_agent_proposed = true`; cleared together with it.
    pub coding_agent_requires_restart: bool,
    /// Whether the coding-agent thread is bound to an external repo. External repos
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
    /// Repository the coding-agent thread bound to (only for `channel == "claude_code"`
    /// threads). Stored as TEXT on `thread_summaries.cc_repo_id`; matches a
    /// `repositories.id` UUID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_repo_id: Option<String>,
    /// Current repo display name resolved from the registry. NULL when the
    /// repo was deleted from `repositories` after the thread was bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_repo_name: Option<String>,
    /// Coding-agent thread flavor — `'lucidos' | 'app' | 'external'`. Stored
    /// on `thread_summaries.coding_agent_kind`. NULL for non-coding-agent threads and
    /// legacy coding-agent rows (consumers default NULL → `'lucidos'`). Frontend reads
    /// this to render the app-thread affordances (branch chip, WIP preview).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent_kind: Option<String>,
    /// Canonical folder the coding agent operates on — `<ws>/data/apps/<id>/`
    /// for App, the repo root for Lucidos and External. Stored on
    /// `thread_summaries.coding_agent_folder`. NULL for non-coding-agent threads and
    /// legacy rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent_folder: Option<String>,
    /// Which backend drives this thread — `'claude-code' | 'codex'`. Stored on
    /// `thread_summaries.coding_agent`. NULL for non-coding-agent threads and legacy coding-agent
    /// rows (consumers default NULL → `'claude-code'` via `CodingAgent::parse`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent: Option<String>,
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
    /// Per-draft dropdown selections (target/scope, coding agent, Lucidos
    /// model + reasoning, coding-agent model + reasoning) as a partial
    /// `ComposeSelectionOverride`-shaped object. `None` = no per-draft picks
    /// stored; the frontend resolves each unset field to its account default.
    /// Carried on the `/api/v1/threads` initial fetch so a reload rehydrates
    /// the draft's picks (the DB is the authoritative store, like `compose_text`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose_selection: Option<serde_json::Value>,
    /// The thread's *compose epoch* (`docs/glossary.md`): how many times a
    /// submission has consumed its compose slot. The client echoes it on every
    /// compose PUT so the engine can refuse a write composed before a
    /// submission that has since landed. Carried on the summary because a
    /// reload is one of the two places a device learns it (the other is the
    /// `ThreadComposeChanged` broadcast).
    pub compose_epoch: i64,
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

/// SQL boolean mirroring the frontend `threadPassesChannelFilter` — the single
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

/// One selectable filter facet (trigger / repo / app) with the timestamp of its
/// most-recent thread. `id` is non-null in practice (the queries filter
/// `… IS NOT NULL`) but stays `Option` to match the nullable columns.
///
/// `name` is the resolved label, populated for the repos facet (live registry →
/// `repo_names` projection) so a removed repo lists under its historical name
/// rather than its raw UUID. NULL for trigger / app facets, whose labels the
/// frontend resolves from its own registries.
#[derive(Serialize, sqlx::FromRow)]
pub struct FilterFacet {
    pub id: Option<String>,
    pub name: Option<String>,
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
    last_user_action: chrono::DateTime<chrono::Utc>,
    last_agent_action: chrono::DateTime<chrono::Utc>,
    message_count: i64,
    section: String,
    active_children_count: i64,
    total_children_count: i64,
    blocking_descendant_count: i64,
    attention_descendant_count: i64,
    live_event_wait_count: i64,
    /// `sqlx::types::Json` rather than a bare `Vec`, because that is what
    /// decodes a JSONB column into a typed value.
    live_event_waits: sqlx::types::Json<Vec<EventWaitSummary>>,
    status: String,
    /// Pure git truth — `git diff main..branch` is non-empty.
    coding_agent_has_diff: bool,
    /// Coding-agent thread's formal "ready for review" — set by `ChangeProposed` only.
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
    coding_agent: Option<String>,
    state: String,
    compose_text: String,
    compose_images: serde_json::Value,
    compose_mode: Option<String>,
    compose_selection: Option<serde_json::Value>,
    compose_epoch: i64,
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
    /// See `ThreadSummary::last_user_action`. Carried on the per-event SSE
    /// aggregate so the drawer's sort key stays live without a refetch.
    pub last_user_action: chrono::DateTime<chrono::Utc>,
    /// See `ThreadSummary::last_agent_action`. Carried on the per-event SSE
    /// aggregate so the row tooltip's "Agent ·" line stays current.
    pub last_agent_action: chrono::DateTime<chrono::Utc>,
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
    /// Current-bubble rule in `display_section`. Strict subset of
    /// `blocking_descendant_count` (drops the Running case). Carried on the
    /// per-event SSE aggregate alongside `blocking_descendant_count` so the
    /// frontend can recompute section without a refetch.
    pub attention_descendant_count: i64,
    /// Count of *event waits* this thread holds unresolved. See
    /// `ThreadSummary::live_event_wait_count`. Carried on the per-event SSE
    /// aggregate so the Waiting dot lights the moment a wait is registered or
    /// resolved, on every open client, without a refetch.
    pub live_event_wait_count: i64,
    /// The waits themselves. See `ThreadSummary::live_event_waits`.
    ///
    /// Carried here as well as on the summary, and that is what makes the
    /// subscription panel self-healing. `applyAggregateToMeta` runs BEFORE
    /// `handleEvent`'s seq-dedup guard, so even a re-delivered event repairs a
    /// stranded list. That is the one path that could not repair it before.
    pub live_event_waits: Vec<EventWaitSummary>,
    /// Pure git truth — drives the Diff button. See `ThreadSummary::coding_agent_has_diff`.
    pub coding_agent_has_diff: bool,
    /// Coding-agent thread's formal "ready for review" offer — set by `ChangeProposed` only.
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
    /// Backend — see `ThreadSummary::coding_agent`. Wire field: `codingAgent`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_agent: Option<String>,
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
        last_user_action: r.last_user_action,
        last_agent_action: r.last_agent_action,
        message_count: r.message_count,
        section: r.section,
        status: r.status,
        active_children_count: r.active_children_count,
        total_children_count: r.total_children_count,
        blocking_descendant_count: r.blocking_descendant_count,
        attention_descendant_count: r.attention_descendant_count,
        live_event_wait_count: r.live_event_wait_count,
        live_event_waits: r.live_event_waits.0,
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
        coding_agent: r.coding_agent,
        state: r.state,
    })
}

/// SQL column list for thread summary queries, qualified with `alias` so the
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
        {a}.message_count::bigint, {a}.archive_state AS section, {a}.active_children_count::bigint, {a}.total_children_count::bigint, \
        {a}.blocking_descendant_count::bigint, {a}.attention_descendant_count::bigint, \
        {a}.live_event_wait_count::bigint, {a}.live_event_waits, \
        {a}.status, {a}.coding_agent_has_diff, {a}.coding_agent_proposed, {a}.coding_agent_requires_restart, \
        {a}.coding_agent_is_external_repo, {a}.coding_agent_applying, {a}.last_revived_at, \
        {a}.is_saved, {a}.has_response, \
        {a}.parent_thread_id::text AS parent_thread_id, \
        (SELECT p.title FROM thread_summaries p WHERE p.thread_id = {a}.parent_thread_id) AS parent_thread_title, \
        {a}.trigger_id, {a}.trigger_name, \
        {a}.cc_repo_id, \
        {repo_name} AS cc_repo_name, \
        {a}.coding_agent_kind, {a}.coding_agent_folder, {a}.coding_agent, \
        {a}.state, {a}.compose_text, {a}.compose_images, {a}.compose_mode, {a}.compose_selection, \
        {a}.compose_epoch",
        a = alias,
        repo_name = repo_name_expr(&format!("{alias}.cc_repo_id")),
    )
}

/// SQL scalar expression resolving a repo name from a `cc_repo_id` text
/// expression: the live `repositories` registry wins (current name, including
/// renames), falling back to the durable `repo_names` projection (last recorded
/// name for a removed repo). `id_text_expr` is the text-typed UUID source —
/// e.g. `t.cc_repo_id` or a subquery alias — cast on the FK side so both PK
/// indexes stay usable.
fn repo_name_expr(id_text_expr: &str) -> String {
    format!(
        "COALESCE(\
            (SELECT r.name FROM repositories r WHERE r.id = ({id})::uuid), \
            (SELECT rn.name FROM repo_names rn WHERE rn.id = ({id})::uuid))",
        id = id_text_expr,
    )
}

/// Default `thread_cols("t")` for the common `FROM thread_summaries t` shape.
/// Cached because every list/get query path formats SQL with it.
static THREAD_COLS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| thread_cols("t"));

impl EventStore {
    /// The name the user sees for a thread: its title once one exists, else a
    /// truncated first message. Exactly [`format_display_title`], so a
    /// coding-agent branch is named after the same string the thread drawer
    /// shows rather than a second, drifting notion of "the thread's name".
    ///
    /// `None` when the thread has no projection row yet; empty when it has one
    /// with neither field (a thread addressed by id before its first message
    /// landed). Both leave the caller to supply its own fallback.
    pub async fn thread_display_title(&self, thread_id: uuid::Uuid) -> Option<String> {
        let row = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT title, first_message FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await
        .unwrap_or_else(|e| {
            log!("[ThreadStore] thread_display_title query failed: {}", e);
            None
        });
        let (title, first_message) = row?;
        Some(format_display_title(title, first_message))
    }

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
                    last_user_action: r.last_user_action,
                    last_agent_action: r.last_agent_action,
                    message_count: r.message_count,
                    saved: r.is_saved,
                    section: r.section,
                    active_children_count: r.active_children_count,
                    total_children_count: r.total_children_count,
                    blocking_descendant_count: r.blocking_descendant_count,
                    attention_descendant_count: r.attention_descendant_count,
                    live_event_wait_count: r.live_event_wait_count,
                    live_event_waits: r.live_event_waits.0,
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
                    coding_agent: r.coding_agent,
                    state: r.state,
                    compose_text: r.compose_text,
                    compose_images: r.compose_images,
                    compose_mode: r.compose_mode,
                    compose_selection: r.compose_selection,
                    compose_epoch: r.compose_epoch,
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
pub struct ThreadSummaryFilters<'a> {
    /// How to narrow by `thread_summaries.status`. See [`StatusFilter`].
    pub status: StatusFilter<'a>,
    pub sources: Option<&'a [String]>,
    /// Restrict to the direct children of this thread. `None` is no filter.
    ///
    /// Direct children only (`parent_thread_id`), never the recursive ancestor
    /// CTE: a parent addresses the level below it, which is what a star
    /// topology at each level means. This is the read side of a parent
    /// orchestrating its own fan-out, and it is how the Lucidos Agent recovers
    /// a child's `thread_id` after history trimming drops the spawn result.
    pub parent: Option<uuid::Uuid>,
    pub limit: i64,
}

/// How a `list` / `count` query narrows by thread status.
///
/// An enum rather than two independent fields because the two ways of asking
/// are alternatives, not filters that compose: "the active union" and "exactly
/// these statuses" are two answers to one question, and a query carrying both
/// would silently intersect. Every caller-facing surface refuses the
/// combination with an actionable message; this type is what makes the refusal
/// exhaustive instead of a check somebody can forget to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter<'a> {
    /// No status narrowing.
    Any,
    /// The `active` union: `true` selects [`active_thread_statuses`], `false`
    /// selects everything else. The oldest of the three and the one every
    /// existing caller uses, kept bit-for-bit so a patch release breaks nobody.
    Active(bool),
    /// Exactly these statuses. This is the precise form: `OneOf(&[Running])`
    /// answers "is the workspace busy?", which the `Active` union answers
    /// wrongly, because a thread awaiting a user answer is blocked on the
    /// human rather than working.
    OneOf(&'a [ThreadStatus]),
}

impl StatusFilter<'_> {
    /// The two bind params the shared SQL predicate takes: the status list
    /// (`None` disables the clause entirely) and whether to invert it.
    ///
    /// Collapsing all three variants onto one `(list, negate)` pair is what
    /// lets `list_thread_summaries` and `count_thread_summaries` share one
    /// static predicate instead of branching per variant.
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

/// The `WHERE` fragment [`StatusFilter::binds`] feeds, shared verbatim by the
/// list and count queries so the two can never disagree about what a filter
/// selects. `$1` is the status list (NULL disables the clause), `$2` inverts.
pub(crate) const STATUS_FILTER_SQL: &str = "($1::text[] IS NULL \
     OR ($2 = FALSE AND t.status = ANY($1)) \
     OR ($2 = TRUE AND NOT (t.status = ANY($1))))";

/// Statuses considered "active": the agentic loop is mid-flow.
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

/// Parse caller-supplied status filter values into the typed form
/// [`StatusFilter::OneOf`] takes.
///
/// Shared by every input surface (the HTTP query param, the LLM tool arg, and
/// through them the CLI flag and the SDK option) so one wrong value produces
/// one wording everywhere. Three things are deliberately errors rather than a
/// quietly different answer, because a filter that answers a question nobody
/// asked is the entire bug this filter exists to fix:
///
/// - an unrecognized value, which would otherwise select nothing,
/// - an empty list, which would otherwise select everything,
/// - a blank item inside an otherwise valid list, which hides a typo.
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

/// Comma-separated form of [`parse_status_filter_values`], for the surfaces
/// that carry the filter as one string: the HTTP query param, the CLI flag it
/// forwards, and the string form of the LLM tool arg.
pub fn parse_status_filter_csv(raw: &str) -> Result<Vec<ThreadStatus>, String> {
    parse_status_filter_values(&raw.split(',').collect::<Vec<_>>())
}

/// Comma-separated list of every accepted status value, for help text, tool
/// schemas and error messages. Sourced from [`ThreadStatus::ALL`] so it cannot
/// drift from the enum.
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

#[cfg(test)]
#[path = "../threads_tests/turn_clock.rs"]
mod turn_clock_tests;
