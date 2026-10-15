//! `/api/v1/widgets`: a thread's widgets, its shelf, and reusable widgets
//! (ADRs 0402, 0407).
//!
//! The UI's widget menu and the `lucidos widgets` CLI both land here. The
//! agent's grouped `widgets` tool runs the same checks in-process.

use super::error::ApiError;
use super::*;

use crate::core::App;
use crate::engine::home_board::{self, HomeBoard};
use crate::engine::thread_events::{ArrangedTile, LayoutClass};
use crate::engine::widgets::{
    self, ThreadWidget, WidgetChange, WidgetCheckFailed, WidgetInstance, WidgetInstanceArgs,
};

#[derive(Debug, Deserialize)]
pub(super) struct ThreadWidgetsQuery {
    thread_id: Option<String>,
}

/// A show, pin or unpin body: the thread, and the widget instance.
#[derive(Debug, Deserialize)]
pub(super) struct WidgetChangeBody {
    thread_id: Option<String>,
    /// A pin's reply event, where "Show in thread" lands. Unpin ignores it.
    #[serde(default)]
    shown_event_id: Option<uuid::Uuid>,
    #[serde(flatten)]
    instance: WidgetInstanceArgs,
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
    let instance = WidgetInstance::try_from(body.instance).map_err(ApiError::bad_request)?;
    let plan = widgets::check_widget_change(
        &state.pool,
        &state.app_manager,
        change,
        &instance,
        thread_id,
    )
    .await
    .map_err(refusal)?;
    let made_reusable = widgets::apply_widget_change(
        &state.app_manager,
        &state.engine.event_bus,
        plan,
        change,
        instance,
        thread_id,
        actor,
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(
        serde_json::json!({ "ok": true, "made_reusable": made_reusable }),
    ))
}

/// POST /api/v1/widgets/pin - "Pin to shelf". Writes one event, no file.
async fn pin_widget(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WidgetChangeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let change = WidgetChange::Pin {
        shown_event_id: body.shown_event_id,
    };
    change_shelf(&state, &headers, change, body).await
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

#[derive(Debug, Deserialize)]
pub(super) struct HomeBoardQuery {
    layout: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ArrangeBody {
    layout: String,
    tiles: Vec<ArrangedTile>,
}

fn layout_class(raw: Option<&str>) -> Result<LayoutClass, ApiError> {
    let raw = raw.ok_or_else(|| ApiError::bad_request("layout is required"))?;
    LayoutClass::from_arg(raw).map_err(ApiError::bad_request)
}

/// GET /api/v1/home-board?layout= - The Home board of one layout class.
async fn get_home_board(
    State(state): State<AppState>,
    Query(query): Query<HomeBoardQuery>,
) -> Result<Json<HomeBoard>, ApiError> {
    let layout = layout_class(query.layout.as_deref())?;
    home_board::home_board(&state.pool, &state.app_manager, layout)
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// POST /api/v1/widgets/arrange - Arrange the Home board of one layout class.
/// Writes one `HomeBoardArranged` and answers the board it gives.
async fn arrange_home_board(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArrangeBody>,
) -> Result<Json<HomeBoard>, ApiError> {
    let actor = super::actor::user_actor(&headers, None);
    let layout = layout_class(Some(&body.layout))?;
    let home = home_board::check_arrangement(&state.pool, &state.app_manager, &body.tiles)
        .await
        .map_err(refusal)?;
    home_board::emit_arrangement(&state.engine.event_bus, home, layout, body.tiles, actor)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    home_board::home_board(&state.pool, &state.app_manager, layout)
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// Routes for the `/widgets` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/widgets", get(list_reusable_widgets))
        .route("/widgets/thread", get(thread_widgets))
        .route("/widgets/pin", post(pin_widget))
        .route("/widgets/unpin", post(unpin_widget))
        .route("/widgets/make-reusable", post(make_reusable))
        .route("/widgets/stop-reusing", post(stop_reusing))
        .route("/widgets/arrange", post(arrange_home_board))
        .route("/home-board", get(get_home_board))
}
