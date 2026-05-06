//! HTTP endpoints for the compose state machine.
//! See `docs/plans/2026-05-03-threads-as-drafts-design.md`.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::api::actor::user_actor_resolved;
use crate::api::AppState;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::thread_state::ThreadState;

/// Cap on `compose_text` size. Compose updates fan out to every connected
/// SSE subscriber on every keystroke; without a cap a pathological paste
/// (or runaway client) multiplies the bandwidth cost by N devices.
const MAX_COMPOSE_TEXT_BYTES: usize = 64 * 1024;
/// Cap on the JSON-encoded `compose_images` array, sized to allow ~32 modest
/// image refs without enabling N×64KB blowups. Each image is a URL/path
/// reference, not the binary blob.
const MAX_COMPOSE_IMAGES: usize = 32;

#[derive(Debug, Deserialize)]
pub(super) struct PostThreadBody {
    pub id: Uuid,
    pub mode: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct PutComposeBody {
    pub text: String,
    #[serde(default)]
    pub images: Vec<JsonValue>,
    /// `Some` only when the user is also toggling mode — rejected with 409 if
    /// the thread is no longer in `composing`. Absent on text-only updates.
    #[serde(default)]
    pub mode: Option<String>,
}

fn internal_err<E: std::fmt::Display>(e: E) -> (StatusCode, Json<JsonValue>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": e.to_string() })),
    )
}

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<JsonValue>) {
    (code, Json(serde_json::json!({ "error": msg })))
}

fn validate_mode(mode: &str) -> Result<(), (StatusCode, Json<JsonValue>)> {
    match mode {
        "lucidos" | "claude_code" => Ok(()),
        _ => Err(err(StatusCode::BAD_REQUEST, "mode must be lucidos|claude_code")),
    }
}

/// Map a thread row's `state` (Option = no row) to the HTTP error for an
/// attempted compose write, returning `Some(_)` when the request must be
/// rejected. `Composing`, `Active`, and `Archived` accept compose updates —
/// the latter so a re-opened archived thread can sync the keystrokes that
/// lead up to the send that revives it (see `ThreadState` doc). `Discarded`
/// is the only hard reject. Used by the post-UPDATE branch where `state` is
/// read back from a `RETURNING` clause to distinguish "no row" (404) from
/// "row but wrong state" (410).
///
/// Listed exhaustively so a new `ThreadState` variant forces the author to
/// make a deliberate accept/reject decision instead of inheriting whatever
/// a catch-all happened to do.
fn compose_error(state: Option<ThreadState>) -> Option<(StatusCode, Json<JsonValue>)> {
    let Some(state) = state else {
        return Some(err(StatusCode::NOT_FOUND, "thread not found"));
    };
    match state {
        ThreadState::Composing | ThreadState::Active | ThreadState::Archived => None,
        ThreadState::Discarded => Some(err(StatusCode::GONE, "thread discarded")),
    }
}

/// POST /api/v1/threads — create a thread in `composing` state.
///
/// Idempotent on `id`: re-POSTing the same `{id, mode}` returns 200; a
/// different `mode` returns 409 (the user's first device wins, drift would
/// reopen the bug class this redesign closed).
pub(super) async fn post_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PostThreadBody>,
) -> Result<StatusCode, (StatusCode, Json<JsonValue>)> {
    validate_mode(&body.mode)?;

    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT state, compose_mode FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(body.id)
    .fetch_optional(state.engine.pool())
    .await
    .map_err(internal_err)?;

    if let Some((state_str, existing_mode)) = row {
        let existing_state = ThreadState::from_db_str(&state_str).map_err(internal_err)?;
        return match existing_state {
            ThreadState::Composing => {
                if existing_mode.as_deref() == Some(body.mode.as_str()) {
                    Ok(StatusCode::OK)
                } else {
                    Err(err(
                        StatusCode::CONFLICT,
                        "thread already exists with a different mode",
                    ))
                }
            }
            ThreadState::Active => Err(err(StatusCode::CONFLICT, "thread already active")),
            ThreadState::Discarded => Err(err(StatusCode::GONE, "thread discarded")),
            ThreadState::Archived => Err(err(StatusCode::CONFLICT, "thread archived")),
        };
    }

    let actor = user_actor_resolved(&headers, state.engine.pool(), None).await;
    let event = BusEvent::Thread {
        thread_id: body.id,
        event: ThreadEvent::ThreadStarted {
            mode: body.mode,
            actor: actor.clone(),
        },
        meta: EventMeta {
            actor,
            ..Default::default()
        },
    };
    state
        .engine
        .event_bus
        .emit(event)
        .await
        .map_err(internal_err)?;
    Ok(StatusCode::CREATED)
}

/// PUT /api/v1/threads/:id/compose — update compose fields.
///
/// One round-trip: `UPDATE ... RETURNING state, compose_mode` folds the
/// state-machine guard, the mutation, and the SSE-payload read-back into a
/// single query. Result: hot-path keystroke cost is one DB query plus one SSE
/// broadcast. No event row written (per design — keystroke history isn't
/// audit-worthy).
///
/// `mode` is COALESCE'd so text-only PUTs preserve the user's existing mode
/// preference; explicit mode-change PUTs are rejected once the thread leaves
/// `composing` (mode locks at first send).
pub(super) async fn put_compose(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<PutComposeBody>,
) -> Result<StatusCode, (StatusCode, Json<JsonValue>)> {
    if body.text.len() > MAX_COMPOSE_TEXT_BYTES {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "compose_text exceeds 64 KiB cap",
        ));
    }
    if body.images.len() > MAX_COMPOSE_IMAGES {
        return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "too many compose images"));
    }
    if let Some(ref m) = body.mode {
        validate_mode(m)?;
    }

    let images_json = serde_json::to_value(&body.images).unwrap_or_else(|_| serde_json::json!([]));

    // Mode toggle on a thread that's already past `composing` is a contract
    // violation — pre-check so we surface 409 before the UPDATE rejects it
    // for an unrelated reason and we lose the precise error.
    // `source` mirrors `compose_mode` so a draft that auto-archives without
    // being sent still renders with the correct channel pill. Send events
    // later overwrite source from the actual channel of the message. The
    // WHERE clause already gates mode-carrying writes to `state='composing'`.
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "UPDATE thread_summaries
            SET compose_text = $2,
                compose_images = $3,
                compose_mode = COALESCE($4, compose_mode),
                source = CASE $4::text
                    WHEN 'claude_code' THEN 'claude_code'
                    WHEN 'lucidos'     THEN 'chat'
                    ELSE source
                END
          WHERE thread_id = $1
            AND state IN ('composing', 'active', 'archived')
            AND ($4::text IS NULL OR state = 'composing')
         RETURNING state, compose_mode",
    )
    .bind(id)
    .bind(&body.text)
    .bind(&images_json)
    .bind(body.mode.as_deref())
    .fetch_optional(state.engine.pool())
    .await
    .map_err(internal_err)?;

    let (_state_str, resolved_mode) = match row {
        Some(r) => r,
        None => {
            // Cold path: UPDATE matched zero rows. Distinguish "no row" from
            // "wrong state" / "mode-locked" with a follow-up read so the e2e
            // contract (404/410/409) stays specific.
            let lookup: Option<(String,)> = sqlx::query_as(
                "SELECT state FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(id)
            .fetch_optional(state.engine.pool())
            .await
            .map_err(internal_err)?;
            let st = lookup
                .map(|(s,)| ThreadState::from_db_str(&s))
                .transpose()
                .map_err(internal_err)?;
            // Mode locks at first send — both `Active` and `Archived` are
            // post-send states. Without this branch the cold-path would fall
            // through to `compose_error` which (correctly) accepts archived
            // for compose, masking the mode lock as a silent no-op.
            if body.mode.is_some()
                && matches!(st, Some(ThreadState::Active | ThreadState::Archived))
            {
                return Err(err(
                    StatusCode::CONFLICT,
                    "mode is locked once the thread has been sent",
                ));
            }
            // TOCTOU: a concurrent send between the UPDATE and the lookup
            // may have flipped state to active and the row now matches
            // `compose_can_compose`. Treat as a benign no-op — the
            // concurrent path already wrote a more authoritative value.
            return match compose_error(st) {
                Some(e) => Err(e),
                None => Ok(StatusCode::NO_CONTENT),
            };
        }
    };

    let device_id = headers
        .get(crate::api::actor::HEADER_DEVICE_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let event = BusEvent::System(SystemEvent::ThreadComposeChanged {
        id,
        text: body.text,
        images: images_json,
        mode: resolved_mode,
        origin_device_id: device_id,
    });
    state
        .engine
        .event_bus
        .emit(event)
        .await
        .map_err(internal_err)?;

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/threads/:id — discard a composing thread.
///
/// Idempotent on missing/already-discarded ids (204). Active threads must
/// use archive instead (409).
pub(super) async fn delete_thread(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, Json<JsonValue>)> {
    let lookup: Option<(String,)> =
        sqlx::query_as("SELECT state FROM thread_summaries WHERE thread_id = $1")
            .bind(id)
            .fetch_optional(state.engine.pool())
            .await
            .map_err(internal_err)?;
    let current_state = lookup
        .map(|(s,)| ThreadState::from_db_str(&s))
        .transpose()
        .map_err(internal_err)?;
    match current_state {
        None | Some(ThreadState::Discarded) => return Ok(StatusCode::NO_CONTENT),
        Some(ThreadState::Composing) => {}
        Some(ThreadState::Active) => {
            return Err(err(
                StatusCode::CONFLICT,
                "thread is active — use archive instead",
            ));
        }
        Some(ThreadState::Archived) => {
            return Err(err(StatusCode::CONFLICT, "thread already archived"));
        }
    }

    let actor = user_actor_resolved(&headers, state.engine.pool(), None).await;
    let event = BusEvent::Thread {
        thread_id: id,
        event: ThreadEvent::ThreadDiscarded {
            actor: actor.clone(),
        },
        meta: EventMeta {
            actor,
            ..Default::default()
        },
    };
    state
        .engine
        .event_bus
        .emit(event)
        .await
        .map_err(internal_err)?;
    Ok(StatusCode::NO_CONTENT)
}
