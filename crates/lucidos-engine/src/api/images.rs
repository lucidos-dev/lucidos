use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    Json,
};
use base64::Engine as _;
use serde::Serialize;
use uuid::Uuid;

use super::AppState;
use crate::core::events::{walk_thread_images, walk_thread_images_meta};

#[derive(Serialize)]
pub struct ThreadImageInfo {
    pub index: usize,
    pub source: &'static str, // "user" or "generated"
    pub mime_type: String,
}

/// GET /api/threads/:thread_id/images — list all images in a thread
pub(super) async fn list_thread_images(
    State(state): State<AppState>,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<Vec<ThreadImageInfo>>, (StatusCode, String)> {
    let events = state
        .event_store
        .get_thread_events_by_seq(thread_id, None)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("DB error: {}", e),
            )
        })?;

    // Listing is metadata-only — skip the per-blob read+encode that the
    // bytes-loading walker would do.
    let images = walk_thread_images_meta(state.engine.workspace_path(), &events)
        .into_iter()
        .map(|img| ThreadImageInfo {
            index: img.index,
            source: img.source,
            mime_type: img.mime_type,
        })
        .collect();

    Ok(Json(images))
}

/// GET /api/threads/:thread_id/images/:index — serve a single image as raw bytes
pub(super) async fn get_thread_image(
    State(state): State<AppState>,
    Path((thread_id, target_index)): Path<(Uuid, usize)>,
) -> Result<([(header::HeaderName, &'static str); 2], Vec<u8>), (StatusCode, String)> {
    if target_index == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Image index must be 1 or greater".to_string(),
        ));
    }

    let events = state
        .event_store
        .get_thread_events_by_seq(thread_id, None)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("DB error: {}", e),
            )
        })?;

    let images = walk_thread_images(state.engine.workspace_path(), &events);
    let total = images.len();

    let img = images
        .into_iter()
        .find(|i| i.index == target_index)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!(
                    "Image {} not found. Thread contains {} images.",
                    target_index, total
                ),
            )
        })?;

    // Empty base64 means walk_thread_images couldn't read the blob
    // (typically a partial backup-restore). The numbering slot exists so
    // thread:N stays stable, but there are no bytes to serve.
    if img.base64.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Image {} blob is missing on disk.", target_index),
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&img.base64)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("base64 decode: {}", e),
            )
        })?;

    let content_type = if img.mime_type.contains("png") {
        "image/png"
    } else {
        "image/jpeg"
    };

    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "max-age=31536000, immutable"),
        ],
        bytes,
    ))
}
