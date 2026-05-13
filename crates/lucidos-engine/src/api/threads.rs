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
use crate::engine::agent_recovery::USER_CLICKED_CONTINUE_REASON;
use crate::memory::{
    EmbeddingProvider, MemorySource, RETRIEVAL_MIN_IMPORTANCE, RETRIEVAL_MIN_SIMILARITY,
};

#[derive(Deserialize)]
pub struct ListThreadsQuery {
    /// Thread ID the frontend currently has focused — ensures it's included in the
    /// response even if it's not in the recent/saved/active lists.
    pub focused: Option<String>,
}

/// GET /api/threads — returns saved threads, recent history, and active thread IDs
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

    // Run all four DB queries in parallel
    let store = state.engine.event_store();
    let (saved_result, recent_result, active_result, composing_result) = tokio::join!(
        store.get_saved_threads(),
        store.get_recent_threads(15),
        store.get_threads_by_ids(&active_id_strings),
        store.get_composing_threads(),
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

    // If the frontend has a focused thread, ensure it's in the response.
    // The focused thread may be older than the recent 15 per source, not saved,
    // and not actively processing — without this, reload would lose it.
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

    let mut response = serde_json::json!({
        "saved": saved,
        "history": recent,
        "active": active_id_strings,
        "active_threads": active_threads,
        "composing": composing,
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

/// Extract a `thread_id` UUID from a JSON body that uses the
/// `{"thread_id": "<uuid>"}` shape. Used by save/unsave/rename/archive handlers.
fn extract_thread_uuid(request: &serde_json::Value) -> Result<Uuid, (StatusCode, String)> {
    let thread_id = request
        .get("thread_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing thread_id".to_string()))?;
    Uuid::parse_str(thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))
}

/// POST /api/threads/save — save a thread
pub(super) async fn save_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_uuid = extract_thread_uuid(&request)?;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;

    state
        .engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id: thread_uuid,
            event: crate::engine::thread_events::ThreadEvent::ThreadSaved,
            meta: crate::engine::thread_events::EventMeta::with_actor(actor),
        })
        .await
        .map_err(|e| {
            log!("[API] Failed to save thread: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to save thread: {}", e),
            )
        })?;

    // Generate title in background — don't block the save response
    let engine = state.engine.clone();
    let tid = thread_uuid.to_string();
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

/// POST /api/threads/unsave — unsave a thread
pub(super) async fn unsave_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_uuid = extract_thread_uuid(&request)?;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;

    state
        .engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id: thread_uuid,
            event: crate::engine::thread_events::ThreadEvent::ThreadUnsaved,
            meta: crate::engine::thread_events::EventMeta::with_actor(actor),
        })
        .await
        .map_err(|e| {
            log!("[API] Failed to unsave thread: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to unsave thread: {}", e),
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
    let thread_uuid = extract_thread_uuid(&request)?;
    let title = request
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing title".to_string()))?;
    let title = title.trim();
    if title.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Title cannot be empty".to_string()));
    }
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;

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

/// POST /api/threads/:thread_id/continue — resume an interrupted thread.
///
/// Dispatches by thread type:
///
/// **CC threads.** Phase 5.3: the engine surfaces a mid-turn-crashed CC
/// session as a synthetic `CodingAgentIdled { reason: "engine_restart_interrupt", .. }`
/// instead of auto-spawning. We emit `ContinueSignal { reason: "user_clicked_continue" }`
/// on the CC channel — the spawn dispatcher dedupes via the event id and
/// re-enters CC via `--resume` against the recorded `cc_session_id`. The
/// resume path then emits `ContinuationStarted { actor: <user> }` BEFORE the
/// first CC text arrives so the timeline reads "You restarted" → resume.
///
/// **Chat / trigger threads.** Calls into `engine.continue_chat(...)` which
/// emits `ContinuationStarted` + `UserPromptInjected` (engine note
/// summarizing completed tool pairs from the aborted run) and re-enters the
/// agentic loop with a fresh `request_event_id`.
///
/// Body is empty. Idempotent — the chat path checks for an existing
/// ContinuationStarted newer than the latest abort; the CC dispatcher's
/// `already_spawned` check rejects duplicates.
pub(super) async fn continue_thread(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;

    // Decide which dispatch path to take based on the thread's recorded type.
    let is_cc: bool = sqlx::query_scalar(
        "SELECT is_cc FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_uuid)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        log!("[API] Failed to read thread type for continue: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read thread: {}", e),
        )
    })?
    .unwrap_or(false);

    if is_cc {
        state
            .engine
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: thread_uuid,
                event: crate::engine::thread_events::ThreadEvent::ContinueSignal {
                    reason: USER_CLICKED_CONTINUE_REASON.to_string(),
                },
                meta: crate::engine::thread_events::EventMeta {
                    channel: Some(crate::engine::thread_events::EventChannel::CodingAgent),
                    actor,
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            })
            .await
            .map_err(|e| {
                log!("[API] Failed to emit ContinueSignal: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to emit ContinueSignal: {}", e),
                )
            })?;
        return Ok(StatusCode::OK);
    }

    // Chat / trigger thread — route through chat::rerun.
    state
        .engine
        .continue_chat(thread_uuid, actor)
        .await
        .map_err(|e| {
            log!("[API] continue_chat failed for thread {}: {}", thread_uuid, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to continue thread: {}", e),
            )
        })?;

    Ok(StatusCode::OK)
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
    /// Comma-separated list of trigger ids to filter by — narrows to
    /// trigger-channel threads spawned by one of the given triggers.
    pub trigger_ids: Option<String>,
    /// Comma-separated list of repository UUIDs to filter by — narrows to
    /// CC-channel threads bound to one of the given repos. Composes with
    /// `trigger_ids` via OR (see `EventStore::get_older_threads`).
    pub repo_ids: Option<String>,
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// GET /api/threads/older?before=ISO8601&limit=15&sources=chat,claude_code&trigger_ids=t1,t2&repo_ids=r1,r2
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
    let sources: Option<Vec<String>> = query.sources.as_deref().map(parse_csv);
    let trigger_ids: Option<Vec<String>> = query.trigger_ids.as_deref().map(parse_csv);
    let repo_ids: Option<Vec<String>> = query.repo_ids.as_deref().map(parse_csv);

    let threads = state
        .engine
        .event_store()
        .get_older_threads(
            before,
            limit,
            sources.as_deref(),
            trigger_ids.as_deref(),
            repo_ids.as_deref(),
        )
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

/// Response shape for `GET /api/threads/:thread_id/events`. Wraps the event
/// rows with a `current_aggregate` snapshot of `thread_summaries` so the
/// frontend's historical-replay path applies meta from a fetched snapshot
/// — same source-of-truth model as live SSE's per-event aggregate.
#[derive(serde::Serialize)]
pub struct ThreadEventsSnapshot {
    pub events: Vec<ThreadEventRow>,
    #[serde(rename = "currentAggregate")]
    pub current_aggregate: Option<crate::core::store::ThreadAggregate>,
}

/// GET /api/threads/:thread_id/events — snapshot of persisted thread events,
/// plus the current `thread_summaries` projection snapshot.
pub(super) async fn get_thread_events_snapshot(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Query(query): Query<ThreadEventsQuery>,
) -> Result<Json<ThreadEventsSnapshot>, (StatusCode, String)> {
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;

    // Independent fetches against the same pool — run in parallel.
    let pool = state.engine.pool();
    let (events_res, aggregate_res) = tokio::join!(
        state
            .event_store
            .get_thread_events_by_seq(thread_uuid, query.after),
        crate::core::store::fetch_thread_aggregate(pool, thread_uuid),
    );

    let mut events = events_res.map_err(|e| {
        log!("[API] Failed to get thread events: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    for row in &mut events {
        strip_image_content_in_tool_result(row);
    }

    // Aggregate fetch is best-effort — absence just means no snapshot to apply
    // (frontend logs a warning).
    let current_aggregate = aggregate_res.unwrap_or_else(|e| {
        log!(
            "[API] Failed to fetch ThreadAggregate for {}: {}",
            thread_uuid,
            e
        );
        None
    });

    Ok(Json(ThreadEventsSnapshot {
        events,
        current_aggregate,
    }))
}

/// Replace `[IMAGE_CONTENT:...]\n<base64>` payloads in `ToolResult.result` with a small stub.
/// Rescues legacy threads (pre-write-time-strip) where `read_file` of an image inlined the
/// full base64 into the event payload — those threads are otherwise unloadable on mobile.
fn strip_image_content_in_tool_result(row: &mut ThreadEventRow) {
    if row.event_type != "ToolResult" {
        return;
    }
    let Some(result_str) = row.payload.get("result").and_then(|v| v.as_str()) else {
        return;
    };
    if let Some(stub) = crate::engine::tools::files::strip_image_content_marker(result_str) {
        row.payload["result"] = serde_json::Value::String(stub);
    }
}

#[derive(Deserialize)]
pub struct ThreadSearchQuery {
    pub q: String,
}

/// Bound on how long archive_thread waits for the stop-fallout terminal event
/// (`ResponseAborted` / `ResponseFailed`) before emitting `ThreadArchived`
/// anyway. Only fires when CC was actively working (not idle) — the
/// stop signal lands the event in <100 ms; this is the safety net for a
/// stuck CC subprocess.
const STOP_FALLOUT_TIMEOUT_MS: u64 = 2000;

/// POST /api/threads/archive — archive a thread (emits ThreadArchived, moves to history)
pub(super) async fn archive_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_uuid = extract_thread_uuid(&request)?;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;

    // For external repo threads, mark pending changes as applied before archiving.
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
        let pending = state.engine.changes().pending_for_thread(thread_uuid).await;
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
                    None,
                    None,
                )
                .await;
        }
        state.engine.broadcast_changes_updated().await;
    }

    // If CC paused on AskUserQuestion, resolve the card before archiving so the
    // QuestionCard renders "Canceled" instead of leaving stale answer buttons in
    // the now-archived thread. CC was killed when the question fired, so this is
    // just an event emit — no resume.
    crate::engine::agent_question::resolve_pending_question_as_canceled(
        &state.engine,
        thread_uuid,
        actor.clone(),
    )
    .await;

    // Stop the CC session on archive. `stop_agent(StopReason::Archive, ...)`
    // sets `pending_stop = Some(Archive)` on the session; the stop arm reads
    // that and suppresses `ResponseCanceled` so `ThreadArchived` stays the sole
    // terminator.
    //
    // Wait for the fallout terminal ONLY when CC was actively working — the
    // wait serializes any pre-existing pending terminal (e.g. `ResponseFailed`
    // from a mid-stream API drop) BEFORE `ThreadArchived`, otherwise the
    // trailing terminal commits last and undoes the archive ("click Archive
    // twice" bug). On an idle session the stop arm emits nothing, so the wait
    // would always time out for the full 2s.
    let liveness = state.engine.agent_liveness(thread_uuid).await;
    if liveness.running {
        let mut bus_rx = state.engine.event_bus.subscribe();
        if let Err(e) = state
            .engine
            .stop_agent(
                crate::engine::claude_code::StopReason::Archive,
                Some(thread_uuid),
                actor.clone(),
            )
            .await
        {
            log!("[API] Failed to end CC session on archive: {}", e);
        }
        if liveness.actively_working {
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(STOP_FALLOUT_TIMEOUT_MS),
                async {
                    loop {
                        match bus_rx.recv().await {
                            Ok(evt) => {
                                let crate::engine::event_bus::BusEvent::Thread {
                                    thread_id: tid,
                                    event,
                                    ..
                                } = &evt.typed
                                else {
                                    continue;
                                };
                                if *tid == thread_uuid
                                    && matches!(
                                        event,
                                        crate::engine::thread_events::ThreadEvent::ResponseAborted { .. }
                                            | crate::engine::thread_events::ThreadEvent::ResponseFailed { .. }
                                    )
                                {
                                    return;
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        }
                    }
                },
            )
            .await;
        }
    }

    state
        .engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id: thread_uuid,
            event: crate::engine::thread_events::ThreadEvent::ThreadArchived,
            meta: crate::engine::thread_events::EventMeta::with_actor(actor.clone()),
        })
        .await
        .map_err(|e| {
            log!("[API] Failed to archive thread: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to archive thread: {}", e),
            )
        })?;

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
    use super::{
        combined_score, dampen_text_score, strip_image_content_in_tool_result,
        TEXT_MATCH_DAMPEN_THRESHOLD,
    };
    use crate::core::ThreadEventRow;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    fn row(event_type: &str, payload: serde_json::Value) -> ThreadEventRow {
        ThreadEventRow {
            sequence: 1,
            event_type: event_type.to_string(),
            payload,
            created: Utc::now(),
            event_id: Uuid::new_v4(),
        }
    }

    #[test]
    fn read_time_strip_replaces_image_content_in_tool_result() {
        let huge_b64 = "A".repeat(2 * 1024 * 1024);
        let mut row = row(
            "ToolResult",
            json!({
                "name": "read_file",
                "result": format!("[IMAGE_CONTENT:image/png]\n{}", huge_b64),
                "images": [],
            }),
        );
        let original_size = row.payload["result"].as_str().unwrap().len();
        assert!(original_size > 1_000_000, "setup: payload should be huge");

        strip_image_content_in_tool_result(&mut row);

        let stripped = row.payload["result"].as_str().unwrap();
        assert!(stripped.len() < 100, "stripped to {} bytes", stripped.len());
        assert!(stripped.contains("image/png"));
    }

    #[test]
    fn read_time_strip_leaves_non_image_tool_results_alone() {
        let mut row = row(
            "ToolResult",
            json!({ "name": "list_files", "result": "file1.txt\nfile2.txt", "images": [] }),
        );
        strip_image_content_in_tool_result(&mut row);
        assert_eq!(row.payload["result"], "file1.txt\nfile2.txt");
    }

    #[test]
    fn read_time_strip_ignores_non_tool_result_events() {
        let mut row = row(
            "MessageReceived",
            json!({ "text": "[IMAGE_CONTENT:image/png]\nABCD" }),
        );
        let before = row.payload.clone();
        strip_image_content_in_tool_result(&mut row);
        assert_eq!(row.payload, before, "unrelated events untouched");
    }

    #[test]
    fn read_time_strip_handles_missing_result_field() {
        let mut row = row("ToolResult", json!({ "name": "x", "images": [] }));
        let before = row.payload.clone();
        strip_image_content_in_tool_result(&mut row);
        assert_eq!(row.payload, before, "missing result field is a no-op");
    }

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
