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

/// Read `is_saved` for a thread. `None` = the thread isn't in the projection.
/// Drives the idempotent save/unsave short-circuit below.
async fn thread_is_saved(
    state: &AppState,
    thread_id: Uuid,
) -> Result<Option<bool>, (StatusCode, String)> {
    sqlx::query_scalar("SELECT is_saved FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read thread state: {e}"),
            )
        })
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
/// Returns 409 when the question is missing or already answered, and 400 for a
/// `Superseded` body (see below).
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
    // `Superseded` is engine-internal and only the message router may write it,
    // because it asserts something no client can make true: that a follow-up
    // arrived and replaced this question. A client-supplied one would put that
    // sentence in the timeline and in the agent's tool result with no message
    // behind it. The kind sits on the public enum because it rides the same
    // persisted `UserQuestionAnswered` as every other answer. So the boundary
    // is here, not in `validate_answer`, which the router shares.
    if matches!(
        body.answer,
        crate::engine::thread_events::AnswerKind::Superseded
    ) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Superseded is engine-internal: it is written only when a follow-up \
                          replaces the question. Use Canceled to dismiss it, or send the \
                          follow-up itself."
            })),
        ));
    }
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
    // Idempotent: saving an already-saved thread is a 200 no-op, not a 409.
    // A duplicate / racing `/threads/save` (e.g. an iOS PWA double-submit)
    // otherwise reaches here after the first request flipped is_saved=TRUE —
    // `available_thread_actions` then offers only Unsave, so the stale Save
    // 409'd, and the client's error handler reverted the (correct) optimistic
    // pin + toasted a spurious "Thread cannot be saved" error.
    match thread_is_saved(&state, thread_uuid).await? {
        None => return Err((StatusCode::NOT_FOUND, "Thread not found".to_string())),
        Some(true) => return Ok(StatusCode::OK),
        Some(false) => {}
    }
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
    // Idempotent mirror of save: unsaving an already-unsaved thread is a 200
    // no-op, not a 409.
    match thread_is_saved(&state, thread_uuid).await? {
        None => return Err((StatusCode::NOT_FOUND, "Thread not found".to_string())),
        Some(false) => return Ok(StatusCode::OK),
        Some(true) => {}
    }
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

/// POST /api/v1/threads/:thread_id/event-waits/:wait_id/cancel, the **Stop
/// waiting** button on one subscription in the indicator.
///
/// Cancels that wait and nothing else: a thread may hold several, and stopping
/// one must not take the others with it (thread-level Stop is
/// `/api/v1/chat/cancel`, which cancels them all). Emits
/// `EventWaitCanceled { UserStop }` stamped with the device that pressed it,
/// plus the closing `ToolResult` when the wait was still holding the turn
/// parked.
///
/// The two failures report differently on purpose. A wait that already resolved
/// is a stale button and 404s. A wait whose cancel could not be *written* is
/// still live and still cancellable, so it 500s: telling the user it had
/// already resolved would send them away from a button that is about to start
/// working again.
pub(in crate::api) async fn cancel_thread_event_wait(
    State(state): State<AppState>,
    Path((thread_id, wait_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    // `wait_id` alone would find the wait; `thread_id` scopes it, so an id from
    // one thread's UI can never cancel another thread's subscription.
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {e}")))?;
    let wait_uuid = Uuid::parse_str(&wait_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid wait_id: {e}")))?;
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;

    use crate::engine::event_wait::CancelWaitOutcome;
    match state
        .engine
        .cancel_event_wait(
            thread_uuid,
            wait_uuid,
            crate::engine::thread_events::EventWaitCancelCause::UserStop,
            actor,
        )
        .await
    {
        CancelWaitOutcome::Canceled => Ok(StatusCode::OK),
        CancelWaitOutcome::NotLive => Err((
            StatusCode::NOT_FOUND,
            "No live wait with that id on this thread. It may have already \
             delivered, timed out, or been canceled."
                .to_string(),
        )),
        CancelWaitOutcome::EmitFailed => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not record the cancel. The wait is still live, so try again.".to_string(),
        )),
    }
}

/// Refuse an AGENT reaching an event-wait route for a thread that is not its
/// own.
///
/// The three agent-facing event-wait routes are the HTTP form of three tools
/// that take no thread argument at all: `await_event`, `list_event_waits` and
/// `cancel_event_wait` each act on the calling thread and have no way to name
/// another. The CLI keeps that shape by reading `$LUCIDOS_THREAD_ID` rather
/// than taking a flag. But a path segment is a path segment, so a subprocess
/// could substitute another thread's id and get back the capability the
/// argument-less shape removed. This is the check that stops it.
///
/// **Scoped to callers that HAVE a thread**, which is the whole realistic set:
/// a Lucidos-spawned subprocess carries a thread-bound origin token it cannot
/// re-point (`api::actor::subprocess_origin`, HMAC over the thread id), and
/// that is exactly what an agent session is. A caller presenting no token is
/// not an agent claiming to be another thread; it is the ordinary local API
/// surface, which every other `/threads/:id/...` route treats the same way and
/// whose trust boundary is not this function's to move. A subprocess with a
/// token but no thread (a scheduled script) has no subscriptions of its own, so
/// it has nothing to be scoped to and is refused rather than granted the run of
/// every thread.
///
/// Deliberately NOT applied to `.../event-waits/:wait_id/cancel`: that is the
/// **Stop waiting** button, a person acting through the UI, which carries no
/// token by construction.
pub(super) fn refuse_event_waits_for_another_thread(
    headers: &HeaderMap,
    thread_id: Uuid,
) -> Result<(), (StatusCode, String)> {
    use crate::api::actor::SubprocessOrigin;
    match crate::api::actor::subprocess_origin(headers) {
        SubprocessOrigin::Subprocess { source_thread_id }
            if source_thread_id != Some(thread_id) =>
        {
            Err((
                StatusCode::FORBIDDEN,
                "A thread's event subscriptions are its own. This route acts on the \
                 calling thread, and the id in the path is not it. Drop the id: \
                 `lucidos event-waits list` / `cancel` and `lucidos await-event` \
                 already act on the thread you are running in."
                    .to_string(),
            ))
        }
        _ => Ok(()),
    }
}

/// POST /api/v1/threads/:thread_id/event-waits, the way a **coding agent**
/// subscribes to an event.
///
/// The chat agent reaches the same registration through the `await_event` LLM
/// tool, in process. A coding agent cannot: the engine does not own a Claude
/// Code or Codex session's tool set, so its route in is the `lucidos
/// await-event` CLI subcommand over this endpoint.
///
/// This is only possible because a subscription no longer holds a turn. The
/// attached shape needed a dangling `tool_use` in a message array the engine
/// controls, which is exactly what it does not have for a coding-agent session
/// (S11 of `docs/plans/2026-08-05-a-thread-parks-on-an-event-wait.md` excluded
/// them for that reason). Registration now returns immediately and the delivery
/// arrives as an ordinary follow-up message, which the coding-agent lane
/// already knows how to deliver.
///
/// The body is the same `{on, timeout_secs, reason}` the tool takes, so the
/// caps, the subscribability gate and the duplicate refusal are one
/// implementation rather than two. `tool_use_id` is synthesized here: there is
/// no tool call to pair with, and the field is only an id the wait carries.
pub(in crate::api) async fn register_thread_event_wait(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(args): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {e}")))?;
    refuse_event_waits_for_another_thread(&headers, thread_uuid)?;

    // The thread has to exist, and this is the one caller that can get it
    // wrong: the LLM tool takes its thread id from `execute_tool` and cannot
    // name another, while a CLI caller passes `$LUCIDOS_THREAD_ID` and could be
    // running outside a session or against a stale id. Arming a wait for a
    // thread with no row is not harmless: it survives restarts, and its
    // eventual delivery drives a turn on a thread the engine knows nothing
    // about. A read failure answers "exists", because refusing a legitimate
    // subscription on a database hiccup is the worse of the two.
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1)")
            .bind(thread_uuid)
            .fetch_one(&state.pool)
            .await
            .unwrap_or(true);
    if !known {
        return Err((
            StatusCode::NOT_FOUND,
            format!("No thread {thread_id} in this workspace."),
        ));
    }

    // A refusal is the caller's own fault (a bad `on` entry, a cap it has hit),
    // so it is a 400 carrying the same text the chat agent would read, not a
    // 500. The two agents get the identical wording by construction.
    let synthetic_tool_use_id = format!("cli-{}", Uuid::new_v4());
    match state
        .engine
        .register_event_wait(thread_uuid, &synthetic_tool_use_id, &args)
        .await
    {
        crate::engine::event_wait::AwaitEventOutcome::Registered(message) => Ok(Json(
            serde_json::json!({ "status": "subscribed", "message": message }),
        )),
        crate::engine::event_wait::AwaitEventOutcome::Refused(message) => {
            Err((StatusCode::BAD_REQUEST, message))
        }
    }
}

/// GET /api/v1/threads/:thread_id/event-waits, the read half of a **coding
/// agent**'s subscription surface (`lucidos event-waits list`).
///
/// The chat agent reaches the same set through the `list_event_waits` LLM tool,
/// in process. Both read the dispatcher's live cache rather than the event
/// store, so neither can disagree with what will actually wake the thread.
///
/// Scoped to the calling thread by
/// [`refuse_event_waits_for_another_thread`], which is what makes this route
/// keep the promise the tool keeps structurally by taking no thread argument at
/// all.
pub(in crate::api) async fn list_thread_event_waits(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {e}")))?;
    refuse_event_waits_for_another_thread(&headers, thread_uuid)?;
    let waits = state.engine.list_event_waits_for_thread(thread_uuid).await;
    Ok(Json(serde_json::json!({
        "count": waits.len(),
        "event_waits": waits,
    })))
}

/// POST /api/v1/threads/:thread_id/event-waits/cancel, an **agent standing its
/// own subscriptions down** (`lucidos event-waits cancel`).
///
/// Deliberately a separate route from the per-wait
/// `.../event-waits/:wait_id/cancel`, which is the UI's **Stop waiting** button
/// and stamps `UserStop`. The two differ in the cause they record and in what
/// they can address: this one also takes `all`, and it never claims a person
/// pressed anything. Folding them into one route would mean a body field
/// deciding whose action the event log attributes it to, which is exactly the
/// thing an actor must not be.
///
/// Body: `{"wait_id": "..."}`, `{"on": "EventType"}` or `{"all": true}`,
/// exactly one.
pub(in crate::api) async fn cancel_thread_event_waits_for_agent(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(args): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = Uuid::parse_str(&thread_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid thread_id: {e}")))?;
    refuse_event_waits_for_another_thread(&headers, thread_uuid)?;
    let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
    // Trimmed and emptied by `resolve_cancel_target`, which is the one place
    // that decides what counts as "an argument was passed" for all three.
    let on = args.get("on").and_then(|v| v.as_str());
    let wait_id = match args
        .get("wait_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => Some(Uuid::parse_str(raw).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Invalid wait_id: {e}. Run `lucidos event-waits list` for the ids."),
            )
        })?),
        None => None,
    };

    // A refusal is the caller's own fault (both arguments, neither, or an id
    // that is not live here), so it is a 400 carrying the same words the chat
    // agent would read. The two agents get identical wording by construction.
    match state
        .engine
        .cancel_event_waits_for_agent(thread_uuid, wait_id, on, all)
        .await
    {
        crate::engine::event_wait::CancelEventWaitOutcome::Stopped(message) => Ok(Json(
            serde_json::json!({ "status": "stopped", "message": message }),
        )),
        crate::engine::event_wait::CancelEventWaitOutcome::Refused(message) => {
            Err((StatusCode::BAD_REQUEST, message))
        }
    }
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
