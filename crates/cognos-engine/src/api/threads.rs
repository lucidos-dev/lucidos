use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use uuid::Uuid;

use super::AppState;
use crate::core::ThreadEventRow;
use crate::memory::{
    EmbeddingProvider, MemorySource, RETRIEVAL_MIN_IMPORTANCE, RETRIEVAL_MIN_SIMILARITY,
};

#[derive(Deserialize)]
pub struct ListThreadsQuery {
    /// Thread ID the frontend currently has focused — ensures it's included in the
    /// response even if it's not in the recent/pinned/active lists.
    pub focused: Option<String>,
}

/// GET /api/threads — returns pinned threads, recent history, and active thread IDs
pub(super) async fn list_threads(
    State(state): State<AppState>,
    Query(query): Query<ListThreadsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Active thread IDs — only threads with a live processing task (chat loop running).
    // CC session existence no longer makes a thread "active" — the status column in
    // thread_summaries handles that (set by CodingAgentIdled → 'waiting').
    let active_id_strings: Vec<String> = state
        .engine
        .processing_thread_ids()
        .iter()
        .map(|id| id.to_string())
        .collect();

    // Run all three DB queries in parallel
    let store = state.engine.event_store();
    let (pinned_result, recent_result, active_result) = tokio::join!(
        store.get_pinned_threads(),
        store.get_recent_threads(15),
        store.get_threads_by_ids(&active_id_strings),
    );

    let pinned = pinned_result.map_err(|e| {
        log!("[API] Failed to get pinned threads: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get pinned threads: {}", e),
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

    // If the frontend has a focused thread, ensure it's in the response.
    // The focused thread may be older than the recent 15 per source, not pinned,
    // and not actively processing — without this, reload would lose it.
    let focused_thread = if let Some(ref focused_id) = query.focused {
        let already_included = pinned.iter().any(|t| t.thread_id == *focused_id)
            || recent.iter().any(|t| t.thread_id == *focused_id)
            || active_threads.iter().any(|t| t.thread_id == *focused_id);
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

    let mut response = serde_json::json!({
        "pinned": pinned,
        "history": recent,
        "active": active_id_strings,
        "active_threads": active_threads,
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

/// POST /api/threads/pin — pin a thread
pub(super) async fn pin_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_id = request
        .get("thread_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing thread_id".to_string()))?;
    let thread_uuid = Uuid::parse_str(thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;
    let actor = super::actor::user_actor(&headers, None, None);

    state
        .engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id: thread_uuid,
            event: crate::engine::thread_events::ThreadEvent::ThreadPinned,
            meta: crate::engine::thread_events::EventMeta::with_actor(actor),
        })
        .await
        .map_err(|e| {
            log!("[API] Failed to pin thread: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to pin thread: {}", e),
            )
        })?;

    // Generate title in background — don't block the pin response
    let engine = state.engine.clone();
    let tid = thread_id.to_string();
    tokio::spawn(async move {
        let has_title = engine
            .event_store()
            .thread_has_title(&tid)
            .await
            .unwrap_or(true);
        if !has_title {
            engine.spawn_title_generation(&tid).await;
        }
    });

    Ok(StatusCode::OK)
}

/// POST /api/threads/unpin — unpin a thread
pub(super) async fn unpin_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_id = request
        .get("thread_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing thread_id".to_string()))?;
    let thread_uuid = Uuid::parse_str(thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;
    let actor = super::actor::user_actor(&headers, None, None);

    state
        .engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id: thread_uuid,
            event: crate::engine::thread_events::ThreadEvent::ThreadUnpinned,
            meta: crate::engine::thread_events::EventMeta::with_actor(actor),
        })
        .await
        .map_err(|e| {
            log!("[API] Failed to unpin thread: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to unpin thread: {}", e),
            )
        })?;

    Ok(StatusCode::OK)
}

/// POST /api/threads/rename — rename a thread (user-initiated)
pub(super) async fn rename_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_id = request
        .get("thread_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing thread_id".to_string()))?;
    let title = request
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing title".to_string()))?;
    let title = title.trim();
    if title.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Title cannot be empty".to_string()));
    }

    let thread_uuid = Uuid::parse_str(thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;
    let actor = super::actor::user_actor(&headers, None, None);

    state
        .engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id: thread_uuid,
            event: crate::engine::thread_events::ThreadEvent::ThreadTitleRenamed {
                title: title.to_string(),
            },
            meta: crate::engine::thread_events::EventMeta::with_actor(actor),
        })
        .await
        .map_err(|e| {
            log!("[API] Failed to rename thread: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to rename thread: {}", e),
            )
        })?;

    Ok(StatusCode::OK)
}

/// POST /api/threads/suggest-title — generate a title suggestion for a thread
pub(super) async fn suggest_title(
    State(state): State<AppState>,
    Json(request): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_id = request
        .get("thread_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing thread_id".to_string()))?;

    // Get recent messages for context (last few messages give better titles than just the first)
    let messages = state
        .engine
        .event_store()
        .get_thread_messages(thread_id)
        .await
        .map_err(|e| {
            log!(
                "[API] Failed to get thread messages for title suggestion: {}",
                e
            );
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    if messages.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Thread has no messages".to_string(),
        ));
    }

    // Build a summary of the conversation for title generation,
    // including image descriptions so visual context (screenshots, tickets, etc.) informs the title.
    let summary: String = messages
        .iter()
        .filter(|m| !m.content.is_empty() || m.image_description.is_some())
        .take(6)
        .map(|m| {
            if let Some(ref desc) = m.image_description {
                format!("{}\n[Attached image: {}]", m.content, desc)
            } else {
                m.content.clone()
            }
        })
        .collect::<Vec<_>>()
        .join("\n---\n");

    let extractor = state.engine.extractor().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "No extraction provider available".to_string(),
        )
    })?;

    let title_model = crate::core::PreferenceStore::get(&state.pool, crate::core::PREF_MODEL_TITLE)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let provider = extractor.provider_for_model(&title_model);
    let title = crate::engine::generate_thread_title(&provider, &summary, None)
        .await
        .map_err(|e| {
            log!("[API] Failed to generate title suggestion: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to generate title: {}", e),
            )
        })?;

    Ok(Json(serde_json::json!({ "title": title })))
}

/// GET /api/threads/:thread_id/messages — get all messages for a thread
pub(super) async fn get_thread_messages(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let messages = state
        .engine
        .event_store()
        .get_thread_messages(&thread_id)
        .await
        .map_err(|e| {
            log!("[API] Failed to get thread messages: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get thread messages: {}", e),
            )
        })?;

    let timeline_events = state
        .engine
        .event_store()
        .get_thread_timeline_events(&thread_id)
        .await
        .map_err(|e| {
            log!("[API] Failed to get thread timeline events: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get thread timeline events: {}", e),
            )
        })?;

    Ok(Json(serde_json::json!({
        "messages": messages,
        "timeline_events": timeline_events,
    })))
}

#[derive(Deserialize)]
pub struct OlderThreadsQuery {
    pub before: String,
    pub limit: Option<i64>,
    /// Comma-separated list of sources to filter by (e.g. "chat,claude_code")
    pub sources: Option<String>,
}

/// GET /api/threads/older?before=ISO8601&limit=15&sources=chat,claude_code — paginated older threads
pub(super) async fn get_older_threads(
    State(state): State<AppState>,
    Query(query): Query<OlderThreadsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let before: DateTime<Utc> = query.before.parse().map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid 'before' timestamp: {}", e),
        )
    })?;
    let limit = query.limit.unwrap_or(15).min(50);
    let sources: Option<Vec<String>> = query.sources.as_ref().map(|s| {
        s.split(',')
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect()
    });

    let threads = state
        .engine
        .event_store()
        .get_older_threads(before, limit, sources.as_deref())
        .await
        .map_err(|e| {
            log!("[API] Failed to get older threads: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get older threads: {}", e),
            )
        })?;

    Ok(Json(serde_json::json!({
        "threads": threads,
        "has_more": threads.len() as i64 == limit,
    })))
}

#[derive(Deserialize)]
pub struct ThreadEventsQuery {
    pub after: Option<i64>,
}

/// GET /api/threads/:thread_id/events — snapshot of persisted thread events
pub(super) async fn get_thread_events_snapshot(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Query(query): Query<ThreadEventsQuery>,
) -> Result<Json<Vec<ThreadEventRow>>, (StatusCode, String)> {
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;

    let events = state
        .event_store
        .get_thread_events_by_seq(thread_uuid, query.after)
        .await
        .map_err(|e| {
            log!("[API] Failed to get thread events: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    Ok(Json(events))
}

#[derive(Deserialize)]
pub struct ThreadSearchQuery {
    pub q: String,
}

/// POST /api/threads/dismiss — dismiss a thread (emits ThreadDismissed, moves to history)
pub(super) async fn dismiss_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_id = request
        .get("thread_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing thread_id".to_string()))?;
    let thread_uuid = Uuid::parse_str(thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;
    let actor = super::actor::user_actor(&headers, None, None);

    // For external repo threads, mark pending changes as applied before dismissing.
    // "Done" replaces "Apply" for external repos — the CC already pushed to the remote,
    // so we shouldn't discard the change.
    let is_external: bool = sqlx::query_scalar::<_, bool>(
        "SELECT cc_is_external_repo FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_uuid)
    .fetch_optional(state.engine.pool())
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("DB error checking cc_is_external_repo: {}", e),
        )
    })?
    .unwrap_or(false);

    if is_external {
        match crate::core::changes::pending_for_thread(state.engine.pool(), thread_uuid).await {
            Ok(pending) => {
                for change in pending {
                    state
                        .engine
                        .emit_change_applied(
                            thread_uuid,
                            change.id,
                            false,
                            false,
                            Vec::new(),
                            change.thread_title.clone(),
                            actor.clone(),
                        )
                        .await;
                }
                state.engine.broadcast_changes_updated().await;
            }
            Err(e) => log!(
                "[API] Failed to fetch pending changes for external dismiss: {}",
                e
            ),
        }
    }

    // If CC paused on AskUserQuestion, resolve the card before dismissing so the
    // QuestionCard renders "Canceled" instead of leaving stale answer buttons in
    // the now-archived thread. CC was killed when the question fired, so this is
    // just an event emit — no resume.
    if let Some(tool_use_id) = crate::engine::agent_question::lookup_pending_question_tool_use_id(
        state.engine.pool(),
        thread_uuid,
    )
    .await
    {
        let result = crate::engine::agent_question::answer_pending_question(
            &state.engine,
            thread_uuid,
            tool_use_id,
            crate::engine::thread_events::AnswerKind::Canceled,
        )
        .await;
        if let crate::engine::agent_question::AnswerResult::Conflict(msg) = result {
            // Conflicts here mean the question was answered between lookup and emit
            // (rare). Log and continue — dismiss should not fail because of it.
            log!("[API] dismiss: pending question already resolved: {}", msg);
        }
    }

    state
        .engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id: thread_uuid,
            event: crate::engine::thread_events::ThreadEvent::ThreadDismissed,
            meta: crate::engine::thread_events::EventMeta::with_actor(actor.clone()),
        })
        .await
        .map_err(|e| {
            log!("[API] Failed to dismiss thread: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to dismiss thread: {}", e),
            )
        })?;

    // End any CC session for this thread (kill process, clean worktree)
    let engine = state.engine.clone();
    tokio::spawn(async move {
        if let Err(e) = engine.end_cc_session_for_thread(thread_uuid).await {
            crate::log!("[API] Failed to end CC session on dismiss: {}", e);
        }
    });

    Ok(StatusCode::OK)
}

/// Weight applied to semantic similarity when combining with a text score.
/// Chosen so that `SEMANTIC_WEIGHT * 1.0` (best possible semantic) stays
/// strictly below the 0.7 text content-match floor in `search_threads_by_text`,
/// keeping pure-semantic noise from outranking legitimate keyword hits.
const SEMANTIC_WEIGHT: f64 = 0.5;

/// Threads at or below this size keep their full text-match score. Larger
/// catch-all threads (e.g. one that accidentally accumulates 2000+ events of
/// unrelated work) get score scaled by `threshold / count` — hyperbolic decay
/// so a couple of incidental keyword matches don't put them above focused
/// thematic threads.
const TEXT_MATCH_DAMPEN_THRESHOLD: i64 = 100;

/// Upper bound on how many memory events the semantic search returns before
/// thread-level aggregation. Multilingual-e5 has a ~0.85 cosine noise floor
/// so a top-N of 20 fills with full-concept matches and crowds out partial
/// matches (e.g. a thread with separate father-only and eye-only facts).
/// Aggregation downstream picks the best event per thread, so a wider set
/// gives such threads a chance to surface.
const SEMANTIC_CANDIDATE_LIMIT: usize = 200;

/// Reduce a text-match score for threads larger than the threshold so a few
/// incidental keyword hits in a giant catch-all thread can't outrank a
/// focused short thread that's actually about the topic.
fn dampen_text_score(score: f64, message_count: i64) -> f64 {
    let count = message_count.max(1) as f64;
    let threshold = TEXT_MATCH_DAMPEN_THRESHOLD as f64;
    if count <= threshold {
        score
    } else {
        score * (threshold / count)
    }
}

/// Combine text and semantic similarity into a single ranking score.
///
/// Text matches always outrank pure semantic noise: multilingual-e5-small
/// produces a ~0.85+ similarity floor for any query, so a raw MAX would let
/// unrelated threads outscore legitimate keyword hits.
pub(super) fn combined_score(text: Option<f64>, semantic: Option<f64>) -> f64 {
    match (text, semantic) {
        (Some(t), Some(s)) => t + SEMANTIC_WEIGHT * s,
        (Some(t), None) => t,
        (None, Some(s)) => SEMANTIC_WEIGHT * s,
        (None, None) => 0.0,
    }
}

/// Run text + semantic thread search and merge results. Combines scores so
/// text matches always rank above pure semantic noise (see `combined_score`).
/// Empty/whitespace queries return an empty vec. Used by both
/// `/api/threads/search` and the SearchEverywhere `/api/search` endpoint.
pub(super) async fn combined_thread_search(
    state: &AppState,
    query: &str,
    limit: i64,
) -> Result<Vec<crate::core::store::ThreadSearchResult>, Box<dyn std::error::Error + Send + Sync>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let store = state.engine.event_store();

    let semantic_future = async {
        let index = state.memory_index.as_ref()?;
        let embedding = match state.embedder.embed(q).await {
            Ok(e) => e,
            Err(e) => {
                log!("[Search] embedder failed, degrading to text-only: {}", e);
                return None;
            }
        };
        let scored_results = match index
            .search_with_scores(&embedding, RETRIEVAL_MIN_IMPORTANCE, SEMANTIC_CANDIDATE_LIMIT)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                log!("[Search] semantic search failed, degrading to text-only: {}", e);
                return None;
            }
        };
        let scored_event_ids: Vec<(Uuid, f64)> = scored_results
            .iter()
            .filter(|(_, similarity)| *similarity >= RETRIEVAL_MIN_SIMILARITY)
            .filter_map(|(entry, similarity)| match &entry.source {
                MemorySource::Event { id } => Some((*id, *similarity)),
                _ => None,
            })
            .collect();
        match store.search_threads_by_memory(&scored_event_ids, limit).await {
            Ok(r) => Some(r),
            Err(e) => {
                log!("[Search] thread aggregation failed, degrading to text-only: {}", e);
                None
            }
        }
    };

    let (text_result, semantic_result) =
        tokio::join!(store.search_threads_by_text(q, limit), semantic_future);
    let text_results = text_result?;

    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut merged = Vec::new();
    for mut r in text_results {
        r.score = dampen_text_score(r.score, r.info.message_count);
        seen.insert(r.info.thread_id.clone(), merged.len());
        merged.push(r);
    }
    if let Some(semantic) = semantic_result {
        for mut r in semantic {
            if let Some(&idx) = seen.get(&r.info.thread_id) {
                merged[idx].score = combined_score(Some(merged[idx].score), Some(r.score));
            } else {
                r.score = combined_score(None, Some(r.score));
                seen.insert(r.info.thread_id.clone(), merged.len());
                merged.push(r);
            }
        }
    }

    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.info.last_activity.cmp(&a.info.last_activity))
    });
    Ok(merged)
}

/// GET /api/threads/search?q=<query> — search threads by title/content (text + semantic)
pub(super) async fn search_threads(
    State(state): State<AppState>,
    Query(query): Query<ThreadSearchQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let merged = combined_thread_search(&state, &query.q, 20)
        .await
        .map_err(|e| {
            log!("[API] Thread search failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Search failed: {}", e),
            )
        })?;
    Ok(Json(serde_json::json!({ "results": merged })))
}

#[cfg(test)]
mod tests {
    use super::{combined_score, dampen_text_score, TEXT_MATCH_DAMPEN_THRESHOLD};

    /// A focused thread (few messages) keeps its full text score.
    #[test]
    fn dampen_preserves_score_for_focused_threads() {
        assert_eq!(dampen_text_score(0.7, 1), 0.7);
        assert_eq!(dampen_text_score(0.7, 50), 0.7);
        assert_eq!(dampen_text_score(0.7, TEXT_MATCH_DAMPEN_THRESHOLD), 0.7);
    }

    /// A catch-all thread with thousands of messages must be crushed below the
    /// pure-semantic floor so it can't outrank focused thematic matches.
    #[test]
    fn dampen_crushes_huge_catch_all_threads() {
        let dampened = dampen_text_score(0.7, 2023);
        assert!(
            dampened < 0.05,
            "2023-msg thread should drop to noise; got {}",
            dampened
        );
    }

    /// A focused 6-message thread that fully matches must outrank a 2000-msg
    /// catch-all that happens to mention the tokens — even with both contributing
    /// the same raw text + semantic signal.
    #[test]
    fn focused_thread_outranks_catch_all_after_dampening() {
        let focused = combined_score(Some(dampen_text_score(0.7, 6)), Some(0.89));
        let catch_all = combined_score(Some(dampen_text_score(0.7, 2023)), Some(0.87));
        assert!(
            focused > catch_all,
            "focused {} must outrank catch-all {}",
            focused,
            catch_all
        );
    }

    /// Pure semantic noise must never outrank a real text content match.
    /// Multilingual-e5-small produces ~0.85+ similarity for almost any pair, so
    /// MAX(text=0.7, semantic=0.88) used to let unrelated threads dominate.
    #[test]
    fn text_match_outranks_pure_semantic_noise() {
        let text = combined_score(Some(0.7), None);
        let noise = combined_score(None, Some(0.88));
        assert!(text > noise, "text {} must outrank semantic {}", text, noise);
    }

    #[test]
    fn both_signals_outrank_either_alone() {
        let both = combined_score(Some(0.7), Some(0.9));
        let text_only = combined_score(Some(0.7), None);
        let semantic_only = combined_score(None, Some(0.9));
        assert!(both > text_only);
        assert!(both > semantic_only);
        assert!((both - 1.15).abs() < 1e-9, "0.7 + 0.5*0.9 = 1.15, got {}", both);
    }

    #[test]
    fn semantic_only_is_halved() {
        assert!((combined_score(None, Some(0.88)) - 0.44).abs() < 1e-9);
    }

    #[test]
    fn empty_signals_score_zero() {
        assert_eq!(combined_score(None, None), 0.0);
    }
}
