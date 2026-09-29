use super::*;
use crate::core::ArtifactManager;
use crate::engine::thread_search::recency_boost;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    #[serde(default = "default_category")]
    pub category: String,
    /// Hits per category. Search Everywhere asks each category on its own,
    /// so it passes the All tab's cap rather than taking a single-tab page.
    pub limit: Option<usize>,
}

fn default_category() -> String {
    "all".to_string()
}

/// A category's own page is longer than its slice of the All tab.
const ALL_TAB_LIMIT: usize = 5;
const CATEGORY_LIMIT: usize = 50;

fn result_limit(is_all: bool, requested: Option<usize>) -> usize {
    let default = if is_all {
        ALL_TAB_LIMIT
    } else {
        CATEGORY_LIMIT
    };
    requested.unwrap_or(default).clamp(1, CATEGORY_LIMIT)
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResultItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub category: String,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: HashMap<String, Vec<SearchResultItem>>,
}

/// GET /api/v1/search?q=<query>&category=all|threads|files|apps|triggers|settings|changes&limit=<1-50>
pub(super) async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, (StatusCode, String)> {
    let q = query.q.as_deref().unwrap_or("").trim().to_string();
    let category = query.category.as_str();
    let limit = result_limit(category == "all", query.limit);

    let mut results: HashMap<String, Vec<SearchResultItem>> = HashMap::new();

    // "settings" is filtered entirely from the frontend registry (see searchIndex.ts) — backend
    // never returns settings results, so the labels stay co-located with the rendered UI strings.
    match category {
        "all" => {
            let (threads, files, apps, triggers, changes) = tokio::join!(
                search_threads_internal(&state, &q, limit),
                search_files_internal(&state, &q, limit),
                search_apps_internal(&state, &q, limit),
                search_triggers_internal(&state, &q, limit),
                search_changes_internal(&state, &q, limit),
            );
            results.insert(
                "threads".into(),
                threads.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?,
            );
            results.insert(
                "files".into(),
                apply_recency_boosts(files.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?),
            );
            results.insert(
                "apps".into(),
                apply_recency_boosts(apps.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?),
            );
            results.insert(
                "triggers".into(),
                apply_recency_boosts(triggers.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?),
            );
            results.insert(
                "changes".into(),
                apply_recency_boosts(changes.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?),
            );
        }
        "threads" => {
            let items = search_threads_internal(&state, &q, limit)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
            results.insert("threads".into(), items);
        }
        "files" => {
            let items = search_files_internal(&state, &q, limit)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
            results.insert("files".into(), apply_recency_boosts(items));
        }
        "apps" => {
            let items = search_apps_internal(&state, &q, limit)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
            results.insert("apps".into(), apply_recency_boosts(items));
        }
        "triggers" => {
            let items = search_triggers_internal(&state, &q, limit)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
            results.insert("triggers".into(), apply_recency_boosts(items));
        }
        "settings" => {
            results.insert("settings".into(), Vec::new());
        }
        "changes" => {
            let items = search_changes_internal(&state, &q, limit)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
            results.insert("changes".into(), apply_recency_boosts(items));
        }
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Unknown category: {}", other),
            ));
        }
    }

    Ok(Json(SearchResponse { results }))
}

/// Apply recency boosts to a category's items and sort them. Threads skip it:
/// `combined_thread_search` already ranks and boosts them.
fn apply_recency_boosts(items: Vec<SearchResultItem>) -> Vec<SearchResultItem> {
    let mut boosted: Vec<SearchResultItem> = items
        .into_iter()
        .map(|mut item| {
            let ts = item
                .last_activity
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc));
            item.score = recency_boost(item.score, ts);
            item
        })
        .collect();
    boosted.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    boosted
}

fn thread_summary_to_item(
    info: &crate::core::store::ThreadSummary,
    score: f64,
) -> SearchResultItem {
    SearchResultItem {
        id: info.thread_id.clone(),
        title: info.title.clone(),
        subtitle: info.channel.clone(),
        category: "threads".into(),
        score,
        last_activity: Some(info.last_activity.to_rfc3339()),
    }
}

async fn search_threads_internal(
    state: &AppState,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, String> {
    let store = state.engine.event_store();

    if query.is_empty() {
        let threads = store
            .get_recent_threads(limit as i64)
            .await
            .map_err(|e| format!("Thread search failed: {}", e))?;
        // get_recent_threads returns the global newest threads (inbox + the newest
        // archived slice), so truncate to limit.
        return Ok(threads
            .into_iter()
            .take(limit)
            .map(|t| thread_summary_to_item(&t, 1.0))
            .collect());
    }

    let results =
        crate::engine::thread_search::combined_thread_search(&state.engine, query, limit as i64)
            .await
            .map_err(|e| format!("Thread search failed: {}", e))?;
    // `limit` bounds each ARM inside the merge, not the merge, so two arms that
    // agree on nothing yield up to twice it and this category would out-fill
    // every sibling in the palette. Truncated here, like `files` / `apps` /
    // `triggers` / `changes` each do in their own `*_internal`. The merge has
    // already ranked, so the cut keeps the best hits.
    Ok(results
        .into_iter()
        .map(|r| thread_summary_to_item(&r.info, r.score))
        .take(limit)
        .collect())
}

async fn search_files_internal(
    state: &AppState,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, String> {
    let artifact_manager = ArtifactManager::new(state.workspace_path.clone())
        .map_err(|e| format!("Failed to open artifact manager: {}", e))?;
    let artifacts = artifact_manager
        .list_artifacts()
        .map_err(|e| format!("File listing failed: {}", e))?;

    let query_lower = query.to_lowercase();
    Ok(artifacts
        .into_iter()
        .map(|path| {
            let filename = std::path::Path::new(&path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            SearchResultItem {
                id: path.clone(),
                title: filename,
                subtitle: path,
                category: "files".into(),
                score: 1.0,
                last_activity: None,
            }
        })
        .filter(|item| {
            query_lower.is_empty()
                || item.title.to_lowercase().contains(&query_lower)
                || item.subtitle.to_lowercase().contains(&query_lower)
        })
        .take(limit)
        .collect())
}

async fn search_apps_internal(
    state: &AppState,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, String> {
    let apps = state
        .app_manager
        .list_apps()
        .map_err(|e| format!("App listing failed: {}", e))?;

    let query_lower = query.to_lowercase();
    Ok(apps
        .into_iter()
        .filter(|app| {
            query_lower.is_empty()
                || app.name.to_lowercase().contains(&query_lower)
                || app.description.to_lowercase().contains(&query_lower)
        })
        .take(limit)
        .map(|app| SearchResultItem {
            id: app.id,
            title: app.name,
            subtitle: app.description,
            category: "apps".into(),
            score: 1.0,
            last_activity: None,
        })
        .collect())
}

async fn search_triggers_internal(
    state: &AppState,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, String> {
    let scheduler = state.scheduler.lock().await;
    let triggers = scheduler.list_trigger_configs();
    drop(scheduler);

    let query_lower = query.to_lowercase();
    Ok(triggers
        .into_iter()
        .filter(|t| query_lower.is_empty() || t.name.to_lowercase().contains(&query_lower))
        .take(limit)
        .map(|t| SearchResultItem {
            id: t.id,
            title: t.name,
            subtitle: t.schedule.join(", "),
            category: "triggers".into(),
            score: 1.0,
            last_activity: None,
        })
        .collect())
}

async fn search_changes_internal(
    state: &AppState,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, String> {
    let proj = state.engine.changes();
    let (pending_r, applied_r) =
        tokio::join!(proj.list_pending(), proj.list_recently_applied(15, None));
    let pending = pending_r.map_err(|e| format!("DB error listing pending changes: {e}"))?;
    let applied = applied_r.map_err(|e| format!("DB error listing applied changes: {e}"))?;
    let all_changes = pending.into_iter().chain(applied.into_iter());

    let query_lower = query.to_lowercase();
    Ok(all_changes
        .filter(|c| {
            query_lower.is_empty()
                || c.description.to_lowercase().contains(&query_lower)
                || c.branch_name.to_lowercase().contains(&query_lower)
        })
        .take(limit)
        .map(|c| SearchResultItem {
            id: c.id.to_string(),
            title: c.description.clone(),
            subtitle: format!("{} - {}", c.branch_name, c.status()),
            category: "changes".into(),
            score: 1.0,
            last_activity: Some(c.created_at.to_rfc3339()),
        })
        .collect())
}

/// Route for the global `/search` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new().route("/search", get(search))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_category_defaults_to_its_page_and_all_to_its_slice() {
        assert_eq!(result_limit(true, None), ALL_TAB_LIMIT);
        assert_eq!(result_limit(false, None), CATEGORY_LIMIT);
    }

    #[test]
    fn a_requested_limit_is_honoured_within_bounds() {
        assert_eq!(result_limit(false, Some(5)), 5);
        assert_eq!(result_limit(false, Some(0)), 1);
        assert_eq!(result_limit(true, Some(10_000)), CATEGORY_LIMIT);
    }
}
