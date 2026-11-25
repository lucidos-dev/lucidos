//! Thread mutation handlers — answer-question, save/unsave, move to folder, rename, suggest
//! title, continue — and the message-history fetch.

use crate::engine::agent_recovery::orphaned_branch_work::OrphanedBranchWorkExt;
use crate::engine::chat::held_messages::HeldMessageDeliveryExt;
use crate::engine::chat::rerun::RerunExt;
use crate::engine::chat::title::TitleExt;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::thread_reach::refuse_without_authority;
use crate::api::AppState;
use crate::engine::continuation_reason::USER_CLICKED_CONTINUE_REASON;
use crate::engine::permission::Action;

use super::{extract_thread_uuid, parse_thread_id};

/// Whether the thread is saved, and its current `summary_version`, or `None`
/// for a thread that does not exist.
async fn thread_is_saved(
    state: &AppState,
    thread_id: Uuid,
) -> Result<Option<(bool, i64)>, (StatusCode, String)> {
    sqlx::query_as("SELECT is_saved, summary_version FROM thread_summaries WHERE thread_id = $1")
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
    /// The client's id for this answer, recorded as the answer event's id.
    #[serde(default)]
    event_id: Option<Uuid>,
    /// The sending page's *input chain* and this answer's *input sequence*,
    /// as on `POST /api/v1/chat/stream` (ADR 0449). Both or neither.
    #[serde(default)]
    input_chain: Option<String>,
    #[serde(default)]
    input_seq: Option<i64>,
    #[serde(default)]
    compose_epoch: Option<i64>,
}

/// POST /api/v1/threads/:thread_id/answer-question — answer a pending question
/// on `thread_id`. Used by both the CC `AskUserQuestion` hook and the chat
/// agent's in-process `ask_user_question` tool; `answer_pending_question`
/// branches on the originating event's channel so chat threads skip the
/// CC-specific resume side-effects (`CodingAgentPromptSent` marker +
/// `ContinueSignal` spawn).
///
/// Returns 409 when the question is missing or already answered, or when a
/// later input from the same page overtook this answer. Returns 400 for a
/// `Superseded` body (see below).
///
/// A positioned answer is admitted to the thread's input order, then recorded
/// at once. A running turn may be blocked on this very question, so waiting
/// behind an input admitted earlier could deadlock that turn. Admission and
/// recording run as one detached task the handler awaits, so a dropped request
/// cannot split them.
pub(in crate::api) async fn answer_thread_question(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AnswerThreadQuestionBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let thread_uuid =
        Uuid::parse_str(&thread_id).map_err(|_| ApiError::bad_request("Invalid thread_id"))?;
    if matches!(
        body.answer,
        crate::engine::thread_events::AnswerKind::Superseded
    ) {
        return Err(ApiError::bad_request(
            "Superseded is engine-internal: it is written only when a follow-up \
             replaces the question. Use Canceled to dismiss it, or send the \
             follow-up itself.",
        ));
    }
    // An answer is the user's own, so it is the owner's button wherever it
    // lands outside the caller's subtree (ADR 0168 clause 4). Before the answer
    // is recorded, so a refusal leaves the card live and answerable.
    refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(thread_uuid),
        Action::AnswerQuestion,
    )
    .await?;
    let actor = crate::api::permission_requests::answering_actor(
        &state.pool,
        &state.engine.event_bus,
        &headers,
        thread_uuid,
        &body.tool_use_id,
    )
    .await?;
    use crate::engine::agent_question::{answer_pending_question_placed, AnswerResult};
    use crate::engine::InputPosition;
    let position =
        InputPosition::from_wire(body.input_chain.as_deref(), body.input_seq, body.event_id)
            .map_err(ApiError::bad_request)?;
    let thread_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_summaries WHERE thread_id = $1)")
            .bind(thread_uuid)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::db)?;
    let position = position.filter(|_| thread_exists);
    let engine = state.engine.clone();
    let answering = tokio::spawn(async move {
        match answer_pending_question_placed(
            &engine,
            thread_uuid,
            body.tool_use_id.clone(),
            body.answer.clone(),
            actor,
            body.event_id,
            Some((position, body.compose_epoch)),
            crate::engine::permission::PickedBy::Tap,
        )
        .await
        {
            AnswerResult::Resumed => {}
            AnswerResult::Conflict(msg) => {
                let mut error = ApiError::new(StatusCode::CONFLICT, msg.clone());
                for reason in [
                    "compose_snapshot_stale",
                    "compose_fence_required",
                    "overtaken",
                ] {
                    if msg.contains(reason) {
                        let message = match reason {
                            "compose_snapshot_stale" => "Another send was accepted before this answer. Retry to send the answer again.".to_string(),
                            "compose_fence_required" => "This answer needs a current draft version. Retry it to send it again.".to_string(),
                            _ => crate::engine::overtaken_message(thread_uuid),
                        };
                        error = ApiError::new(StatusCode::CONFLICT, message).with_reason(reason);
                        break;
                    }
                }
                return Err(error);
            }
        }
        let compose: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT payload->'compose' FROM events WHERE thread_id=$1 AND event_type='UserQuestionAnswered' AND payload->>'tool_use_id'=$2",
        ).bind(thread_uuid).bind(&body.tool_use_id).fetch_one(&state.pool).await.map_err(ApiError::db)?;
        Ok(Json(serde_json::json!({ "ok": true, "compose": compose })))
    });
    answering
        .await
        .map_err(|e| ApiError::internal(format!("The answer could not be dispatched: {e}")))?
}

/// The body a pin or unpin answers with: the row's `summary_version` once the
/// write landed. The client holds its optimistic row until it sees that
/// version (ADR 0437).
fn summary_version_body(version: i64) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "summary_version": version }))
}

/// The version the emit's own aggregate carried. A thread event always has
/// one, so a missing version is a projection bug, reported as such.
fn emitted_version(
    emitted: Option<crate::engine::event_bus::EmitResult>,
) -> Result<i64, (StatusCode, String)> {
    emitted.and_then(|r| r.summary_version).ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "The write landed but reported no summary version".to_string(),
        )
    })
}

/// The custom folder a pin or move names, checked against the
/// `thread_folders` list. A missing or null `folder` is plain Pinned.
async fn requested_folder(
    state: &AppState,
    request: &serde_json::Value,
) -> Result<Option<String>, (StatusCode, String)> {
    let folder = match request.get("folder") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::String(id)) => id.clone(),
        Some(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                "folder must be a folder id or null".to_string(),
            ))
        }
    };
    let known = crate::core::thread_folders::list(&state.pool)
        .await
        .iter()
        .any(|f| f.id == folder);
    if !known {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("No custom folder has the id '{folder}'"),
        ));
    }
    Ok(Some(folder))
}

async fn emit_pin_event(
    state: &AppState,
    thread_id: Uuid,
    event: crate::engine::thread_events::ThreadEvent,
    actor: Option<crate::engine::thread_events::MessageOrigin>,
) -> Result<i64, (StatusCode, String)> {
    let name = event.event_type();
    let emitted = state
        .engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event,
            meta: crate::engine::thread_events::EventMeta::with_actor(actor),
        })
        .await
        .map_err(|e| {
            log_legacy!("[API] Failed to emit {}: {}", name, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to emit {name}: {e}"),
            )
        })?;
    emitted_version(emitted)
}

/// POST /api/v1/threads/save — pin a thread, into the custom folder `folder`
/// names when the body carries one. Pinning a pinned thread with a folder
/// moves it there; without one it changes nothing.
pub(in crate::api) async fn save_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = extract_thread_uuid(&request)?;
    let folder = requested_folder(&state, &request).await?;
    let already_saved = match thread_is_saved(&state, thread_uuid).await? {
        None => return Err((StatusCode::NOT_FOUND, "Thread not found".to_string())),
        Some((true, version)) if folder.is_none() => return Ok(summary_version_body(version)),
        Some((saved, _)) => saved,
    };
    let actor = crate::api::actor::user_actor(&headers, None);
    let mut version = None;
    if !already_saved {
        // The home thread sits in no drawer section, so a pin would move it
        // nowhere (ADR 0362).
        let is_home = crate::engine::home_thread::is_home_thread(&state.pool, thread_uuid)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if is_home {
            return Err((
                StatusCode::CONFLICT,
                "The home thread opens from Home rather than the thread list, so it is never pinned."
                    .to_string(),
            ));
        }
        version = Some(
            emit_pin_event(
                &state,
                thread_uuid,
                crate::engine::thread_events::ThreadEvent::ThreadSaved,
                actor.clone(),
            )
            .await?,
        );

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
    }
    if folder.is_some() {
        version = Some(
            emit_pin_event(
                &state,
                thread_uuid,
                crate::engine::thread_events::ThreadEvent::ThreadMovedToFolder { folder },
                actor,
            )
            .await?,
        );
    }
    // The early return above leaves a pin, a move, or both to emit.
    version.map(summary_version_body).ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Neither a pin nor a move was written".to_string(),
        )
    })
}

/// POST /api/v1/threads/move-to-folder — file a pinned thread in the custom
/// folder `folder` names, or back in plain Pinned when it is null. Refuses an
/// unpinned thread with 409, since a folder only files a pin.
pub(in crate::api) async fn move_thread_to_folder(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = extract_thread_uuid(&request)?;
    let folder = requested_folder(&state, &request).await?;
    match thread_is_saved(&state, thread_uuid).await? {
        None => return Err((StatusCode::NOT_FOUND, "Thread not found".to_string())),
        Some((false, _)) => {
            return Err((
                StatusCode::CONFLICT,
                "Only a pinned thread sits in a folder. Pin it with a folder instead.".to_string(),
            ))
        }
        Some((true, _)) => {}
    }
    let actor = crate::api::actor::user_actor(&headers, None);
    let version = emit_pin_event(
        &state,
        thread_uuid,
        crate::engine::thread_events::ThreadEvent::ThreadMovedToFolder { folder },
        actor,
    )
    .await?;
    Ok(summary_version_body(version))
}

/// POST /api/v1/threads/unsave — unsave a thread
pub(in crate::api) async fn unsave_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_uuid = extract_thread_uuid(&request)?;
    // Idempotent mirror of save: unsaving an already-unsaved thread is a 200
    // no-op, not a 409.
    match thread_is_saved(&state, thread_uuid).await? {
        None => return Err((StatusCode::NOT_FOUND, "Thread not found".to_string())),
        Some((false, version)) => return Ok(summary_version_body(version)),
        Some((true, _)) => {}
    }
    let actor = crate::api::actor::user_actor(&headers, None);
    let version = emit_pin_event(
        &state,
        thread_uuid,
        crate::engine::thread_events::ThreadEvent::ThreadUnsaved,
        actor,
    )
    .await?;
    Ok(summary_version_body(version))
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
    let actor = crate::api::actor::user_actor(&headers, None);

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
            log_legacy!("[API] Failed to rename thread: {}", e);
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
    let thread_uuid = uuid::Uuid::parse_str(thread_id).ok();

    // Refused before the model call, so a stale menu spends nothing.
    if let Some(id) = thread_uuid {
        let home = crate::engine::home_thread::is_home_thread(&state.pool, id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if home {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "Thread {id} is the home thread. Only you name it, so Lucidos suggests \
                     no name for it. Rename it by hand instead."
                ),
            ));
        }
    }

    // Get recent messages for context (last few messages give better titles than just the first)
    let messages = state
        .engine
        .event_store()
        .get_thread_messages(thread_id)
        .await
        .map_err(|e| {
            log_legacy!(
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

    let call = crate::engine::chat::title_call(&state.engine).await;
    // A suggestion costs the same tokens an automatic title does, so it is
    // captured the same way. An unparseable id records on the home thread,
    // rather than refusing the call over bookkeeping.
    let capture = crate::engine::AuxCapture::for_thread_or_home(
        &state.engine.event_bus,
        thread_uuid,
        crate::engine::ContextPurpose::Title,
    );
    let title = crate::engine::chat::generate_thread_title(&call, &summary, None, &capture)
        .await
        .map_err(|e| {
            log_legacy!("[API] Failed to generate title suggestion: {}", e);
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
/// emits `ContinuationStarted` + `PromptInjected` (engine note
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
    let thread_uuid = parse_thread_id(&thread_id)?;
    // Restarting another thread's turn is nobody's answer to anything, which is
    // why ADR 0168 puts it on clause 4. Before the emit, so a refusal starts no
    // turn.
    refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(thread_uuid),
        Action::Continue,
    )
    .await
    .map_err(|e| (e.status_code(), e.to_string()))?;
    let actor = crate::api::actor::user_actor(&headers, None);

    // Decide which dispatch path to take based on the thread's recorded type.
    let is_coding_agent: bool =
        sqlx::query_scalar("SELECT is_coding_agent FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_uuid)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                log_legacy!("[API] Failed to read thread type for continue: {}", e);
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
                log_legacy!("[API] Failed to emit ContinuationRequested: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to emit ContinuationRequested: {}", e),
                )
            })?;
        // Continue is the user's reply too, so held agent messages follow it
        // into the resumed session (ADR 0256).
        state.engine.spawn_held_message_release(thread_uuid);
        return Ok(StatusCode::OK);
    }

    // Chat / trigger thread — route through chat::rerun.
    state
        .engine
        .continue_chat(thread_uuid, actor, Default::default())
        .await
        .map_err(|e| {
            log_legacy!(
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

/// POST /api/v1/threads/:thread_id/harden, the Not ready strip's **Harden**.
///
/// Starts the hardening run for a resting coding-agent thread whose turn end
/// withheld its work as `hardening_missing`. Its turn end then proposes the
/// change through the normal floor. Any other thread is refused with a 409,
/// so the button can never start a turn the strip did not offer.
pub(in crate::api) async fn harden_thread(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    let thread_uuid = parse_thread_id(&thread_id)?;
    // Starting a turn on another thread is the same act as Continue.
    refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(thread_uuid),
        Action::Continue,
    )
    .await
    .map_err(|e| (e.status_code(), e.to_string()))?;
    let actor = crate::api::actor::user_actor(&headers, None);
    if !state.engine.work_awaits_hardening(thread_uuid).await {
        return Err((
            StatusCode::CONFLICT,
            "Nothing on this thread waits on hardening.".to_string(),
        ));
    }
    state
        .engine
        .harden_held_work(thread_uuid, actor)
        .await
        .map_err(|e| {
            log_legacy!("[API] harden failed for thread {}: {}", thread_uuid, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to start hardening: {}", e),
            )
        })?;
    // Harden is the user's reply too, so held agent messages follow it into
    // the resumed session, as they follow Continue (ADR 0256).
    state.engine.spawn_held_message_release(thread_uuid);
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
    let thread_uuid = parse_thread_id(&thread_id)?;
    let wait_uuid = Uuid::parse_str(&wait_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid wait_id: {e}")))?;
    // The path names the thread, and the caller picks the path. So an agent
    // presenting its token is held to its own reach, as on every cancel route.
    refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(thread_uuid),
        Action::Cancel,
    )
    .await
    .map_err(|e| (e.status_code(), e.to_string()))?;
    let actor = crate::api::actor::user_actor(&headers, None);

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

pub(super) fn refuse_event_waits_for_another_thread(
    headers: &HeaderMap,
    thread_id: Uuid,
) -> Result<(), (StatusCode, String)> {
    refuse_an_agent_from_another_thread(
        headers,
        thread_id,
        "A thread's event subscriptions are its own. This route acts on the \
         calling thread, and the id in the path is not it. Drop the id: \
         `lucidos event-waits list` / `cancel` and `lucidos await-event` \
         already act on the thread you are running in.",
    )
}

/// Refuse a subprocess whose origin token names a thread other than
/// `thread_id`, with `refusal` as the 403 body. The check behind every route
/// that acts on the calling thread only. See
/// [`refuse_event_waits_for_another_thread`] for who is in scope and why.
pub(super) fn refuse_an_agent_from_another_thread(
    headers: &HeaderMap,
    thread_id: Uuid,
    refusal: &str,
) -> Result<(), (StatusCode, String)> {
    use crate::core::subprocess_origin::SubprocessOrigin;
    match crate::core::subprocess_origin::subprocess_origin(headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } if source_thread_id != Some(thread_id) => {
            Err((StatusCode::FORBIDDEN, refusal.to_string()))
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
    let thread_uuid = parse_thread_id(&thread_id)?;
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
    let thread_uuid = parse_thread_id(&thread_id)?;
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
    let thread_uuid = parse_thread_id(&thread_id)?;
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
            log_legacy!("[API] Failed to get thread messages: {}", e);
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
            log_legacy!("[API] Failed to get thread timeline events: {}", e);
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

#[cfg(test)]
mod composer_submission_tests {
    use crate::engine::agent_question::composer_submission;
    use crate::engine::thread_events::AnswerKind;

    #[test]
    fn an_option_click_submits_no_composer_content() {
        let answer = AnswerKind::Selected {
            option_id: "opt-a".into(),
        };
        assert_eq!(composer_submission(&answer), None);
    }

    #[test]
    fn typed_text_is_the_submission() {
        let answer = AnswerKind::FreeText {
            text: "night has passed by".into(),
            image_hashes: vec![],
        };
        assert_eq!(
            composer_submission(&answer),
            Some(("night has passed by".into(), vec![]))
        );
    }

    #[test]
    fn a_multi_select_folds_in_the_prompt_text_and_images() {
        let hash = "a".repeat(64);
        let answer = AnswerKind::MultiSelected {
            option_ids: vec!["opt-a".into()],
            text: Some("and check the logs".into()),
            image_hashes: vec![hash.clone()],
        };
        assert_eq!(
            composer_submission(&answer),
            Some(("and check the logs".into(), vec![hash]))
        );
    }

    #[test]
    fn whitespace_alone_submits_nothing() {
        let answer = AnswerKind::FreeText {
            text: "  \n".into(),
            image_hashes: vec![],
        };
        assert_eq!(composer_submission(&answer), None);
    }
}

#[cfg(test)]
mod route_tests {
    use crate::test_support::{boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db};
    use axum::{body::Body, http::Request};
    use serde_json::json;
    use tower::ServiceExt;
    use uuid::Uuid;

    async fn router_for(
        engine: &std::sync::Arc<crate::engine::LucidosEngine>,
        pool: &sqlx::PgPool,
    ) -> axum::Router {
        let scheduler = crate::scheduler::SchedulerManager::new(engine.clone(), pool.clone())
            .await
            .unwrap();
        crate::api::create_router(
            engine.clone(),
            pool.clone(),
            engine.event_store().clone(),
            crate::memory::EmbedderSlot::empty(),
            None,
            engine.workspace_path().to_path_buf(),
            std::sync::Arc::new(tokio::sync::Mutex::new(scheduler)),
            chrono::Utc::now(),
            &crate::net_config::BindChoice::Loopback,
            std::sync::Arc::new(crate::api::hooks::NoEventHooks),
        )
    }

    async fn send(
        app: &axum::Router,
        method: &str,
        uri: &str,
        body: serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("/api/v1{uri}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        request
            .headers_mut()
            .extend(crate::api::gateway_stamp::test_support::stamped_device_headers("device-abc"));
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
        (status, value)
    }

    #[tokio::test]
    async fn the_answer_route_refuses_a_client_superseded_answer_and_a_missing_thread_leaves_no_chain_row(
    ) {
        if !in_engine_child(
            "api::threads::actions::route_tests::the_answer_route_refuses_a_client_superseded_answer_and_a_missing_thread_leaves_no_chain_row",
        )
        .await
        {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let app = router_for(&engine, &pool).await;
        let thread = crate::test_support::seed_coding_thread(&engine.event_bus, None).await;

        let (status, body) = send(
            &app,
            "POST",
            &format!("/threads/{thread}/answer-question"),
            json!({"tool_use_id": "tool-1", "answer": {"kind": "Superseded"}}),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert!(body.to_string().contains("engine-internal"), "{body}");

        let missing = Uuid::new_v4();
        let chain = Uuid::new_v4();
        let (status, body) = send(
            &app,
            "POST",
            &format!("/threads/{missing}/answer-question"),
            json!({
                "tool_use_id": "tool-2",
                "answer": {"kind": "Canceled"},
                "event_id": Uuid::new_v4(),
                "input_chain": chain,
                "input_seq": 1,
            }),
        )
        .await;
        assert_eq!(status, 409, "{body}");
        let rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM thread_input_chains WHERE input_chain = $1")
                .bind(chain)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows, 0,
            "a refused answer must not make its thread addressable"
        );

        drop(app);
        drop(engine);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn saving_a_saved_thread_is_a_200_no_op_not_a_409() {
        if !in_engine_child(
            "api::threads::actions::route_tests::saving_a_saved_thread_is_a_200_no_op_not_a_409",
        )
        .await
        {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let app = router_for(&engine, &pool).await;
        let thread = crate::test_support::seed_coding_thread(&engine.event_bus, None).await;

        for _ in 0..2 {
            let (status, body) =
                send(&app, "POST", "/threads/save", json!({"thread_id": thread})).await;
            assert_eq!(status, 200, "{body}");
        }

        drop(app);
        drop(engine);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_wait_is_canceled_only_through_its_own_threads_path() {
        if !in_engine_child("api::threads::actions::route_tests::a_wait_is_canceled_only_through_its_own_threads_path")
            .await
        {
            return;
        }
        let (pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let app = router_for(&engine, &pool).await;
        let mine = crate::test_support::seed_coding_thread(&engine.event_bus, None).await;
        let theirs = crate::test_support::seed_coding_thread(&engine.event_bus, None).await;

        let outcome = engine
            .register_event_wait(
                mine,
                "tool-1",
                &json!({
                    "on": [{"event_type": "ChangeProposed"}],
                    "timeout_secs": 600,
                    "reason": "the build to finish",
                }),
            )
            .await;
        assert!(
            matches!(
                outcome,
                crate::engine::event_wait::AwaitEventOutcome::Registered(_)
            ),
            "the wait must register for this test to mean anything"
        );
        let wait_id = engine.list_event_waits_for_thread(mine).await[0].wait_id;

        let (status, _) = send(
            &app,
            "POST",
            &format!("/threads/{theirs}/event-waits/{wait_id}/cancel"),
            json!({}),
        )
        .await;
        assert_eq!(status, 404);
        assert_eq!(engine.list_event_waits_for_thread(mine).await.len(), 1);

        let (status, _) = send(
            &app,
            "POST",
            &format!("/threads/{mine}/event-waits/{wait_id}/cancel"),
            json!({}),
        )
        .await;
        assert_eq!(status, 200);
        assert!(engine.list_event_waits_for_thread(mine).await.is_empty());

        drop(app);
        drop(engine);
        pool.close().await;
        teardown_test_db(&db).await;
    }
}
