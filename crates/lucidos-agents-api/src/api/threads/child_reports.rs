use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::background_tasks::{child_reports_json, require_own_thread_agent};
use super::parse_thread_id;
use crate::api::AppState;

pub(crate) const HEADER_CHILD_REPORTS_WAITING: &str = "lucidos-child-reports-waiting";

#[derive(Clone, Copy, Debug)]
pub(in crate::api) struct ChildReportsCarried;

pub(in crate::api) async fn note_waiting_child_reports(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let caller_thread = match crate::core::subprocess_origin::subprocess_origin(request.headers()) {
        crate::core::subprocess_origin::SubprocessOrigin::Subprocess {
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
