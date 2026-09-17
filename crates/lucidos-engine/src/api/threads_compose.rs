//! HTTP endpoints for the compose state machine.
//! See `docs/plans/2026-05-03-threads-as-drafts-design.md`.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
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
/// How many images one compose draft may carry. A COUNT, not a byte budget:
/// each entry is a blob hash of fixed length, checked by `is_blob_hash`, so
/// the two caps together bound the array at 32 x 64 bytes.
const MAX_COMPOSE_IMAGES: usize = 32;
/// Cap on the JSON-encoded `compose_selection` object. A partial
/// `ComposeSelectionOverride` is a handful of short fields (~a few hundred
/// bytes); 8 KiB is generous headroom while still fencing off a runaway client,
/// since this fans out over SSE like the other compose fields.
const MAX_COMPOSE_SELECTION_BYTES: usize = 8 * 1024;

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
    /// `Some` only when the user is toggling the channel, absent on every other
    /// write. A CURRENT one is rejected with 409 once the thread leaves
    /// `composing`. A stale one takes the 412 below instead: it was composed
    /// while the thread was still a draft, so its mode was legal then.
    #[serde(default)]
    pub mode: Option<String>,
    /// Per-draft dropdown selections (target/scope, coding agent, Lucidos
    /// model + reasoning, coding-agent model + reasoning) as a partial
    /// `ComposeSelectionOverride`-shaped object. `None` (absent) preserves the
    /// existing stored selection via SQL COALESCE — a text-only keystroke PUT
    /// must not wipe the draft's picks; a dropdown change sends the full object.
    #[serde(default)]
    pub selection: Option<JsonValue>,
    /// The *compose epoch* (`docs/glossary.md`) this write was composed
    /// against: the newest value the client had heard for the thread when it
    /// read the draft. The UPDATE matches on it, so a write composed BEFORE a
    /// submission is refused when it arrives AFTER one, however long it was
    /// stalled and whatever the client concluded about it.
    ///
    /// `None` (absent) skips the precondition. That is permanent back-compat
    /// for a cached PWA bundle running against a newer engine: refusing an
    /// epoch-less write would break draft sync outright for a client that
    /// cannot know to send one.
    #[serde(default)]
    pub compose_epoch: Option<i64>,
}

/// Accept a compose mode in either spelling, and answer with the STORED one.
///
/// Public API parameter values are kebab-case, so `claude-code` is what a
/// caller reading the rest of this API will send. The snake form is what the
/// column holds (`thread_summaries.compose_mode` and `source`), so it stays the
/// stored spelling and only the boundary aliases.
fn canonical_mode(mode: &str) -> Result<&'static str, ApiError> {
    match mode {
        "lucidos" => Ok("lucidos"),
        "claude_code" | "claude-code" => Ok("claude_code"),
        _ => Err(ApiError::bad_request(
            "mode must be lucidos|claude-code (claude_code is accepted, and is what is stored)",
        )),
    }
}

/// The one refusal both image paths answer with, so the legacy upload and the
/// hash list cannot report the same cap differently.
fn too_many_compose_images() -> ApiError {
    ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "too many compose images")
}

/// Is `hash` the blob address `core::blobs::resolve_blob` accepts: 64 ASCII
/// hex characters?
///
/// A compose image hash goes verbatim into `thread_summaries.compose_images`
/// and back out in every `ThreadComposeChanged` frame. Unvalidated, one entry
/// can be a string of any size, so the count cap alone bounds nothing.
fn is_blob_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())
}

/// Refuse a compose image list the projection must not store: too many
/// entries, or an entry that is not a blob address.
fn reject_compose_images(hashes: &[String]) -> Option<ApiError> {
    if hashes.len() > MAX_COMPOSE_IMAGES {
        return Some(too_many_compose_images());
    }
    let bad = hashes.iter().find(|h| !is_blob_hash(h))?;
    // The rejected value is reported by SIZE, never echoed. Echoing it is how
    // a 90 MB entry becomes a 90 MB error body.
    Some(ApiError::bad_request(format!(
        "compose image hash must be 64 hex characters, got {} bytes",
        bad.len()
    )))
}

/// Map a thread row's `state` (Option = no row) to the HTTP error for an
/// attempted compose write, returning `Some(_)` when the request must be
/// rejected. `Composing` and `Active` accept compose updates; `Discarded`
/// is the only hard reject. Archived threads carry `state='active'` plus
/// `archive_state='archived'` and so flow through the `Active` arm — the
/// gmail-like revival behavior (keystrokes lead up to the send that
/// re-surfaces the thread) is preserved without needing a separate
/// `Archived` value on this column. Used by the cold path after a zero-row
/// UPDATE, where `state` is read back via a follow-up lookup to distinguish
/// "no row" (404) from "row but wrong state" (410).
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
    let mode = canonical_mode(&body.mode)?;

    let row: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT state, archive_state, compose_mode FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(body.id)
    .fetch_optional(state.engine.pool())
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;

    if let Some((state_str, archive_state_str, existing_mode)) = row {
        let existing_state =
            ThreadState::from_db_str(&state_str).map_err(|e| ApiError::internal(e.to_string()))?;
        // Each arm fully handles its state — no spatial-ordering dependency
        // between an early-return and a later match. Discarded returns 410
        // unconditionally (the `ThreadDiscarded` projection sets BOTH
        // state='discarded' AND archive_state='archived', and the 410-Gone
        // contract for "this id is dead, mint a new one" must not be masked
        // by the post-collapse archive flag). Active distinguishes the
        // archived sub-case for a more specific 409 message.
        return match existing_state {
            ThreadState::Composing => {
                if existing_mode.as_deref() == Some(mode) {
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
            mode: mode.to_string(),
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

/// Upload a legacy inline-base64 compose batch and return its blob hashes.
///
/// The count cap is answered BEFORE the first write. Checking it after the
/// loop still lands one blob file per image in the workspace, and the 413 it
/// then answers references none of them. A client repeating that with fresh
/// pixel noise grows the blob store without bound.
fn upload_legacy_compose_images(
    workspace: &std::path::Path,
    images: Vec<LegacyComposeImage>,
) -> Result<Vec<String>, ApiError> {
    if images.len() > MAX_COMPOSE_IMAGES {
        return Err(too_many_compose_images());
    }
    if images.is_empty() {
        return Ok(Vec::new());
    }
    crate::log!(
        "[Compat] legacy image upload via PUT compose ({} images)",
        images.len()
    );
    let mut hashes = Vec::with_capacity(images.len());
    for img in images {
        let blob = write_blob_from_base64(workspace, &img.base64).map_err(|e| {
            let status = match e {
                crate::core::blobs::BlobError::BadEncoding(_) => StatusCode::BAD_REQUEST,
                crate::core::blobs::BlobError::UnsupportedMime(_) => {
                    StatusCode::UNSUPPORTED_MEDIA_TYPE
                }
                crate::core::blobs::BlobError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
            };
            ApiError::new(status, e.to_string())
        })?;
        hashes.push(blob.hash);
    }
    Ok(hashes)
}

/// The write was composed against a *compose epoch* a submission has since
/// consumed, so it was not applied. `412 Precondition Failed` is the exact HTTP
/// semantic for a failed optimistic-concurrency check, and this endpoint's 409
/// already means the unrelated mode lock, so the status alone tells the two
/// apart without a bespoke error code. The body carries the current epoch so
/// the client can adopt it and re-issue in one round trip.
///
/// Hand-built rather than returned through `ApiError`, which is a bare
/// `{"error": msg}` and has nowhere to put the epoch.
fn stale_compose_epoch_response(current_epoch: i64) -> Response {
    (
        StatusCode::PRECONDITION_FAILED,
        Json(serde_json::json!({
            "error": "compose write is stale: the draft was consumed by a submission",
            "compose_epoch": current_epoch,
        })),
    )
        .into_response()
}

/// PUT /api/v1/threads/:id/compose — update compose fields.
///
/// One round-trip: `UPDATE ... RETURNING compose_mode, …` folds the
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
) -> Result<Response, ApiError> {
    if body.text.len() > MAX_COMPOSE_TEXT_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "compose_text exceeds 64 KiB cap",
        ));
    }
    let mode = match body.mode {
        Some(ref m) => Some(canonical_mode(m)?),
        None => None,
    };

    // None = preserve (SQL COALESCE). `image_hashes` wins over legacy
    // `images`; the latter is uploaded inline before the UPDATE.
    let new_image_hashes: Option<Vec<String>> = if let Some(hashes) = body.image_hashes {
        if let Some(rejection) = reject_compose_images(&hashes) {
            return Err(rejection);
        }
        Some(hashes)
    } else if let Some(legacy) = body.images {
        Some(upload_legacy_compose_images(
            state.engine.workspace_path(),
            legacy,
        )?)
    } else {
        None
    };

    let images_bind: Option<JsonValue> = new_image_hashes
        .as_ref()
        .map(|h| serde_json::Value::Array(h.iter().cloned().map(JsonValue::String).collect()));

    // Guard the selection payload: it fans out over SSE like the text/images do,
    // and a partial `ComposeSelectionOverride` is tiny (a handful of short
    // fields), so anything large is a runaway client, not a real draft.
    if let Some(ref sel) = body.selection {
        if serde_json::to_string(sel).map(|s| s.len()).unwrap_or(0) > MAX_COMPOSE_SELECTION_BYTES {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "compose_selection exceeds size cap",
            ));
        }
    }

    // Mode toggle on a thread that's already past `composing` is a contract
    // violation — pre-check so we surface 409 before the UPDATE rejects it
    // for an unrelated reason and we lose the precise error.
    // `source` mirrors `compose_mode` so a draft that auto-archives without
    // being sent still renders with the correct channel pill. Send events
    // later overwrite source from the actual channel of the message. The
    // WHERE clause already gates mode-carrying writes to `state='composing'`.
    //
    // `compose_images` uses COALESCE($3, compose_images): NULL bind preserves
    // the existing array, `[]` clears it. `compose_selection` uses
    // COALESCE($5, compose_selection) the same way: a text-only/keystroke PUT
    // (NULL bind) must preserve the draft's stored dropdown picks, while a
    // dropdown change sends the full object.
    //
    // `compose_epoch = $6` is the write fence. It costs nothing on the
    // keystroke path because the epoch counts SUBMISSIONS, not writes: every
    // PUT between two submissions carries the same value, so only a write that
    // straddles a submission can fail it.
    let row: Option<(Option<String>, JsonValue, Option<JsonValue>, i64)> = sqlx::query_as(
        "UPDATE thread_summaries
            SET compose_text = $2,
                compose_images = COALESCE($3, compose_images),
                compose_mode = COALESCE($4, compose_mode),
                compose_selection = COALESCE($5, compose_selection),
                source = CASE $4::text
                    WHEN 'claude_code' THEN 'claude_code'
                    WHEN 'lucidos'     THEN 'chat'
                    ELSE source
                END
          WHERE thread_id = $1
            AND state IN ('composing', 'active')
            AND ($4::text IS NULL OR state = 'composing')
            AND ($6::bigint IS NULL OR compose_epoch = $6)
         RETURNING compose_mode, compose_images, compose_selection, compose_epoch",
    )
    .bind(id)
    .bind(&body.text)
    .bind(images_bind.as_ref())
    .bind(mode)
    .bind(body.selection.as_ref())
    .bind(body.compose_epoch)
    .fetch_optional(state.engine.pool())
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;

    let (resolved_mode, post_compose_images, post_compose_selection, compose_epoch) = match row {
        Some(r) => r,
        None => {
            // Cold path: UPDATE matched zero rows. A follow-up read tells the
            // refusals apart, so the client gets the reason rather than a bare
            // "not applied".
            let lookup: Option<(String, i64)> = sqlx::query_as(
                "SELECT state, compose_epoch FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(id)
            .fetch_optional(state.engine.pool())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
            let current_epoch = lookup.as_ref().map(|(_, e)| *e);
            let st = lookup
                .map(|(s, _)| ThreadState::from_db_str(&s))
                .transpose()
                .map_err(|e| ApiError::internal(e.to_string()))?;
            // Answered most specific reason first, and the order is the
            // contract. A missing or discarded row settles the request on its
            // own. There is no draft to compose against, so neither the fence
            // nor the mode lock applies.
            if let Some(e) = compose_error(st) {
                return Err(e);
            }
            // The fence before the mode lock, and the order is the fix. A write
            // carrying an epoch a submission has consumed was composed against
            // the pre-send state, where its mode was legal. The epoch is why it
            // was not applied, and 412 is the one answer the client resyncs
            // from silently. Asking about the mode first reported that race as
            // an illegal mode change, and every keystroke write on a draft
            // carries a mode. A silent 204 stays wrong for the reason it always
            // was: the client would record the text as stored while the engine
            // dropped it.
            // See `docs/plans/2026-08-26-compose-mode-lock-masks-the-stale-write-fence.md`.
            if let (Some(sent), Some(current)) = (body.compose_epoch, current_epoch) {
                if sent != current {
                    return Ok(stale_compose_epoch_response(current));
                }
            }
            // Mode locks at first send. This write is CURRENT: it matched the
            // epoch, or carried none. A mode here is a real divergence rather
            // than a straggler. Archived rows carry state='active' and reach
            // this naturally. Without it the request falls through to the 204
            // below, which would swallow the mode change in silence.
            if mode.is_some() && matches!(st, Some(ThreadState::Active)) {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "mode is locked once the thread has been sent",
                ));
            }
            // TOCTOU: a concurrent send between the UPDATE and the lookup may
            // have flipped state to active, so the row now satisfies the state
            // guard. Treat as a benign no-op: the concurrent path already wrote
            // a more authoritative value.
            return Ok(StatusCode::NO_CONTENT.into_response());
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
        // Read back whatever COALESCE produced — the new object on a dropdown
        // change, the existing stored object on a preserve (keystroke) write —
        // so every SSE receiver hydrates the authoritative per-draft selection.
        selection: post_compose_selection,
        // Unchanged by a compose write (only a submission moves it), but
        // carried anyway so every broadcast is a complete report of the
        // thread's compose state and a receiver never has to merge two frames
        // to know which epoch the text belongs to.
        compose_epoch,
        origin_device_id: device_id,
    });
    state
        .engine
        .event_bus
        .emit(event)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT.into_response())
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
    let lookup: Option<(String, String)> =
        sqlx::query_as("SELECT state, archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(id)
            .fetch_optional(state.engine.pool())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    let Some((state_str, archive_state_str)) = lookup else {
        return Ok(StatusCode::NO_CONTENT);
    };
    let current_state =
        ThreadState::from_db_str(&state_str).map_err(|e| ApiError::internal(e.to_string()))?;
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
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "thread already archived",
                ));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1x1 PNG whose trailing byte varies, so each call is distinct content
    /// and lands its own blob. Enough leading bytes to pass the magic sniff.
    fn distinct_png_base64(seed: u8) -> String {
        use base64::Engine as _;
        let mut bytes: Vec<u8> = vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H',
            b'D', b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00,
        ];
        bytes.push(seed);
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    }

    fn legacy_batch(count: u8) -> Vec<LegacyComposeImage> {
        (0..count)
            .map(|i| LegacyComposeImage {
                base64: distinct_png_base64(i),
            })
            .collect()
    }

    /// Count the blob files the workspace holds, across the 256-way fan-out.
    fn blob_count(workspace: &std::path::Path) -> usize {
        let root = workspace.join("data/blobs");
        let Ok(shards) = std::fs::read_dir(&root) else {
            return 0;
        };
        shards
            .filter_map(Result::ok)
            .filter_map(|shard| std::fs::read_dir(shard.path()).ok())
            .map(|files| files.filter_map(Result::ok).count())
            .sum()
    }

    /// An over-cap legacy batch is refused before anything is written. The
    /// cap used to run after the upload loop, so the workspace kept one blob
    /// per image of a request the engine had answered 413.
    #[test]
    fn an_over_cap_legacy_upload_writes_no_blobs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let over_cap = (MAX_COMPOSE_IMAGES + 1) as u8;

        let err = upload_legacy_compose_images(tmp.path(), legacy_batch(over_cap))
            .expect_err("over the cap is a refusal");
        assert_eq!(err.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            blob_count(tmp.path()),
            0,
            "a refused batch must leave no blob behind"
        );
    }

    /// A batch inside the cap still uploads, one blob per distinct image.
    #[test]
    fn a_legacy_upload_inside_the_cap_writes_one_blob_per_image() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let hashes =
            upload_legacy_compose_images(tmp.path(), legacy_batch(3)).expect("inside the cap");
        assert_eq!(hashes.len(), 3);
        assert!(hashes.iter().all(|h| is_blob_hash(h)));
        assert_eq!(blob_count(tmp.path()), 3);
    }

    /// `image_hashes` is a list of blob addresses, and the count cap bounds it
    /// only once each entry has a bounded length. One unvalidated entry of any
    /// size passes the count, lands in the projection row, and goes out in
    /// every SSE frame. A megabyte here stands in for the reported 90 MB.
    #[test]
    fn a_compose_image_hash_that_is_not_a_blob_address_is_refused() {
        let real = "a".repeat(64);
        assert!(reject_compose_images(std::slice::from_ref(&real)).is_none());

        let huge = vec!["x".repeat(1024 * 1024)];
        let err = reject_compose_images(&huge).expect("an oversized entry is refused");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(
            err.message.len() < 200 && !err.message.contains(&huge[0]),
            "the refusal must not echo the rejected value back"
        );

        let non_hex = "g".repeat(64);
        for bad in ["", "zz", non_hex.as_str(), &real[..63]] {
            assert!(
                reject_compose_images(&[bad.to_string()]).is_some(),
                "{bad:?} is not a 64-character hex blob address"
            );
        }
    }

    /// The count cap still holds, and both image paths report it the same way.
    #[test]
    fn more_hashes_than_the_cap_are_refused_with_the_shared_message() {
        let hashes: Vec<String> = (0..=MAX_COMPOSE_IMAGES)
            .map(|i| format!("{i:064x}"))
            .collect();
        let err = reject_compose_images(&hashes).expect("over the cap is a refusal");
        assert_eq!(err.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(err.message, too_many_compose_images().message);
    }
}
