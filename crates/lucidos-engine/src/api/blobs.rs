//! HTTP endpoints for the content-addressed image blob store.
//!
//! - POST /api/v1/threads/:id/blobs (multipart) → writes blob, emits
//!   ImageUploaded, returns `{hash, mime, byte_size}`.
//! - GET  /api/v1/blobs/:hash → streams the original blob with `Content-Type`
//!   set to the sniffed mime and `Cache-Control: immutable`.
//! - GET  /api/v1/blobs/:hash/preview → streams a downscaled JPEG suitable
//!   for browser display (long edge ≤ `PREVIEW_MAX_EDGE`). iOS Safari
//!   silently fails to decode multi-megapixel images at fullscreen size
//!   and renders them black; the preview keeps the decoded bitmap inside
//!   WebKit's per-image budget.
//!
//! See `docs/plans/2026-05-07-image-blob-store-design.md`.

use axum::{
    body::Body,
    extract::{Multipart, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use uuid::Uuid;

use crate::api::actor::user_actor_resolved;
use crate::api::{ApiError, AppState};
use crate::core::blobs::{
    get_or_create_preview, resolve_blob, write_blob, BlobError, PREVIEW_MAX_EDGE,
};
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::thread_state::ThreadState;

#[derive(Debug, Serialize)]
pub(super) struct BlobUploadResponse {
    pub hash: String,
    pub mime: String,
    pub byte_size: u64,
}

/// POST /api/v1/threads/:id/blobs — content-addressed image upload.
///
/// State-machine guard mirrors `PUT /threads/:id/compose`: rejects with
/// 410 on `discarded`, 404 on missing, accepts `composing|active`. Archived
/// threads carry `state='active'` plus `archive_state='archived'`, and
/// uploads into them are accepted so a gmail-like revival reply can attach
/// images. Same blob attached to two threads = two `ImageUploaded` events but
/// a single on-disk file (write_blob is idempotent on identical bytes).
pub(super) async fn post_blob(
    State(state): State<AppState>,
    Path(thread_id): Path<Uuid>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<BlobUploadResponse>), ApiError> {
    // Verify thread state before reading the upload body — fail fast on
    // 404/410 without spending bandwidth on the file.
    let lookup: Option<(String,)> =
        sqlx::query_as("SELECT state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(state.engine.pool())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    let current_state = lookup
        .map(|(s,)| ThreadState::from_db_str(&s))
        .transpose()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    match current_state {
        None => return Err(ApiError::not_found("thread not found")),
        Some(ThreadState::Discarded) => {
            return Err(ApiError::new(StatusCode::GONE, "thread discarded"))
        }
        Some(ThreadState::Composing | ThreadState::Active) => {}
    }

    // Read the single `file` part. The frontend uploads exactly one image
    // per request so the user can show per-file progress + retry; bulk
    // uploads have no UX advantage and would just complicate error reporting.
    let field = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("multipart read: {e}")))?
        .ok_or_else(|| ApiError::bad_request("missing 'file' part"))?;
    let bytes = field
        .bytes()
        .await
        .map_err(|e| ApiError::bad_request(format!("read body: {e}")))?;

    // write_blob sniffs the mime against the allowlist; on rejection
    // it returns BlobError::UnsupportedMime → 415 with no disk write.
    // The error carries what the bytes turned out to be, so the 415 names
    // this upload's format rather than reciting the allowlist.
    let resolved = match write_blob(&state.workspace_path, &bytes) {
        Ok(r) => r,
        Err(e @ BlobError::UnsupportedMime(_)) => {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                e.to_string(),
            ));
        }
        Err(BlobError::Io(e)) => return Err(ApiError::internal(e.to_string())),
        // write_blob never returns BadEncoding (it takes raw bytes); the
        // variant only fires through write_blob_from_base64.
        Err(BlobError::BadEncoding(_)) => unreachable!(),
    };

    let actor = user_actor_resolved(&headers, state.engine.pool(), None).await;
    let event = BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ImageUploaded {
            hash: resolved.hash.clone(),
            mime: resolved.mime.clone(),
            byte_size: resolved.byte_size,
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

    // Pre-generate the browser-display preview off the request path so the
    // first GET /blobs/:hash/preview always hits the warm cache. Lazy
    // generation took 16-25 s for large iPhone JPEGs in dev — long enough
    // for iOS Safari to abort the fetch and leave the page with broken
    // `<img>` elements until reload.
    let workspace = state.workspace_path.clone();
    let preview_hash = resolved.hash.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = get_or_create_preview(&workspace, &preview_hash, PREVIEW_MAX_EDGE) {
            crate::log!("[Blobs] preview pre-generation failed for {preview_hash}: {e}");
        }
    });

    Ok((
        StatusCode::CREATED,
        Json(BlobUploadResponse {
            hash: resolved.hash,
            mime: resolved.mime,
            byte_size: resolved.byte_size,
        }),
    ))
}

/// Stream `path` with the given `mime` and the immutable cache header
/// shared by both blob endpoints. Content-addressed = the bytes never
/// change for a hash, so once fetched the browser never re-requests.
async fn stream_immutable(path: std::path::PathBuf, mime: String) -> Result<Response, ApiError> {
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let stream = tokio_util::io::ReaderStream::new(file);
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime).map_err(|e| ApiError::internal(e.to_string()))?,
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    Ok(response.into_response())
}

/// GET /api/v1/blobs/:hash — stream the original blob bytes. Streamed via
/// `tokio::fs::File` so the async runtime never blocks on a multi-MB read
/// and the response body flows in chunks instead of materializing in memory.
pub(super) async fn get_blob(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> Result<Response, ApiError> {
    let resolved = match resolve_blob(&state.workspace_path, &hash) {
        Some(r) => r,
        None => return Err(ApiError::not_found("blob not found")),
    };
    stream_immutable(resolved.path, resolved.mime).await
}

/// GET /api/v1/blobs/:hash/preview — stream a downscaled JPEG (long edge
/// ≤ `PREVIEW_MAX_EDGE`) suitable for browser display. Resize is done once
/// and cached under `.lucidos/blob-previews/`; second hit is a plain disk
/// read. Decode happens off the async runtime via `spawn_blocking` so a
/// 10 MB iPhone JPEG doesn't stall other requests.
pub(super) async fn get_blob_preview(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> Result<Response, ApiError> {
    let workspace = state.workspace_path.clone();
    let resolved = tokio::task::spawn_blocking(move || {
        get_or_create_preview(&workspace, &hash, PREVIEW_MAX_EDGE)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let resolved = match resolved {
        Some(r) => r,
        None => return Err(ApiError::not_found("blob not found")),
    };
    stream_immutable(resolved.path, resolved.mime).await
}

/// Routes for the `/blobs/*` read surface. The upload route is
/// thread-shaped (`POST /api/v1/threads/:id/blobs`) and registers under
/// `threads::router()`.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/blobs/:hash", get(get_blob))
        .route("/blobs/:hash/preview", get(get_blob_preview))
}
