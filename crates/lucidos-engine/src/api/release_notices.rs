//! HTTP for *release notices*: what this workspace still owes the reader, and
//! the answer that settles one.
//!
//! Two endpoints, one list. The modal draws the notice named by `next_id`, and
//! the What's New panel draws them all. Both read this one response, so they
//! cannot disagree about what is answered.
//!
//! - `GET  /api/v1/release-notices`
//! - `POST /api/v1/release-notices/resolve` with body `{ "id": "<notice id>" }`
//!
//! The rules live in `engine::release_notices`. This module is the boundary.

use axum::extract::State;
use axum::{
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use super::error::ApiError;
use super::AppState;
use crate::engine::event_bus::SystemEvent;
use crate::engine::release_notices;

/// The release this engine reports, which decides what a caller may see or
/// answer.
///
/// Read here rather than taken from the request. A client could otherwise name
/// a release it is not on, and be handed a notice that has not shipped.
fn running_release() -> Result<semver::Version, ApiError> {
    semver::Version::parse(crate::LUCIDOS_RELEASE)
        .map_err(|e| ApiError::internal(format!("{} is not semver: {e}", crate::LUCIDOS_RELEASE)))
}

/// `GET /api/v1/release-notices`.
async fn list(
    State(state): State<AppState>,
) -> Result<Json<release_notices::NoticeView>, ApiError> {
    let running = running_release()?;
    let cursor = release_notices::stored_cursor(&state.pool).await;
    Ok(Json(release_notices::view(
        release_notices::all(),
        &running,
        cursor.as_deref(),
    )))
}

#[derive(Deserialize)]
struct ResolveRequest {
    id: String,
}

/// `POST /api/v1/release-notices/resolve`.
///
/// Answering a notice the workspace has already walked past is a no-op rather
/// than an error. Two devices showing the same modal is ordinary, so the second
/// answer changes nothing and announces nothing.
async fn resolve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ResolveRequest>,
) -> Result<Json<release_notices::NoticeView>, ApiError> {
    let running = running_release()?;
    let notices = release_notices::all();
    if !notices.iter().any(|n| n.id == body.id) {
        return Err(ApiError::not_found(format!(
            "no release notice with id {:?} in this build",
            body.id
        )));
    }
    let moved = release_notices::resolve(&state.pool, notices, &running, &body.id)
        .await
        .map_err(ApiError::db)?;
    if moved {
        state
            .engine
            .event_bus
            .emit_user_system(
                &headers,
                &state.pool,
                "[ReleaseNotices] ReleaseNoticeResolved",
                |actor| SystemEvent::ReleaseNoticeResolved {
                    notice_id: body.id.clone(),
                    actor,
                },
            )
            .await;
    }
    list(State(state)).await
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/release-notices", get(list))
        .route("/release-notices/resolve", post(resolve))
}
