use super::*;
use crate::core::ArtifactManager;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    #[serde(default = "default_category")]
    pub category: String,
}

fn default_category() -> String {
    "all".to_string()
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

/// Apply a recency boost: `score * (0.5 + 0.5 * exp(-age_days / 14.0))`.
/// Items with no timestamp get score 1.0.
fn recency_boost(score: f64, last_activity: Option<chrono::DateTime<chrono::Utc>>) -> f64 {
    match last_activity {
        Some(ts) => {
            let age_days = (chrono::Utc::now() - ts).num_seconds() as f64 / 86400.0;
            score * (0.5 + 0.5 * (-age_days / 14.0).exp())
        }
        None => score,
    }
}

/// GET /api/v1/search?q=<query>&category=all|threads|files|apps|triggers|settings|changes
pub(super) async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, (StatusCode, String)> {
    let q = query.q.as_deref().unwrap_or("").trim().to_string();
    let category = query.category.as_str();
    let is_all = category == "all";
    let limit = if is_all { 5 } else { 50 };

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
                apply_recency_boosts(threads.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?),
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
            results.insert("threads".into(), apply_recency_boosts(items));
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

/// Apply recency boosts to all items (for the "all" tab).
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

fn thread_summary_to_item(info: &crate::core::store::ThreadSummary, score: f64) -> SearchResultItem {
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

    let results = super::threads::combined_thread_search(state, query, limit as i64)
        .await
        .map_err(|e| format!("Thread search failed: {}", e))?;
    Ok(results
        .into_iter()
        .map(|r| thread_summary_to_item(&r.info, r.score))
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

    if query.is_empty() {
        return Ok(artifacts
            .into_iter()
            .take(limit)
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
            .collect());
    }

    let query_lower = query.to_lowercase();
    let matched: Vec<SearchResultItem> = artifacts
        .into_iter()
        .filter_map(|path| {
            let filename = std::path::Path::new(&path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            if filename.to_lowercase().contains(&query_lower)
                || path.to_lowercase().contains(&query_lower)
            {
                Some(SearchResultItem {
                    id: path.clone(),
                    title: filename,
                    subtitle: path,
                    category: "files".into(),
                    score: 1.0,
                    last_activity: None,
                })
            } else {
                None
            }
        })
        .take(limit)
        .collect();

    Ok(matched)
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

    if query.is_empty() {
        return Ok(apps
            .into_iter()
            .take(limit)
            .map(|app| SearchResultItem {
                id: app.id,
                title: app.name,
                subtitle: app.description,
                category: "apps".into(),
                score: 1.0,
                last_activity: None,
            })
            .collect());
    }

    let query_lower = query.to_lowercase();
    let matched: Vec<SearchResultItem> = apps
        .into_iter()
        .filter(|app| {
            app.name.to_lowercase().contains(&query_lower)
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
        .collect();

    Ok(matched)
}

async fn search_triggers_internal(
    state: &AppState,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, String> {
    let scheduler = state.scheduler.lock().await;
    let triggers = scheduler.list_trigger_configs();
    drop(scheduler);

    if query.is_empty() {
        return Ok(triggers
            .into_iter()
            .take(limit)
            .map(|t| SearchResultItem {
                id: t.id,
                title: t.name,
                subtitle: t.schedule.join(", "),
                category: "triggers".into(),
                score: 1.0,
                last_activity: None,
            })
            .collect());
    }

    let query_lower = query.to_lowercase();
    let matched: Vec<SearchResultItem> = triggers
        .into_iter()
        .filter(|t| t.name.to_lowercase().contains(&query_lower))
        .take(limit)
        .map(|t| SearchResultItem {
            id: t.id,
            title: t.name,
            subtitle: t.schedule.join(", "),
            category: "triggers".into(),
            score: 1.0,
            last_activity: None,
        })
        .collect();

    Ok(matched)
}

async fn search_changes_internal(
    state: &AppState,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResultItem>, String> {
    let proj = state.engine.changes();
    let (pending_r, applied_r) = tokio::join!(proj.list_pending(), proj.list_recently_applied(15, None));
    let pending = pending_r.map_err(|e| format!("DB error listing pending changes: {e}"))?;
    let applied = applied_r.map_err(|e| format!("DB error listing applied changes: {e}"))?;
    let all_changes = pending.into_iter().chain(applied.into_iter());

    if query.is_empty() {
        return Ok(all_changes
            .take(limit)
            .map(|c| SearchResultItem {
                id: c.id.to_string(),
                title: c.description.clone(),
                subtitle: format!("{} - {}", c.branch_name, c.status),
                category: "changes".into(),
                score: 1.0,
                last_activity: Some(c.created_at.to_rfc3339()),
            })
            .collect());
    }

    let query_lower = query.to_lowercase();
    let matched: Vec<SearchResultItem> = all_changes
        .filter(|c| {
            c.description.to_lowercase().contains(&query_lower)
                || c.branch_name.to_lowercase().contains(&query_lower)
        })
        .take(limit)
        .map(|c| SearchResultItem {
            id: c.id.to_string(),
            title: c.description.clone(),
            subtitle: format!("{} - {}", c.branch_name, c.status),
            category: "changes".into(),
            score: 1.0,
            last_activity: Some(c.created_at.to_rfc3339()),
        })
        .collect();

    Ok(matched)
}

/// Route for the global `/search` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new().route("/search", get(search))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_boost_no_timestamp() {
        let boosted = recency_boost(0.8, None);
        assert!((boosted - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn recency_boost_recent_item() {
        let now = chrono::Utc::now();
        let boosted = recency_boost(1.0, Some(now));
        // Very recent: boost should be close to 1.0
        assert!(boosted > 0.95);
    }

    #[test]
    fn recency_boost_old_item() {
        let old = chrono::Utc::now() - chrono::Duration::days(60);
        let boosted = recency_boost(1.0, Some(old));
        // 60 days old: exp(-60/14) ~ 0.014, so boost ~ 0.5 + 0.5*0.014 ~ 0.507
        assert!(boosted < 0.55);
        assert!(boosted > 0.49);
    }
}
