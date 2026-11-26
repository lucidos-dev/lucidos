use crate::engine::chat::held_messages::HeldMessagesExt;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::api::AppState;
use crate::core::store::{
    attach_reader_fields, EventStore, FilterFacets, StatusFilter, ThreadSummary,
    ThreadSummaryFilters,
};

#[derive(Deserialize)]
pub struct ListThreadsQuery {
    pub focused: Option<String>,
}

const MAX_FAMILY_THREADS: i64 = 100;

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
            log_legacy!("[API] Failed to fetch family extension: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to fetch family extension: {}", e),
            )
        })
}

async fn home_thread_summary(
    store: &EventStore,
    pool: &sqlx::PgPool,
) -> Result<Option<ThreadSummary>, (StatusCode, String)> {
    let internal = |e: String| {
        log_legacy!("[API] Failed to fetch the home thread: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to fetch the home thread: {e}"),
        )
    };
    let Some(id) = crate::engine::home_thread::home_thread_id(pool)
        .await
        .map_err(|e| internal(e.to_string()))?
    else {
        return Ok(None);
    };
    let mut threads = store
        .get_threads_by_ids(&[id.to_string()])
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok(threads.pop())
}

pub(in crate::api) async fn list_threads(
    State(state): State<AppState>,
    Query(query): Query<ListThreadsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let active_id_strings: Vec<String> = state
        .engine
        .processing_thread_ids()
        .iter()
        .map(|id| id.to_string())
        .collect();

    let store = state.engine.event_store();
    let (
        saved_result,
        recent_result,
        active_result,
        composing_result,
        archive_count_result,
        home_result,
    ) = tokio::join!(
        store.get_saved_threads(),
        store.get_recent_threads(30),
        store.get_threads_by_ids(&active_id_strings),
        store.get_composing_threads(),
        store.count_archived_threads(None, None, None, None),
        home_thread_summary(store, &state.pool),
    );

    let saved = saved_result.map_err(|e| {
        log_legacy!("[API] Failed to get saved threads: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get saved threads: {}", e),
        )
    })?;
    let recent = recent_result.map_err(|e| {
        log_legacy!("[API] Failed to get recent threads: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get recent threads: {}", e),
        )
    })?;
    let active_threads = active_result.map_err(|e| {
        log_legacy!("[API] Failed to get active thread info: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get active thread info: {}", e),
        )
    })?;
    let composing = composing_result.map_err(|e| {
        log_legacy!("[API] Failed to get composing threads: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get composing threads: {}", e),
        )
    })?;
    let archive_count = archive_count_result.map_err(|e| {
        log_legacy!("[API] Failed to count archived threads: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to count archived threads: {}", e),
        )
    })?;

    let home_thread = home_result?;

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
                    log_legacy!("[API] Failed to fetch focused thread {}: {}", focused_id, e);
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

    // INVARIANT: Load any family members (ancestors + descendants) of the base set
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
        "archive_count": archive_count,
        "active": active_id_strings,
        "active_threads": active_threads,
        "composing": composing,
        "family_threads": family_threads,
        "home_thread": home_thread,
    });
    if let Some(ft) = focused_thread {
        response["focused_thread"] = match serde_json::to_value(&ft) {
            Ok(v) => v,
            Err(e) => {
                log_legacy!("[API] Failed to serialize focused thread: {}", e);
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to serialize focused thread: {}", e),
                ));
            }
        };
    }

    Ok(Json(response))
}

#[derive(Deserialize)]
pub struct ListThreadSummariesQuery {
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub has_draft: Option<bool>,
    #[serde(default)]
    pub change_state: Option<crate::engine::thread_lifecycle::ChangeStateKind>,
    #[serde(default)]
    pub has_diff: Option<String>,
}

fn refuse_retired_filters(q: &ListThreadSummariesQuery) -> Result<(), (StatusCode, String)> {
    match q.has_diff {
        Some(_) => Err((
            StatusCode::BAD_REQUEST,
            format!(
                "has_diff was replaced by change_state, which takes one of {}.",
                crate::engine::thread_lifecycle::ChangeStateKind::ALL
                    .map(crate::engine::thread_lifecycle::ChangeStateKind::as_str)
                    .join(", ")
            ),
        )),
        None => Ok(()),
    }
}

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

fn parse_thread_id_param(
    raw: &Option<String>,
    param: &str,
) -> Result<Option<uuid::Uuid>, (StatusCode, String)> {
    match raw.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => s.parse::<uuid::Uuid>().map(Some).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                format!("Invalid {param}: '{s}' is not a thread id"),
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

fn parse_source_filter(raw: &Option<String>) -> Option<Vec<String>> {
    raw.as_deref()
        .map(parse_source_csv)
        .filter(|v| !v.is_empty())
}

pub(in crate::api) async fn list_thread_summaries(
    State(state): State<AppState>,
    Query(q): Query<ListThreadSummariesQuery>,
) -> Result<Json<Vec<ThreadSummary>>, (StatusCode, String)> {
    let sources = parse_source_filter(&q.source);
    let parent = parse_thread_id_param(&q.parent, "parent")?;
    refuse_retired_filters(&q)?;
    let statuses = parse_status_filter(&q)?;
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let mut summaries = state
        .engine
        .event_store()
        .list_thread_summaries(ThreadSummaryFilters {
            status: status_filter(q.active, &statuses),
            sources: sources.as_deref(),
            parent,
            has_draft: q.has_draft,
            change_state: q.change_state,
            limit,
        })
        .await
        .map_err(|e| {
            log_legacy!("[API] Failed to list thread summaries: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to list thread summaries: {}", e),
            )
        })?;
    attach_reader_fields(&mut summaries, &state.engine.workspace_name());
    crate::engine::thread_liveness::attach(state.engine.pool(), &mut summaries).await;
    Ok(Json(summaries))
}

#[derive(Deserialize)]
pub struct DraftsQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub thread_id: Option<String>,
}

pub(in crate::api) async fn list_drafts(
    State(state): State<AppState>,
    Query(q): Query<DraftsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state.engine.event_store();
    let workspace = state.engine.workspace_name();
    let internal = |e: Box<dyn std::error::Error + Send + Sync>| {
        log_legacy!("[API] Failed to read drafts: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read drafts: {}", e),
        )
    };
    let Some(thread_id) = parse_thread_id_param(&q.thread_id, "thread_id")? else {
        let limit = q.limit.unwrap_or(100).clamp(1, 1000);
        let drafts = store
            .list_drafts(&workspace, limit)
            .await
            .map_err(internal)?;
        return Ok(Json(serde_json::json!(drafts)));
    };
    match store
        .get_draft(&workspace, thread_id)
        .await
        .map_err(internal)?
    {
        Some(draft) => Ok(Json(serde_json::json!(draft))),
        None => Err((
            StatusCode::NOT_FOUND,
            format!("Thread {thread_id} holds no draft"),
        )),
    }
}

#[derive(Deserialize)]
pub struct HeldMessagesQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

pub(in crate::api) async fn list_held_messages(
    State(state): State<AppState>,
    Query(q): Query<HeldMessagesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let held = state.engine.list_held_messages(limit).await.map_err(|e| {
        log_legacy!("[API] Failed to list held messages: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list held messages: {}", e),
        )
    })?;
    Ok(Json(serde_json::json!(held)))
}

/// INVARIANT: GET /api/v1/threads/:id — a single thread's summary by id (404 if the
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
            log_legacy!("[API] Failed to fetch thread {}: {}", thread_id, e);
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

pub(in crate::api) async fn count_thread_summaries(
    State(state): State<AppState>,
    Query(q): Query<ListThreadSummariesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let sources = parse_source_filter(&q.source);
    let parent = parse_thread_id_param(&q.parent, "parent")?;
    refuse_retired_filters(&q)?;
    let statuses = parse_status_filter(&q)?;
    let count = state
        .engine
        .event_store()
        .count_thread_summaries(ThreadSummaryFilters {
            status: status_filter(q.active, &statuses),
            sources: sources.as_deref(),
            parent,
            has_draft: q.has_draft,
            change_state: q.change_state,
            limit: 0,
        })
        .await
        .map_err(|e| {
            log_legacy!("[API] Failed to count thread summaries: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to count thread summaries: {}", e),
            )
        })?;
    Ok(Json(serde_json::json!({ "count": count })))
}

#[derive(Deserialize)]
pub struct ArchivedCountQuery {
    pub sources: Option<String>,
    pub trigger_ids: Option<String>,
    pub repo_ids: Option<String>,
    pub app_ids: Option<String>,
}

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
            log_legacy!("[API] Failed to count archived threads: {}", e);
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
    pub sources: Option<String>,
    pub trigger_ids: Option<String>,
    pub repo_ids: Option<String>,
    pub app_ids: Option<String>,
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

fn older_page_limit(requested: Option<i64>) -> i64 {
    requested.unwrap_or(15).clamp(1, 50)
}

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
    let limit = older_page_limit(query.limit);
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
            log_legacy!("[API] Failed to get older threads: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get older threads: {}", e),
            )
        })?;

    // INVARIANT: `has_more` reflects only the BASE pagination — family extension
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

pub(in crate::api) async fn get_filter_facets(
    State(state): State<AppState>,
) -> Result<Json<FilterFacets>, (StatusCode, String)> {
    read_filter_facets(&state).await.map(Json)
}

pub(in crate::api) async fn read_filter_facets(
    state: &AppState,
) -> Result<FilterFacets, (StatusCode, String)> {
    state
        .engine
        .event_store()
        .get_filter_facets(&state.app_manager)
        .await
        .map_err(|e| {
            log_legacy!("[API] Failed to get filter facets: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get filter facets: {}", e),
            )
        })
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
            has_draft: None,
            change_state: None,
            has_diff: None,
        }
    }

    #[test]
    fn the_older_page_limit_is_clamped_at_both_ends() {
        assert_eq!(super::older_page_limit(None), 15);
        assert_eq!(super::older_page_limit(Some(-1)), 1);
        assert_eq!(super::older_page_limit(Some(0)), 1);
        assert_eq!(super::older_page_limit(Some(9_999)), 50);
    }

    #[test]
    fn the_retired_has_diff_filter_is_refused_by_name() {
        assert!(super::refuse_retired_filters(&query(None, None)).is_ok());
        let retired = ListThreadSummariesQuery {
            has_diff: Some("true".to_string()),
            ..query(None, None)
        };
        let (code, msg) = super::refuse_retired_filters(&retired).expect_err("has_diff is retired");
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert!(msg.contains("change_state"), "{msg}");
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

    #[test]
    fn running_is_reachable_without_waiting_for_user_answer() {
        let statuses = parse_status_filter(&query(None, Some("running"))).expect("valid");
        assert_eq!(statuses, [ThreadStatus::Running]);
        assert!(!statuses.contains(&ThreadStatus::WaitingForUserAnswer));
    }

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
