use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::api::actor::user_actor;
use crate::api::thread_reach::refuse_without_authority;
use crate::api::{ApiError, AppState};
use crate::core::blobs::write_blob_from_base64;
use crate::core::store::{
    ComposeReport, ComposeReportRow, COMPOSE_REPORT_COLS, MAX_COMPOSE_IMAGES,
    MAX_COMPOSE_SELECTION_BYTES, MAX_COMPOSE_TEXT_BYTES,
};
use crate::engine::event_bus::BusEvent;
use crate::engine::permission::Action;
use crate::engine::thread_events::{EventMeta, RecoveredCompose, ThreadEvent};
use crate::engine::thread_state::ThreadState;

pub(in crate::api) const COMPOSE_VERB: Action = Action::Control;
pub(in crate::api) const DISCARD_DRAFT_VERB: Action = Action::Archive;

#[derive(Debug, Deserialize)]
pub(super) struct PostThreadBody {
    pub id: Uuid,
    pub mode: String,
    #[serde(default)]
    pub recovered_compose: Option<RecoveredCompose>,
    /// INVARIANT: Request-only bytes: never copied into ThreadStarted or SSE.
    #[serde(default)]
    pub recovered_images: Vec<RecoveredImage>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RecoveredImage {
    pub hash: String,
    pub base64: String,
}

fn restore_compose_images(
    workspace: &std::path::Path,
    hashes: &[String],
    images: Vec<RecoveredImage>,
) -> Result<(), ApiError> {
    use crate::core::blobs::{compute_hash, sniff_image_mime, write_blob};
    use base64::Engine as _;
    if images.len() > MAX_COMPOSE_IMAGES {
        return Err(too_many_compose_images());
    }
    if images.iter().map(|image| image.base64.len()).sum::<usize>() > 96 * 1024 * 1024 {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "recovered images exceed size cap",
        ));
    }
    let mut decoded = Vec::with_capacity(images.len());
    let mut seen = std::collections::HashSet::new();
    for image in images {
        if !hashes.contains(&image.hash) || !seen.insert(image.hash.clone()) {
            return Err(ApiError::bad_request(
                "recovered image must name one unique compose hash",
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.base64)
            .map_err(|_| ApiError::bad_request("invalid recovered image base64"))?;
        if compute_hash(&bytes) != image.hash {
            return Err(ApiError::bad_request("recovered image hash mismatch"));
        }
        if sniff_image_mime(&bytes).is_none() {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported recovered image",
            ));
        }
        decoded.push(bytes);
    }
    for bytes in decoded {
        write_blob(workspace, &bytes).map_err(|e| ApiError::internal(e.to_string()))?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub(super) struct LegacyComposeImage {
    pub base64: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct PutComposeBody {
    pub text: String,
    #[serde(default)]
    pub image_hashes: Option<Vec<String>>,
    #[serde(default)]
    pub images: Option<Vec<LegacyComposeImage>>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub selection: Option<JsonValue>,
    #[serde(default)]
    pub compose_epoch: Option<i64>,
}

fn canonical_mode(mode: &str) -> Result<&'static str, ApiError> {
    match mode {
        "lucidos" => Ok("lucidos"),
        "claude_code" | "claude-code" => Ok("claude_code"),
        _ => Err(ApiError::bad_request(
            "mode must be lucidos|claude-code (claude_code is accepted, and is what is stored)",
        )),
    }
}

fn too_many_compose_images() -> ApiError {
    ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "too many compose images")
}

fn is_blob_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())
}

fn reject_compose_images(hashes: &[String]) -> Option<ApiError> {
    if hashes.len() > MAX_COMPOSE_IMAGES {
        return Some(too_many_compose_images());
    }
    let bad = hashes.iter().find(|h| !is_blob_hash(h))?;
    Some(ApiError::bad_request(format!(
        "compose image hash must be 64 hex characters, got {} bytes",
        bad.len()
    )))
}

fn compose_error(state: Option<ThreadState>) -> Option<ApiError> {
    let Some(state) = state else {
        return Some(ApiError::not_found("thread not found"));
    };
    match state {
        ThreadState::Composing | ThreadState::Active => None,
        ThreadState::Discarded => Some(ApiError::new(StatusCode::GONE, "thread discarded")),
    }
}

pub(super) async fn post_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PostThreadBody>,
) -> Result<(StatusCode, Json<ComposeReport>), ApiError> {
    let mode = canonical_mode(&body.mode)?;
    if body.recovered_compose.is_none() && !body.recovered_images.is_empty() {
        return Err(ApiError::bad_request(
            "recovered images require recovered compose",
        ));
    }
    if let Some(recovered) = &body.recovered_compose {
        refuse_without_authority(
            &state.engine.event_bus,
            &headers,
            Some(body.id),
            COMPOSE_VERB,
        )
        .await?;
        let boundary = crate::core::snapshot::boundary::read(state.engine.pool())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if boundary.map(|b| b.restore_id) != Some(recovered.generation) {
            return Err(ApiError::new(
                StatusCode::PRECONDITION_FAILED,
                "snapshot_restore_generation_mismatch",
            ));
        }
        if recovered.text.len() > MAX_COMPOSE_TEXT_BYTES {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "compose_text exceeds 64 KiB cap",
            ));
        }
        if let Some(rejection) = reject_compose_images(&recovered.image_hashes) {
            return Err(rejection);
        }
        if recovered
            .selection
            .as_ref()
            .is_some_and(|s| s.to_string().len() > MAX_COMPOSE_SELECTION_BYTES)
        {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "compose_selection exceeds size cap",
            ));
        }
    }
    let recovering = body.recovered_compose.is_some();

    let row: Option<PostThreadRow> = sqlx::query_as(&format!(
        "SELECT state, archive_state, {COMPOSE_REPORT_COLS} FROM thread_summaries WHERE thread_id = $1"
    ))
    .bind(body.id)
    .fetch_optional(state.engine.pool())
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;

    if let Some((
        state_str,
        archive_state_str,
        version,
        epoch,
        text,
        images,
        stored_mode,
        selection,
        restorations,
    )) = row
    {
        let existing = ComposeReport::from((
            version,
            epoch,
            text,
            images,
            stored_mode,
            selection,
            restorations,
        ));
        // INVARIANT: A recovery retry observes the destination without restoring its old
        // payload over subsequent user edits, sends, mode changes or discards.
        if recovering {
            state
                .engine
                .event_bus
                .emit(existing.clone().broadcast(body.id, None))
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
            return Ok((StatusCode::OK, Json(existing)));
        }
        let existing_state =
            ThreadState::from_db_str(&state_str).map_err(|e| ApiError::internal(e.to_string()))?;
        return match existing_state {
            ThreadState::Composing => {
                if existing.mode.as_deref() == Some(mode) {
                    Ok((StatusCode::OK, Json(existing)))
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

    if let Some(recovered) = &body.recovered_compose {
        let workspace = state.engine.workspace_path().to_path_buf();
        let hashes = recovered.image_hashes.clone();
        tokio::task::spawn_blocking(move || {
            restore_compose_images(&workspace, &hashes, body.recovered_images)
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))??;
        for hash in &recovered.image_hashes {
            if crate::core::blobs::resolve_blob(state.engine.workspace_path(), hash).is_none() {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "recovered compose image is unavailable",
                ));
            }
        }
    }
    let actor = user_actor(&headers, None);
    let event = BusEvent::Thread {
        thread_id: body.id,
        event: ThreadEvent::ThreadStarted {
            recovered_compose: body.recovered_compose,
            agent_draft: None,
            mode: mode.to_string(),
            actor: actor.clone(),
        },
        meta: EventMeta {
            actor,
            ..Default::default()
        },
    };
    let created = state
        .engine
        .event_bus
        .emit(event)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .is_some();
    // INVARIANT: `ThreadStarted` inserts with `ON CONFLICT DO NOTHING`, so a racing POST
    // from another device may own the row. Read it back: the report is then
    // whatever the row holds, in one consistent statement.
    let row: ComposeReportRow = sqlx::query_as(&format!(
        "SELECT {COMPOSE_REPORT_COLS} FROM thread_summaries WHERE thread_id = $1"
    ))
    .bind(body.id)
    .fetch_one(state.engine.pool())
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let report = ComposeReport::from(row);
    if !recovering && report.mode.as_deref() != Some(mode) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "thread already exists with a different mode",
        ));
    }
    if recovering {
        state
            .engine
            .event_bus
            .emit(report.clone().broadcast(body.id, None))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(report),
    ))
}

type PostThreadRow = (
    String,
    String,
    i64,
    i64,
    String,
    JsonValue,
    Option<String>,
    Option<JsonValue>,
    JsonValue,
);

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
    crate::log_legacy!(
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

fn stale_compose_epoch_response(current: ComposeReport) -> Response {
    #[derive(serde::Serialize)]
    struct StaleBody {
        error: &'static str,
        #[serde(flatten)]
        current: ComposeReport,
    }
    (
        StatusCode::PRECONDITION_FAILED,
        Json(StaleBody {
            error: "compose write is stale: the draft was consumed by a submission",
            current,
        }),
    )
        .into_response()
}

fn compose_update_sql() -> String {
    format!(
        "UPDATE thread_summaries
            SET compose_text = $2,
                compose_images = COALESCE($3, compose_images),
                compose_mode = COALESCE($4, compose_mode),
                compose_selection = CASE WHEN $5::jsonb IS NULL THEN compose_selection
                    ELSE (CASE WHEN compose_selection->>'codingAgent' IS NOT NULL
                        THEN jsonb_build_object('codingAgent', compose_selection->'codingAgent')
                        ELSE '{{}}'::jsonb END) || $5::jsonb END,
                compose_updated_at = now(),
                source = CASE $4::text
                    WHEN 'claude_code' THEN 'claude_code'
                    WHEN 'lucidos'     THEN 'chat'
                    ELSE source
                END
          WHERE thread_id = $1
            AND state IN ('composing', 'active')
            AND ($4::text IS NULL OR state = 'composing')
            AND ($6::bigint IS NULL OR compose_epoch = $6)
         RETURNING {COMPOSE_REPORT_COLS}"
    )
}

/// INVARIANT: PUT /api/v1/threads/:id/compose — update compose fields.
///
/// A shared transaction lock excludes coding-agent seed changes while the
/// UPDATE applies the state-machine guard and returns the compose report.
/// The report is both the `200` body and the SSE payload. Keystrokes write
/// no event row.
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
    // INVARIANT: Before the legacy upload too, so a refused caller leaves no blob behind.
    refuse_without_authority(&state.engine.event_bus, &headers, Some(id), COMPOSE_VERB).await?;
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

    if let Some(ref sel) = body.selection {
        if serde_json::to_string(sel).map(|s| s.len()).unwrap_or(0) > MAX_COMPOSE_SELECTION_BYTES {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "compose_selection exceeds size cap",
            ));
        }
    }

    // INVARIANT: Mode toggle on a thread that's already past `composing` is a contract
    // violation — pre-check so we surface 409 before the UPDATE rejects it
    // for an unrelated reason and we lose the precise error.
    // `source` mirrors `compose_mode` so a draft that auto-archives without
    // being sent still renders with the correct channel pill. Send events
    // later overwrite source from the actual channel of the message. The
    // WHERE clause already gates mode-carrying writes to `state='composing'`.
    //
    // `compose_images` uses COALESCE($3, compose_images): NULL bind preserves
    // the existing array, `[]` clears it. A missing selection preserves all
    // stored picks. A present selection replaces them, except that omitting
    // codingAgent preserves its pin for clients editing a legacy draft.
    //
    // `compose_updated_at` dates the draft for the `threads` tool's `drafts`
    // action. Recovered drafts also initialize these fields in their creation event.
    //
    // `compose_epoch = $6` is the write fence. It costs nothing on the
    // keystroke path because only submissions and Stop restorations advance it.
    // A PUT straddling either change fails the fence.
    let mut tx = state
        .engine
        .pool()
        .begin()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    crate::core::preferences::lock_compose_agent_seed(&mut tx, false)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let before: Option<(Option<Uuid>, bool)> = sqlx::query_as(&format!(
        "SELECT t.parent_thread_id, {} FROM thread_summaries t WHERE t.thread_id = $1",
        crate::core::store::HAS_DRAFT_SQL,
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let row: Option<ComposeReportRow> = sqlx::query_as(&compose_update_sql())
        .bind(id)
        .bind(&body.text)
        .bind(images_bind.as_ref())
        .bind(mode)
        .bind(body.selection.as_ref())
        .bind(body.compose_epoch)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let Some(row) = row else {
        return refuse_unapplied_compose(&state, id, body.compose_epoch, mode).await;
    };
    let report = ComposeReport::from(row);
    if let Some((Some(parent), had_draft)) = before {
        let images = serde_json::json!(report.image_hashes);
        if had_draft != crate::core::store::has_draft(&report.text, &images) {
            state.engine.event_bus.request_archive_recount(parent);
        }
    }

    let device_id = headers
        .get(crate::core::subprocess_origin::HEADER_DEVICE_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    state
        .engine
        .event_bus
        .emit(report.clone().broadcast(id, device_id))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(report).into_response())
}

async fn refuse_unapplied_compose(
    state: &AppState,
    id: Uuid,
    sent_epoch: Option<i64>,
    mode: Option<&str>,
) -> Result<Response, ApiError> {
    let lookup: Option<StateAndReportRow> = sqlx::query_as(&format!(
        "SELECT state, {COMPOSE_REPORT_COLS} FROM thread_summaries WHERE thread_id = $1"
    ))
    .bind(id)
    .fetch_optional(state.engine.pool())
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let (st, current) = match lookup {
        Some((st, version, epoch, text, images, stored_mode, selection, restorations)) => (
            Some(ThreadState::from_db_str(&st).map_err(|e| ApiError::internal(e.to_string()))?),
            Some(ComposeReport::from((
                version,
                epoch,
                text,
                images,
                stored_mode,
                selection,
                restorations,
            ))),
        ),
        None => (None, None),
    };
    unapplied_compose_refusal(st, current, sent_epoch, mode)
}

fn unapplied_compose_refusal(
    st: Option<ThreadState>,
    current: Option<ComposeReport>,
    sent_epoch: Option<i64>,
    mode: Option<&str>,
) -> Result<Response, ApiError> {
    if let Some(e) = compose_error(st) {
        return Err(e);
    }
    let current: ComposeReport =
        current.ok_or_else(|| ApiError::internal("compose row vanished"))?;
    if sent_epoch.is_some_and(|sent| sent != current.compose_epoch) {
        return Ok(stale_compose_epoch_response(current));
    }
    if mode.is_some() && matches!(st, Some(ThreadState::Active)) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "mode is locked once the thread has been sent",
        ));
    }
    Err(ApiError::new(
        StatusCode::CONFLICT,
        "compose write raced the thread's creation and was not applied",
    )
    .with_reason("compose_not_applied"))
}

type StateAndReportRow = (
    String,
    i64,
    i64,
    String,
    JsonValue,
    Option<String>,
    Option<JsonValue>,
    JsonValue,
);

pub(super) async fn delete_thread(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // INVARIANT: Before the lookup, so a refusal says nothing about whether the thread exists.
    refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(id),
        DISCARD_DRAFT_VERB,
    )
    .await?;
    let lookup: Option<(String, String)> =
        sqlx::query_as("SELECT state, archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(id)
            .fetch_optional(state.engine.pool())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    let Some((state_str, archive_state_str)) = lookup else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    let current_state =
        ThreadState::from_db_str(&state_str).map_err(|e| ApiError::internal(e.to_string()))?;
    match current_state {
        ThreadState::Composing => {}
        ThreadState::Discarded => return Ok(StatusCode::NO_CONTENT.into_response()),
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

    let actor = user_actor(&headers, None);
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
    let emitted = state
        .engine
        .event_bus
        .emit(event)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let summary_version = emitted.and_then(|e| e.summary_version);
    Ok(Json(serde_json::json!({ "summary_version": summary_version })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn recovered_images_verify_entire_batch_before_landing_bytes() {
        use base64::Engine as _;
        let workspace = tempfile::tempdir().unwrap();
        let encoded = distinct_png_base64(1);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();
        let hash = crate::core::blobs::compute_hash(&bytes);
        let image = || RecoveredImage {
            hash: hash.clone(),
            base64: encoded.clone(),
        };
        let invalid = RecoveredImage {
            hash: "a".repeat(64),
            base64: encoded.clone(),
        };
        assert!(restore_compose_images(
            workspace.path(),
            &[hash.clone(), invalid.hash.clone()],
            vec![image(), invalid]
        )
        .is_err());
        assert!(crate::core::blobs::resolve_blob(workspace.path(), &hash).is_none());
        assert!(restore_compose_images(workspace.path(), &[], vec![image()]).is_err());
        assert!(restore_compose_images(
            workspace.path(),
            std::slice::from_ref(&hash),
            vec![image(), image()]
        )
        .is_err());
        restore_compose_images(workspace.path(), std::slice::from_ref(&hash), vec![image()])
            .unwrap();
        assert_eq!(
            std::fs::read(
                crate::core::blobs::resolve_blob(workspace.path(), &hash)
                    .unwrap()
                    .path
            )
            .unwrap(),
            bytes
        );
        restore_compose_images(workspace.path(), std::slice::from_ref(&hash), vec![image()])
            .unwrap();
    }

    fn legacy_batch(count: u8) -> Vec<LegacyComposeImage> {
        (0..count)
            .map(|i| LegacyComposeImage {
                base64: distinct_png_base64(i),
            })
            .collect()
    }

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

    #[test]
    fn a_legacy_upload_inside_the_cap_writes_one_blob_per_image() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let hashes =
            upload_legacy_compose_images(tmp.path(), legacy_batch(3)).expect("inside the cap");
        assert_eq!(hashes.len(), 3);
        assert!(hashes.iter().all(|h| is_blob_hash(h)));
        assert_eq!(blob_count(tmp.path()), 3);
    }

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

    #[test]
    fn more_hashes_than_the_cap_are_refused_with_the_shared_message() {
        let hashes: Vec<String> = (0..=MAX_COMPOSE_IMAGES)
            .map(|i| format!("{i:064x}"))
            .collect();
        let err = reject_compose_images(&hashes).expect("over the cap is a refusal");
        assert_eq!(err.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(err.message, too_many_compose_images().message);
    }

    fn report_at_epoch(epoch: i64) -> ComposeReport {
        ComposeReport::from((
            3,
            epoch,
            "draft".to_string(),
            serde_json::json!([]),
            Some("lucidos".to_string()),
            None,
            serde_json::json!([]),
        ))
    }

    fn status_of(result: Result<Response, ApiError>) -> StatusCode {
        match result {
            Ok(response) => response.status(),
            Err(e) => e.status,
        }
    }

    #[test]
    fn a_missing_or_discarded_row_is_answered_before_the_fence_and_the_mode_lock() {
        let stale = Some(1);
        let mode = Some("claude_code");
        assert_eq!(
            status_of(unapplied_compose_refusal(None, None, stale, mode)),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(unapplied_compose_refusal(
                Some(ThreadState::Discarded),
                Some(report_at_epoch(5)),
                stale,
                mode
            )),
            StatusCode::GONE
        );
    }

    #[test]
    fn a_stale_mode_write_takes_the_412_and_a_current_one_takes_the_409() {
        let stale = unapplied_compose_refusal(
            Some(ThreadState::Active),
            Some(report_at_epoch(5)),
            Some(4),
            Some("claude_code"),
        );
        assert_eq!(status_of(stale), StatusCode::PRECONDITION_FAILED);
        for sent in [Some(5), None] {
            let current = unapplied_compose_refusal(
                Some(ThreadState::Active),
                Some(report_at_epoch(5)),
                sent,
                Some("claude_code"),
            );
            assert_eq!(status_of(current), StatusCode::CONFLICT, "sent {sent:?}");
        }
    }

    #[tokio::test]
    async fn the_stale_write_response_is_a_412_carrying_the_current_report() {
        let response = stale_compose_epoch_response(report_at_epoch(9));
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: JsonValue = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["compose_epoch"], 9);
        assert_eq!(body["text"], "draft");
        assert!(body["error"].is_string());
    }

    async fn started_draft(bus: &crate::engine::event_bus::EventBus) -> Uuid {
        let id = Uuid::new_v4();
        bus.emit(BusEvent::Thread {
            thread_id: id,
            event: ThreadEvent::ThreadStarted {
                recovered_compose: None,
                agent_draft: None,
                mode: "lucidos".to_string(),
                actor: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("start the draft");
        id
    }

    async fn put_through_update(
        pool: &sqlx::PgPool,
        id: Uuid,
        mode: Option<&str>,
        epoch: Option<i64>,
    ) -> Option<ComposeReportRow> {
        sqlx::query_as(&compose_update_sql())
            .bind(id)
            .bind("text")
            .bind(None::<&JsonValue>)
            .bind(mode)
            .bind(None::<&JsonValue>)
            .bind(epoch)
            .fetch_optional(pool)
            .await
            .expect("the update runs")
    }

    #[tokio::test]
    async fn the_update_fences_a_stale_epoch_and_lets_an_epochless_write_land() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let id = started_draft(&bus).await;
        let epoch: i64 =
            sqlx::query_scalar("SELECT compose_epoch FROM thread_summaries WHERE thread_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert!(put_through_update(&pool, id, None, Some(epoch + 1))
            .await
            .is_none());
        assert!(put_through_update(&pool, id, None, Some(epoch))
            .await
            .is_some());
        assert!(put_through_update(&pool, id, None, None).await.is_some());

        pool.close().await;
        teardown_test_db(&db).await;
    }
}
