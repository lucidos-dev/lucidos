//! Thread mutation handlers — answer-question, save/unsave, rename, suggest
//! title, continue — and the message-history fetch.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::engine::agent_recovery::USER_CLICKED_CONTINUE_REASON;

use super::extract_thread_uuid;

/// Reject a thread mutation when the availability selector doesn't currently
/// grant `action` (server-side mirror of the UI gate via
/// `available_thread_actions_for`). 409 with a user-facing message.
async fn guard_thread_action(
    state: &AppState,
    thread_id: Uuid,
    action: crate::engine::thread_lifecycle::Action,
    reject_msg: &str,
) -> Result<(), (StatusCode, String)> {
    let actions = super::available_thread_actions_for(&state.pool, thread_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read thread state: {e}"),
            )
        })?;
    if !actions.contains(&action) {
        return Err((StatusCode::CONFLICT, reject_msg.to_string()));
    }
    Ok(())
}

#[derive(Deserialize)]
pub(in crate::api) struct AnswerThreadQuestionBody {
    tool_use_id: String,
    answer: crate::engine::thread_events::AnswerKind,
}

/// POST /api/v1/threads/:thread_id/answer-question — answer a pending question
/// on `thread_id`. Used by both the CC `AskUserQuestion` hook and the chat
/// agent's in-process `ask_user_question` tool; `answer_pending_question`
/// branches on the originating event's channel so chat threads skip the
/// CC-specific resume side-effects (`CodingAgentPromptSent` marker +
/// `ContinueSignal` spawn).
///
/// Returns 409 when the question is missing or already answered.
pub(in crate::api) async fn answer_thread_question(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AnswerThreadQuestionBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let thread_uuid = Uuid::parse_str(&thread_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid thread_id" })),
        )
    })?;
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    use crate::engine::agent_question::{answer_pending_question, AnswerResult};
    match answer_pending_question(
        &state.engine,
        thread_uuid,
        body.tool_use_id,
        body.answer,
        actor,
    )
    .await
    {
        AnswerResult::Resumed => Ok(Json(serde_json::json!({ "ok": true }))),
        AnswerResult::Conflict(msg) => Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": msg })),
        )),
    }
}

/// POST /api/v1/threads/save — save a thread
pub(in crate::api) async fn save_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_uuid = extract_thread_uuid(&request)?;
    // Defense in depth: only Save the thread if the availability selector grants
    // it (same `available_thread_actions` the UI derives the button from).
    guard_thread_action(
        &state,
        thread_uuid,
        crate::engine::thread_lifecycle::Action::Save,
        "Thread cannot be saved in its current state",
    )
    .await?;
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;

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

/// POST /api/v1/threads/unsave — unsave a thread
pub(in crate::api) async fn unsave_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_uuid = extract_thread_uuid(&request)?;
    guard_thread_action(
        &state,
        thread_uuid,
        crate::engine::thread_lifecycle::Action::Unsave,
        "Thread is not currently saved",
    )
    .await?;
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;

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

/// POST /api/v1/threads/rename — rename a thread (user-initiated)
pub(in crate::api) async fn rename_thread(
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
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;

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

/// POST /api/v1/threads/suggest-title — generate a title suggestion for a thread
pub(in crate::api) async fn suggest_title(
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
    let provider = extractor.provider_for_model(&title_model).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to build title provider: {}", e),
        )
    })?;
    let title = crate::engine::generate_thread_title(provider.as_ref(), &summary, None)
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

/// POST /api/v1/threads/:thread_id/continue — resume an interrupted thread.
///
/// Dispatches by thread type:
///
/// **CC threads.** Phase 5.3: the engine surfaces a mid-turn-crashed CC
/// session as a synthetic `CodingAgentIdled { reason: "engine_restart_interrupt", .. }`
/// instead of auto-spawning. We emit `ContinuationRequested { reason: "user_clicked_continue" }`
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
pub(in crate::api) async fn continue_thread(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {}", e)))?;
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;

    // Decide which dispatch path to take based on the thread's recorded type.
    let is_coding_agent: bool =
        sqlx::query_scalar("SELECT is_coding_agent FROM thread_summaries WHERE thread_id = $1")
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

    if is_coding_agent {
        state
            .engine
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: thread_uuid,
                event: crate::engine::thread_events::ThreadEvent::ContinuationRequested {
                    reason: USER_CLICKED_CONTINUE_REASON.to_string(),
                },
                meta: crate::engine::thread_events::EventMeta {
                    channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                    actor,
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            })
            .await
            .map_err(|e| {
                log!("[API] Failed to emit ContinuationRequested: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to emit ContinuationRequested: {}", e),
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
            log!(
                "[API] continue_chat failed for thread {}: {}",
                thread_uuid,
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to continue thread: {}", e),
            )
        })?;

    Ok(StatusCode::OK)
}

/// GET /api/v1/threads/:thread_id/messages — get all messages for a thread
pub(in crate::api) async fn get_thread_messages(
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
