//! `/api/v1/threads/:thread_id/child-reports/take`, where a coding agent takes
//! its children's reports mid-turn (`lucidos threads child-reports`). See
//! `engine::agent_session::child_reports`.
//!
//! Every other response to that agent carries [`HEADER_CHILD_REPORTS_WAITING`]
//! while a report waits, and the CLI turns it into a one-line note.

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::background_tasks::{child_reports_json, require_own_thread_agent};
use super::parse_thread_id;
use crate::api::AppState;

/// How many child reports wait unread for the calling agent's own thread. Set
/// only when one does. The CLI mirrors the name.
pub(crate) const HEADER_CHILD_REPORTS_WAITING: &str = "lucidos-child-reports-waiting";

/// Marks a response that already carries the caller's child reports, so the
/// note does not announce them again.
#[derive(Clone, Copy, Debug)]
pub(in crate::api) struct ChildReportsCarried;

/// Stamp [`HEADER_CHILD_REPORTS_WAITING`] on a response to a thread's own
/// agent while a child report waits for it.
pub(in crate::api) async fn note_waiting_child_reports(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let caller_thread = match crate::api::actor::subprocess_origin(request.headers()) {
        crate::api::actor::SubprocessOrigin::Subprocess {
            source_thread_id: Some(thread_id),
            ..
        } => Some(thread_id),
        _ => None,
    };
    let mut response = next.run(request).await;
    let Some(thread_id) = caller_thread else {
        return response;
    };
    if response.extensions().get::<ChildReportsCarried>().is_some() {
        return response;
    }
    let waiting = match state.engine.child_reports_waiting(thread_id).await {
        Some(waiting) => *waiting.borrow(),
        None => 0,
    };
    if waiting > 0 {
        response
            .headers_mut()
            .insert(HEADER_CHILD_REPORTS_WAITING, HeaderValue::from(waiting));
    }
    response
}

/// POST /api/v1/threads/:thread_id/child-reports/take
///
/// Hands the thread's own agent every child report it has not read yet, and
/// withdraws each queued copy so the report arrives once.
pub(in crate::api) async fn take_child_reports(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let thread_id = parse_thread_id(&thread_id)?;
    require_own_thread_agent(&headers, thread_id)?;
    let taken = state
        .engine
        .take_child_reports(thread_id, crate::api::actor::user_actor(&headers, None))
        .await;
    let message = if taken.is_empty() {
        "No child report is waiting. One that arrives while your turn runs is \
         announced on your next `lucidos` call."
    } else {
        "Your children's reports, oldest first. They will not be delivered again."
    };
    let mut response = Json(serde_json::json!({
        "child_reports": child_reports_json(&taken),
        "message": message,
    }))
    .into_response();
    response.extensions_mut().insert(ChildReportsCarried);
    Ok(response)
}
