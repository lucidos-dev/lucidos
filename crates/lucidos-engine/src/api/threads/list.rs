//! `GET` thread-listing handlers — the UI-shaped fetch, the projection
//! summaries/count surface, the older-page paginator, and filter facets —
//! plus the shared family-extension loader they build on.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::api::AppState;
use crate::core::store::{
    EventStore, FilterFacets, StatusFilter, ThreadSummary, ThreadSummaryFilters,
};

#[derive(Deserialize)]
pub struct ListThreadsQuery {
    /// Thread ID the frontend currently has focused — ensures it's included in the
    /// response even if it's not in the recent/saved/active lists.
    pub focused: Option<String>,
}

/// Hard cap on how many family-extension threads `list_threads` and
/// `get_older_threads` will pull in alongside the paginated base set. Without
/// a cap, an archive page covering a pathological fan-out root (one chat
/// spawning hundreds of coding-agent / trigger children) balloons the initial payload
/// into the hundreds, and the frontend's drawer re-renders every row on each
/// SSE flush. 100 is enough to render typical families completely; deeper
/// families reveal themselves on scroll-pagination.
const MAX_FAMILY_THREADS: i64 = 100;

/// Collect the family extension for a base set of threads. Skips the SQL
/// roundtrip entirely when no base thread has a parent or children — most
/// chat-only workspaces never trigger the recursive CTE this way. Used by
/// both `list_threads` and `get_older_threads`; the error mapping lives
/// here so both call sites stay one-liners.
async fn load_family_threads<'a>(
    store: &EventStore,
    base: impl Iterator<Item = &'a ThreadSummary>,
) -> Result<Vec<ThreadSummary>, (StatusCode, String)> {
    let mut any_family = false;
    let ids: Vec<String> = base
        .map(|t| {
            if t.parent_thread_id.is_some() || t.total_children_count > 0 {
                any_family = true;
            }
            t.thread_id.clone()
        })
        .collect();
    if !any_family {
        return Ok(Vec::new());
    }
    store
        .fetch_family_extension(&ids, MAX_FAMILY_THREADS)
        .await
        .map_err(|e| {
            log!("[API] Failed to fetch family extension: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to fetch family extension: {}", e),
            )
        })
}

/// GET /api/v1/threads — returns saved threads, recent archive, and active thread IDs
pub(in crate::api) async fn list_threads(
    State(state): State<AppState>,
    Query(query): Query<ListThreadsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Active thread IDs — only threads with a live processing task (chat loop running).
    // Coding-agent session existence no longer makes a thread "active" — the status column in
    // thread_summaries handles that (set by CodingAgentIdled → 'waiting').
    let active_id_strings: Vec<String> = state
        .engine
        .processing_thread_ids()
        .iter()
        .map(|id| id.to_string())
        .collect();

    // Run all DB queries in parallel
    let store = state.engine.event_store();
    let (saved_result, recent_result, active_result, composing_result, archive_count_result) = tokio::join!(
        store.get_saved_threads(),
        // Global newest-`created_at` archive slice (no longer per-source); 30 fills
        // the initial Archive view comfortably before scroll-pagination takes over.
        store.get_recent_threads(30),
        store.get_threads_by_ids(&active_id_strings),
        store.get_composing_threads(),
        // Unfiltered total for the first-paint badge; the frontend re-fetches a
        // filter-scoped count via GET /threads/archived-count when a drawer
        // filter is active (see `refreshArchivedCount`).
        store.count_archived_threads(None, None, None, None),
    );

    let saved = saved_result.map_err(|e| {
        log!("[API] Failed to get saved threads: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get saved threads: {}", e),
        )
    })?;
    let recent = recent_result.map_err(|e| {
        log!("[API] Failed to get recent threads: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get recent threads: {}", e),
        )
    })?;
    let active_threads = active_result.map_err(|e| {
        log!("[API] Failed to get active thread info: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get active thread info: {}", e),
        )
    })?;
    let composing = composing_result.map_err(|e| {
        log!("[API] Failed to get composing threads: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get composing threads: {}", e),
        )
    })?;
    let archive_count = archive_count_result.map_err(|e| {
        log!("[API] Failed to count archived threads: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to count archived threads: {}", e),
        )
    })?;

    // If the frontend has a focused thread, ensure it's in the response.
    // The focused thread may fall outside the global newest-30 archive slice
    // fetched above, not be saved, and not be actively processing. Without
    // this, reload would lose it.
    let focused_thread = if let Some(ref focused_id) = query.focused {
        let already_included = saved.iter().any(|t| t.thread_id == *focused_id)
            || recent.iter().any(|t| t.thread_id == *focused_id)
            || active_threads.iter().any(|t| t.thread_id == *focused_id)
            || composing.iter().any(|t| t.thread_id == *focused_id);
        if !already_included {
            let mut threads = store
                .get_threads_by_ids(std::slice::from_ref(focused_id))
                .await
                .map_err(|e| {
                    log!("[API] Failed to fetch focused thread {}: {}", focused_id, e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to fetch focused thread: {}", e),
                    )
                })?;
            threads.pop()
        } else {
            None
        }
    } else {
        None
    };

    // Load any family members (ancestors + descendants) of the base set
    // that aren't already in it. The drawer's family-aware routing
    // (ThreadDrawer.tsx → categorizeThreads) lifts a whole family to the
    // freshest member's recency and nests children under their parent;
    // without this extension, a family member whose own `last_activity`
    // falls below the loaded window would silently disappear under the
    // parent's "N/N done" badge.
    let family_base = saved
        .iter()
        .chain(recent.iter())
        .chain(active_threads.iter())
        .chain(composing.iter())
        .chain(focused_thread.iter());
    let family_threads = load_family_threads(store, family_base).await?;

    let mut response = serde_json::json!({
        "saved": saved,
        "archive": recent,
        // Total archived-pile size (not just the loaded `archive` window) — the
        // collapsed Archive section's count badge reads this so it reflects the
        // true total instead of however many rows happen to be paginated in.
        "archive_count": archive_count,
        "active": active_id_strings,
        "active_threads": active_threads,
        "composing": composing,
        "family_threads": family_threads,
    });
    if let Some(ft) = focused_thread {
        response["focused_thread"] = match serde_json::to_value(&ft) {
            Ok(v) => v,
            Err(e) => {
                log!("[API] Failed to serialize focused thread: {}", e);
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to serialize focused thread: {}", e),
                ));
            }
        };
    }

    Ok(Json(response))
}

/// Shared query parameters for `GET /api/v1/threads/list` and
/// `GET /api/v1/threads/count` — the script/trigger/LLM-tool surface for
/// querying thread summaries. Mirrors the CLI flags on `lucidos threads list`
/// and the args on the `list_threads` LLM tool.
#[derive(Deserialize)]
pub struct ListThreadSummariesQuery {
    /// The UNION of `running` and `waiting_for_user_answer`. `true` selects it,
    /// `false` inverts it, omitting it filters nothing.
    ///
    /// For "is the workspace busy?", use `status=running` instead: a thread
    /// awaiting a user answer is blocked on the human, not working, so it is in
    /// this union while being the opposite of busy. `waiting` is in neither: it
    /// means the coding agent stopped and proposed changes the user must act on.
    ///
    /// Mutually exclusive with `status`, which is the precise form of the same
    /// question.
    #[serde(default)]
    pub active: Option<bool>,
    /// Comma-separated status filter, naming exactly the statuses to keep, in
    /// the same spelling every returned row's `status` field carries (the
    /// kebab form `waiting-for-user-answer` is accepted too). Mutually
    /// exclusive with `active`. An unrecognized or empty value is a 400, never
    /// a silently empty or unfiltered result.
    #[serde(default)]
    pub status: Option<String>,
    /// Comma-separated source filter: `chat`, `trigger`, `coding-agent`
    /// (`claude_code` is accepted as a legacy alias).
    #[serde(default)]
    pub source: Option<String>,
    /// Server clamps to 1..=1000. Default 100. Only consumed by the list
    /// handler; the count handler ignores it.
    #[serde(default)]
    pub limit: Option<i64>,
    /// Restrict to the direct children of this thread id. Always a literal
    /// uuid: HTTP has no ambient caller, so there is no "self" to resolve. The
    /// LLM tool's `my_children` argument is the surface that resolves the
    /// caller's own id, from its ambient `thread_id`.
    #[serde(default)]
    pub parent: Option<String>,
}

/// Resolve the `active` / `status` query params into the single filter the
/// store takes.
///
/// Sending both is a 400 rather than an intersection: they are two answers to
/// one question, and quietly ANDing them would produce an empty result for
/// `active=true&status=idle` and a redundant one for `active=true&status=running`,
/// neither of which is what the caller asked. The message names the way out
/// because the likely reason a caller reached for both is that `active` did not
/// mean what they expected.
fn parse_status_filter(
    q: &ListThreadSummariesQuery,
) -> Result<Vec<crate::engine::thread_lifecycle::ThreadStatus>, (StatusCode, String)> {
    match (q.active, q.status.as_deref()) {
        (Some(_), Some(_)) => Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Pass either active or status, not both. active is the union \
                 (running, waiting_for_user_answer); status names exactly the \
                 statuses you want, out of {}.",
                crate::core::store::status_value_list()
            ),
        )),
        (_, Some(raw)) => crate::core::store::parse_status_filter_csv(raw)
            .map_err(|e| (StatusCode::BAD_REQUEST, e)),
        (_, None) => Ok(Vec::new()),
    }
}

/// Build the store filter from the already-validated pieces. Kept separate
/// from [`parse_status_filter`] because `StatusFilter::OneOf` borrows the
/// parsed vector, which has to outlive the query.
fn status_filter<'a>(
    active: Option<bool>,
    statuses: &'a [crate::engine::thread_lifecycle::ThreadStatus],
) -> StatusFilter<'a> {
    if !statuses.is_empty() {
        StatusFilter::OneOf(statuses)
    } else if let Some(want) = active {
        StatusFilter::Active(want)
    } else {
        StatusFilter::Any
    }
}

/// Parse the `parent` query param. A malformed uuid is a 400 rather than a
/// silent "no filter", which would quietly return the whole workspace.
fn parse_parent_filter(raw: &Option<String>) -> Result<Option<uuid::Uuid>, (StatusCode, String)> {
    match raw.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => s.parse::<uuid::Uuid>().map(Some).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                format!("Invalid parent thread id: {s}"),
            )
        }),
    }
}

fn canonical_source_filter_value(value: String) -> String {
    match value.as_str() {
        "coding-agent" => "claude_code".to_string(),
        _ => value,
    }
}

fn parse_source_csv(value: &str) -> Vec<String> {
    parse_csv(value)
        .into_iter()
        .map(canonical_source_filter_value)
        .collect()
}

/// Map an optional comma-separated query param to the filter shape the store
/// helpers expect. API callers should use `coding-agent`; legacy callers may
/// still send `claude_code`, which is the persisted `thread_summaries.source`
/// value. An empty list (no items after trim) collapses to `None` so the SQL
/// `IS NULL` branch fires; `Some(vec![])` would bind as `ANY('{}')` and
/// silently return zero rows.
fn parse_source_filter(raw: &Option<String>) -> Option<Vec<String>> {
    raw.as_deref()
        .map(parse_source_csv)
        .filter(|v| !v.is_empty())
}

/// GET /api/v1/threads/list — script/trigger/LLM-tool surface. Returns a flat
/// newest-first list of `ThreadSummary` rows from the projection.
///
/// Distinct from `GET /api/v1/threads`, which is the UI-shaped fetch (returns
/// a grouped saved / archive / active / composing / family payload).
pub(in crate::api) async fn list_thread_summaries(
    State(state): State<AppState>,
    Query(q): Query<ListThreadSummariesQuery>,
) -> Result<Json<Vec<ThreadSummary>>, (StatusCode, String)> {
    let sources = parse_source_filter(&q.source);
    let parent = parse_parent_filter(&q.parent)?;
    let statuses = parse_status_filter(&q)?;
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let summaries = state
        .engine
        .event_store()
        .list_thread_summaries(ThreadSummaryFilters {
            status: status_filter(q.active, &statuses),
            sources: sources.as_deref(),
            parent,
            limit,
        })
        .await
        .map_err(|e| {
            log!("[API] Failed to list thread summaries: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to list thread summaries: {}", e),
            )
        })?;
    Ok(Json(summaries))
}

/// GET /api/v1/threads/:id — a single thread's summary by id (404 if the
/// id isn't in the projection). Registered on the same `/threads/:id` leaf as
/// `delete_thread` (see `api::threads::router`), so the param name is `:id`. The
/// lightweight by-id complement to `/api/v1/threads/list`: the message-route
/// popover uses it to resolve a cross-workspace Origin's thread name from the
/// *source* workspace's engine
/// (the engine serves CORS-permissive, and the cross-workspace link already
/// requires that engine to be running, so resolving the title live adds no new
/// requirement and stays fresh across renames).
pub(in crate::api) async fn get_thread_summary(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Json<ThreadSummary>, (StatusCode, String)> {
    let mut threads = state
        .engine
        .event_store()
        .get_threads_by_ids(std::slice::from_ref(&thread_id))
        .await
        .map_err(|e| {
            log!("[API] Failed to fetch thread {}: {}", thread_id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to fetch thread: {}", e),
            )
        })?;
    match threads.pop() {
        Some(summary) => Ok(Json(summary)),
        None => Err((
            StatusCode::NOT_FOUND,
            format!("Thread {} not found", thread_id),
        )),
    }
}

/// GET /api/v1/threads/count — same filters as `/api/v1/threads/list`, returns
/// `{ "count": N }`. Cheaper than fetching all rows to read `.length`.
pub(in crate::api) async fn count_thread_summaries(
    State(state): State<AppState>,
    Query(q): Query<ListThreadSummariesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let sources = parse_source_filter(&q.source);
    let parent = parse_parent_filter(&q.parent)?;
    let statuses = parse_status_filter(&q)?;
    let count = state
        .engine
        .event_store()
        // The count handler reuses `ThreadSummaryFilters` for signature
        // symmetry with the list handler: `limit` is irrelevant for a
        // COUNT(*) and the store helper ignores it.
        .count_thread_summaries(ThreadSummaryFilters {
            status: status_filter(q.active, &statuses),
            sources: sources.as_deref(),
            parent,
            limit: 0,
        })
        .await
        .map_err(|e| {
            log!("[API] Failed to count thread summaries: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to count thread summaries: {}", e),
            )
        })?;
    Ok(Json(serde_json::json!({ "count": count })))
}

/// Query params for `GET /api/v1/threads/archived-count` — the filter-scoped
/// Archive-badge count. Mirrors [`OlderThreadsQuery`]'s facet params minus the
/// pagination cursor (a `COUNT(*)` needs no `before`/`limit`).
#[derive(Deserialize)]
pub struct ArchivedCountQuery {
    pub sources: Option<String>,
    pub trigger_ids: Option<String>,
    pub repo_ids: Option<String>,
    pub app_ids: Option<String>,
}

/// GET /api/v1/threads/archived-count?sources=&trigger_ids=&repo_ids=&app_ids=
/// → `{ "count": N }`. True size of the archived pile MATCHING the active drawer
/// filter, so the collapsed Archive badge reflects the filter and stays stable
/// regardless of how many rows are paginated in. Same predicate as
/// `/threads/older` (`channel_facet_filter_sql`): a `sources` channel gate
/// ANDed with per-channel facet narrowing, so whole channels and facet ids
/// compose (chat + coding-agent + one trigger counts the union, not the trigger
/// alone).
pub(in crate::api) async fn get_archived_count(
    State(state): State<AppState>,
    Query(q): Query<ArchivedCountQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let sources = parse_source_filter(&q.sources);
    let trigger_ids: Option<Vec<String>> = q.trigger_ids.as_deref().map(parse_csv);
    let repo_ids: Option<Vec<String>> = q.repo_ids.as_deref().map(parse_csv);
    let app_ids: Option<Vec<String>> = q.app_ids.as_deref().map(parse_csv);
    let count = state
        .engine
        .event_store()
        .count_archived_threads(
            sources.as_deref(),
            trigger_ids.as_deref(),
            repo_ids.as_deref(),
            app_ids.as_deref(),
        )
        .await
        .map_err(|e| {
            log!("[API] Failed to count archived threads: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to count archived threads: {}", e),
            )
        })?;
    Ok(Json(serde_json::json!({ "count": count })))
}

#[derive(Deserialize)]
pub struct OlderThreadsQuery {
    pub before: String,
    pub limit: Option<i64>,
    /// Comma-separated list of sources to filter by (e.g. "chat,coding-agent").
    /// `claude_code` remains accepted as a legacy alias.
    pub sources: Option<String>,
    /// Comma-separated list of trigger ids to filter by — narrows to
    /// trigger-channel threads spawned by one of the given triggers.
    pub trigger_ids: Option<String>,
    /// Comma-separated list of repository UUIDs to filter by — narrows to
    /// coding-agent threads bound to one of the given repos. Composes with
    /// `trigger_ids` via OR (see `EventStore::get_older_threads`).
    pub repo_ids: Option<String>,
    /// Comma-separated list of app ids to filter by — narrows to app
    /// coding-agent threads operating on one of the given `data/apps/<id>`
    /// folders. Composes with `trigger_ids` / `repo_ids` via OR.
    pub app_ids: Option<String>,
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// GET /api/v1/threads/older?before=ISO8601&limit=15&sources=chat,coding-agent&trigger_ids=t1,t2&repo_ids=r1,r2
pub(in crate::api) async fn get_older_threads(
    State(state): State<AppState>,
    Query(query): Query<OlderThreadsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let before: DateTime<Utc> = query.before.parse().map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid 'before' timestamp: {}", e),
        )
    })?;
    // `clamp`, not `min`: the value is bound straight into `LIMIT $6`, and
    // Postgres rejects a negative one, so `?limit=-1` came back as a 500
    // instead of a page. Every sibling list endpoint clamps both ends
    // (`memory.rs`, `changes.rs`, `history.rs`, `list_thread_summaries`).
    let limit = query.limit.unwrap_or(15).clamp(1, 50);
    let sources = parse_source_filter(&query.sources);
    let trigger_ids: Option<Vec<String>> = query.trigger_ids.as_deref().map(parse_csv);
    let repo_ids: Option<Vec<String>> = query.repo_ids.as_deref().map(parse_csv);
    let app_ids: Option<Vec<String>> = query.app_ids.as_deref().map(parse_csv);

    let store = state.engine.event_store();
    let threads = store
        .get_older_threads(
            before,
            limit,
            sources.as_deref(),
            trigger_ids.as_deref(),
            repo_ids.as_deref(),
            app_ids.as_deref(),
        )
        .await
        .map_err(|e| {
            log!("[API] Failed to get older threads: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get older threads: {}", e),
            )
        })?;

    // `has_more` reflects only the BASE pagination — family extension
    // never advances the cursor. Otherwise an eagerly-loaded family member
    // could push the count up to `limit` even when the natural page is
    // empty, locking the user in an infinite-scroll loop.
    let has_more = threads.len() as i64 == limit;

    let family_threads = load_family_threads(store, threads.iter()).await?;

    Ok(Json(serde_json::json!({
        "threads": threads,
        "family_threads": family_threads,
        "has_more": has_more,
    })))
}

/// GET /api/v1/threads/filter-facets — distinct triggers / repos / apps that
/// have at least one thread, powering the drawer "Show" dropdown's complete
/// option lists. Labels are resolved client-side from the existing registries.
pub(in crate::api) async fn get_filter_facets(
    State(state): State<AppState>,
) -> Result<Json<FilterFacets>, (StatusCode, String)> {
    let facets = state
        .engine
        .event_store()
        .get_filter_facets()
        .await
        .map_err(|e| {
            log!("[API] Failed to get filter facets: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get filter facets: {}", e),
            )
        })?;
    Ok(Json(facets))
}

#[cfg(test)]
mod tests {
    use super::{
        parse_source_filter, parse_status_filter, status_filter, ListThreadSummariesQuery,
    };
    use crate::core::store::StatusFilter;
    use crate::engine::thread_lifecycle::ThreadStatus;
    use axum::http::StatusCode;

    fn query(active: Option<bool>, status: Option<&str>) -> ListThreadSummariesQuery {
        ListThreadSummariesQuery {
            active,
            status: status.map(str::to_string),
            source: None,
            limit: None,
            parent: None,
        }
    }

    #[test]
    fn parse_source_filter_normalizes_coding_agent_filter_value() {
        let raw = Some("chat,coding-agent,claude_code, trigger ".to_string());
        assert_eq!(
            parse_source_filter(&raw),
            Some(vec![
                "chat".to_string(),
                "claude_code".to_string(),
                "claude_code".to_string(),
                "trigger".to_string(),
            ])
        );
    }

    #[test]
    fn parse_source_filter_collapses_blank_to_none() {
        let raw = Some(" , ".to_string());
        assert_eq!(parse_source_filter(&raw), None);
    }

    #[test]
    fn no_filter_params_narrows_nothing() {
        let statuses = parse_status_filter(&query(None, None)).expect("no filter is valid");
        assert!(matches!(status_filter(None, &statuses), StatusFilter::Any));
    }

    /// The union is what every pre-existing caller sends, and it has to keep
    /// resolving to the same filter it always did.
    #[test]
    fn active_alone_still_resolves_to_the_union() {
        for want in [true, false] {
            let statuses =
                parse_status_filter(&query(Some(want), None)).expect("active alone is valid");
            match status_filter(Some(want), &statuses) {
                StatusFilter::Active(got) => assert_eq!(got, want),
                _ => panic!("active={want} must resolve to the union filter"),
            }
        }
    }

    #[test]
    fn status_alone_resolves_to_exactly_those_statuses() {
        let statuses =
            parse_status_filter(&query(None, Some("running,failed"))).expect("a valid status list");
        match status_filter(None, &statuses) {
            StatusFilter::OneOf(got) => {
                assert_eq!(got, [ThreadStatus::Running, ThreadStatus::Failed]);
            }
            _ => panic!("an explicit status list must not fall back to the union"),
        }
    }

    /// The whole point of the filter: `status=running` must not drag in the
    /// question-parked thread that `active=true` would.
    #[test]
    fn running_is_reachable_without_waiting_for_user_answer() {
        let statuses = parse_status_filter(&query(None, Some("running"))).expect("valid");
        assert_eq!(statuses, [ThreadStatus::Running]);
        assert!(!statuses.contains(&ThreadStatus::WaitingForUserAnswer));
    }

    /// Two answers to one question. Silently ANDing them would make
    /// `active=true&status=idle` an empty result that looks like "nothing
    /// matched" rather than "you asked two things".
    #[test]
    fn active_and_status_together_is_refused() {
        let (code, msg) = parse_status_filter(&query(Some(true), Some("running")))
            .expect_err("both filters at once must be refused");
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert!(msg.contains("not both"), "must say what is wrong: {msg}");
        assert!(
            msg.contains("waiting_for_user_answer"),
            "must spell out what the union actually is: {msg}"
        );
    }

    /// A typo that silently returned zero rows would read as "the workspace is
    /// quiet", which is the exact failure this filter exists to prevent.
    #[test]
    fn an_unknown_status_is_refused_with_the_valid_values() {
        let (code, msg) = parse_status_filter(&query(None, Some("runnign")))
            .expect_err("a typo must not select nothing");
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert!(msg.contains("runnign"), "must echo the bad value: {msg}");
        assert!(
            msg.contains("running") && msg.contains("idle"),
            "must list the valid values: {msg}"
        );
    }

    /// An empty `status=` must not read as "no filter", which would answer a
    /// question the caller did not ask with the whole workspace.
    #[test]
    fn an_empty_status_is_refused_rather_than_ignored() {
        for raw in ["", "  ", "running,"] {
            let (code, msg) = parse_status_filter(&query(None, Some(raw)))
                .err()
                .unwrap_or_else(|| panic!("status={raw:?} must be refused, not read as no filter"));
            assert_eq!(code, StatusCode::BAD_REQUEST);
            assert!(
                msg.contains("running"),
                "the refusal must list the valid values: {msg}"
            );
        }
    }

    #[test]
    fn the_kebab_spelling_selects_the_same_status() {
        let kebab = parse_status_filter(&query(None, Some("waiting-for-user-answer")))
            .expect("kebab spelling is accepted");
        let snake = parse_status_filter(&query(None, Some("waiting_for_user_answer")))
            .expect("the wire spelling is accepted");
        assert_eq!(kebab, snake);
        assert_eq!(kebab, [ThreadStatus::WaitingForUserAnswer]);
    }
}
