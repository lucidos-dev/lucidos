//! `/api/v1/widgets`: a thread's widgets, its shelf, and reusable widgets
//! (ADRs 0402, 0407).
//!
//! The UI's widget menu and the `lucidos widgets` CLI both land here. The
//! agent's grouped `widgets` tool runs the same checks in-process.

use super::error::ApiError;
use super::*;

use crate::core::App;
use crate::engine::widgets::{self, ThreadWidget, WidgetChange, WidgetCheckFailed};

#[derive(Debug, Deserialize)]
pub(super) struct ThreadWidgetsQuery {
    thread_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WidgetChangeBody {
    app_id: String,
    thread_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WidgetBody {
    app_id: String,
}

/// An HTTP caller has no thread of its own, so it names one.
fn required_thread(raw: Option<&str>) -> Result<uuid::Uuid, ApiError> {
    let raw = raw.ok_or_else(|| ApiError::bad_request("thread_id is required"))?;
    resolve_thread_id_arg(raw, None).map_err(ApiError::bad_request)
}

fn refusal(e: WidgetCheckFailed) -> ApiError {
    match e {
        WidgetCheckFailed::Refused(reason) => ApiError::new(StatusCode::CONFLICT, reason),
        WidgetCheckFailed::ReadFailed(_) => ApiError::internal(e.to_string()),
    }
}

/// GET /api/v1/widgets - The reusable widgets.
async fn list_reusable_widgets(State(state): State<AppState>) -> Result<Json<Vec<App>>, ApiError> {
    state
        .app_manager
        .list_reusable_widgets()
        .map(Json)
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// GET /api/v1/widgets/thread?thread_id= - A thread's widgets, pinned or not.
async fn thread_widgets(
    State(state): State<AppState>,
    Query(query): Query<ThreadWidgetsQuery>,
) -> Result<Json<Vec<ThreadWidget>>, ApiError> {
    let thread_id = required_thread(query.thread_id.as_deref())?;
    widgets::thread_widgets(&state.pool, &state.app_manager, thread_id)
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(e.to_string()))
}

async fn change_shelf(
    state: &AppState,
    headers: &HeaderMap,
    change: WidgetChange,
    body: WidgetChangeBody,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::user_actor(headers, None);
    let thread_id = required_thread(body.thread_id.as_deref())?;
    widgets::check_widget_change(
        &state.pool,
        &state.app_manager,
        change,
        &body.app_id,
        thread_id,
    )
    .await
    .map_err(refusal)?;
    widgets::emit_widget_change(
        &state.engine.event_bus,
        change,
        &body.app_id,
        thread_id,
        actor,
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/v1/widgets/show - Show a widget in a thread.
async fn show_widget(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WidgetChangeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_shelf(&state, &headers, WidgetChange::Show, body).await
}

/// POST /api/v1/widgets/pin - "Pin to shelf". Writes one event, no file.
async fn pin_widget(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WidgetChangeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_shelf(&state, &headers, WidgetChange::Pin, body).await
}

/// POST /api/v1/widgets/unpin - "Unpin from shelf". Writes one event, no file.
async fn unpin_widget(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WidgetChangeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_shelf(&state, &headers, WidgetChange::Unpin, body).await
}

async fn set_reusable(
    state: &AppState,
    headers: &HeaderMap,
    app_id: &str,
    reusable: bool,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::user_actor(headers, None);
    widgets::check_set_reusable(&state.pool, &state.app_manager, app_id, reusable)
        .await
        .map_err(refusal)?;
    let commit = state
        .app_manager
        .set_widget_reusable(&state.engine.event_bus, app_id, reusable, actor)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "commit": commit })))
}

/// POST /api/v1/widgets/make-reusable - Offer a widget to every thread.
async fn make_reusable(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WidgetBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    set_reusable(&state, &headers, &body.app_id, true).await
}

/// POST /api/v1/widgets/stop-reusing - Hand a widget back to its origin thread.
async fn stop_reusing(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WidgetBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    set_reusable(&state, &headers, &body.app_id, false).await
}

/// Routes for the `/widgets` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/widgets", get(list_reusable_widgets))
        .route("/widgets/thread", get(thread_widgets))
        .route("/widgets/show", post(show_widget))
        .route("/widgets/pin", post(pin_widget))
        .route("/widgets/unpin", post(unpin_widget))
        .route("/widgets/make-reusable", post(make_reusable))
        .route("/widgets/stop-reusing", post(stop_reusing))
}
