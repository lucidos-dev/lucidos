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
use crate::api::{ApiError, AppState};
use crate::core::blobs::write_blob_from_base64;
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

/// Legacy compose-image payload (`[{base64, mime_type}, ...]`). Mime is
/// re-sniffed server-side from the bytes so this struct only needs the
/// base64; the user-supplied mime_type is intentionally dropped.
#[derive(Debug, Deserialize)]
pub(super) struct LegacyComposeImage {
    pub base64: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct PutComposeBody {
    pub text: String,
    /// `null` (absent) preserves existing draft images via SQL COALESCE;
    /// `[]` clears; `[hash, …]` replaces.
    #[serde(default)]
    pub image_hashes: Option<Vec<String>>,
    /// Compat shim for legacy frontends still posting inline base64.
    /// Mutually exclusive with `image_hashes`.
    #[serde(default)]
    pub images: Option<Vec<LegacyComposeImage>>,
    /// `Some` only when the user is also toggling mode — rejected with 409 if
    /// the thread is no longer in `composing`. Absent on text-only updates.
    #[serde(default)]
    pub mode: Option<String>,
}

fn validate_mode(mode: &str) -> Result<(), ApiError> {
    match mode {
        "lucidos" | "claude_code" => Ok(()),
        _ => Err(ApiError::bad_request("mode must be lucidos|claude_code")),
    }
}

/// Map a thread row's `state` (Option = no row) to the HTTP error for an
/// attempted compose write, returning `Some(_)` when the request must be
/// rejected. `Composing` and `Active` accept compose updates; `Discarded`
/// is the only hard reject. Archived threads carry `state='active'` plus
/// `archive_state='archived'` and so flow through the `Active` arm — the
/// gmail-like revival behavior (keystrokes lead up to the send that
/// re-surfaces the thread) is preserved without needing a separate
/// `Archived` value on this column. Used by the post-UPDATE branch where
/// `state` is read back from a `RETURNING` clause to distinguish "no row"
/// (404) from "row but wrong state" (410).
///
/// Listed exhaustively so a new `ThreadState` variant forces the author to
/// make a deliberate accept/reject decision instead of inheriting whatever
/// a catch-all happened to do.
fn compose_error(state: Option<ThreadState>) -> Option<ApiError> {
    let Some(state) = state else {
        return Some(ApiError::not_found("thread not found"));
    };
    match state {
        ThreadState::Composing | ThreadState::Active => None,
        ThreadState::Discarded => Some(ApiError::new(StatusCode::GONE, "thread discarded")),
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
) -> Result<StatusCode, ApiError> {
    validate_mode(&body.mode)?;

    let row: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT state, archive_state, compose_mode FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(body.id)
    .fetch_optional(state.engine.pool())
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;

    if let Some((state_str, archive_state_str, existing_mode)) = row {
        let existing_state = ThreadState::from_db_str(&state_str)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        // Each arm fully handles its state — no spatial-ordering dependency
        // between an early-return and a later match. Discarded returns 410
        // unconditionally (the `ThreadDiscarded` projection sets BOTH
        // state='discarded' AND archive_state='archived', and the 410-Gone
        // contract for "this id is dead, mint a new one" must not be masked
        // by the post-collapse archive flag). Active distinguishes the
        // archived sub-case for a more specific 409 message.
        return match existing_state {
            ThreadState::Composing => {
                if existing_mode.as_deref() == Some(body.mode.as_str()) {
                    Ok(StatusCode::OK)
                } else {
                    Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "thread already exists with a different mode",
                    ))
                }
            }
            ThreadState::Active => {
                if archive_state_str == "archived" {
                    Err(ApiError::new(StatusCode::CONFLICT, "thread archived"))
                } else {
                    Err(ApiError::new(StatusCode::CONFLICT, "thread already active"))
                }
            }
            ThreadState::Discarded => Err(ApiError::new(StatusCode::GONE, "thread discarded")),
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
        .map_err(|e| ApiError::internal(e.to_string()))?;
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
) -> Result<StatusCode, ApiError> {
    if body.text.len() > MAX_COMPOSE_TEXT_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "compose_text exceeds 64 KiB cap",
        ));
    }
    if let Some(ref m) = body.mode {
        validate_mode(m)?;
    }

    // None = preserve (SQL COALESCE). `image_hashes` wins over legacy
    // `images`; the latter is uploaded inline before the UPDATE.
    let new_image_hashes: Option<Vec<String>> = if let Some(hashes) = body.image_hashes {
        Some(hashes)
    } else if let Some(legacy) = body.images {
        if legacy.is_empty() {
            Some(Vec::new())
        } else {
            crate::log!(
                "[Compat] legacy image upload via PUT compose ({} images)",
                legacy.len()
            );
            let mut hashes = Vec::with_capacity(legacy.len());
            for img in legacy {
                let blob = write_blob_from_base64(state.engine.workspace_path(), &img.base64)
                    .map_err(|e| {
                        let status = match e {
                            crate::core::blobs::BlobError::BadEncoding(_) => StatusCode::BAD_REQUEST,
                            crate::core::blobs::BlobError::UnsupportedMime => {
                                StatusCode::UNSUPPORTED_MEDIA_TYPE
                            }
                            crate::core::blobs::BlobError::Io(_) => {
                                StatusCode::INTERNAL_SERVER_ERROR
                            }
                        };
                        ApiError::new(status, e.to_string())
                    })?;
                hashes.push(blob.hash);
            }
            Some(hashes)
        }
    } else {
        None
    };

    if let Some(ref h) = new_image_hashes {
        if h.len() > MAX_COMPOSE_IMAGES {
            return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "too many compose images"));
        }
    }

    let images_bind: Option<JsonValue> = new_image_hashes
        .as_ref()
        .map(|h| serde_json::Value::Array(h.iter().cloned().map(JsonValue::String).collect()));

    // Mode toggle on a thread that's already past `composing` is a contract
    // violation — pre-check so we surface 409 before the UPDATE rejects it
    // for an unrelated reason and we lose the precise error.
    // `source` mirrors `compose_mode` so a draft that auto-archives without
    // being sent still renders with the correct channel pill. Send events
    // later overwrite source from the actual channel of the message. The
    // WHERE clause already gates mode-carrying writes to `state='composing'`.
    //
    // `compose_images` uses COALESCE($3, compose_images): NULL bind preserves
    // the existing array, `[]` clears it.
    let row: Option<(String, Option<String>, JsonValue)> = sqlx::query_as(
        "UPDATE thread_summaries
            SET compose_text = $2,
                compose_images = COALESCE($3, compose_images),
                compose_mode = COALESCE($4, compose_mode),
                source = CASE $4::text
                    WHEN 'claude_code' THEN 'claude_code'
                    WHEN 'lucidos'     THEN 'chat'
                    ELSE source
                END
          WHERE thread_id = $1
            AND state IN ('composing', 'active')
            AND ($4::text IS NULL OR state = 'composing')
         RETURNING state, compose_mode, compose_images",
    )
    .bind(id)
    .bind(&body.text)
    .bind(images_bind.as_ref())
    .bind(body.mode.as_deref())
    .fetch_optional(state.engine.pool())
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;

    let (_state_str, resolved_mode, post_compose_images) = match row {
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
            .map_err(|e| ApiError::internal(e.to_string()))?;
            let st = lookup
                .map(|(s,)| ThreadState::from_db_str(&s))
                .transpose()
                .map_err(|e| ApiError::internal(e.to_string()))?;
            // Mode locks at first send — once the thread leaves Composing,
            // mode is fixed. Archived rows carry state='active' so they hit
            // this branch naturally. Without it the cold-path would fall
            // through to `compose_error` which (correctly) accepts compose
            // updates from post-send states, masking the mode lock as a
            // silent no-op.
            if body.mode.is_some() && matches!(st, Some(ThreadState::Active)) {
                return Err(ApiError::new(
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

    // Reads back whatever COALESCE produced — new hashes on a touched
    // write, the existing array on a preserve write.
    let hashes_for_event: Vec<String> = post_compose_images
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let event = BusEvent::System(SystemEvent::ThreadComposeChanged {
        id,
        text: body.text,
        image_hashes: hashes_for_event,
        mode: resolved_mode,
        origin_device_id: device_id,
    });
    state
        .engine
        .event_bus
        .emit(event)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

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
) -> Result<StatusCode, ApiError> {
    let lookup: Option<(String, String)> = sqlx::query_as(
        "SELECT state, archive_state FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(id)
    .fetch_optional(state.engine.pool())
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let Some((state_str, archive_state_str)) = lookup else {
        return Ok(StatusCode::NO_CONTENT);
    };
    let current_state = ThreadState::from_db_str(&state_str)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    // Each arm fully handles its state — no spatial-ordering dependency.
    // Discarded is idempotent (204). Active is rejected; the
    // already-archived sub-case takes the more specific 409 message
    // ("thread already archived") since `archive_state` is the sole
    // archive flag post-collapse and archived rows now carry
    // state='active'.
    match current_state {
        ThreadState::Composing => {} // fall through to emit ThreadDiscarded
        ThreadState::Discarded => return Ok(StatusCode::NO_CONTENT),
        ThreadState::Active => {
            if archive_state_str == "archived" {
                return Err(ApiError::new(StatusCode::CONFLICT, "thread already archived"));
            }
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "thread is active — use archive instead",
            ));
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
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
