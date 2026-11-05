//! `/api/v1/widgets`: reusable widgets, a thread's widget card sizes and the
//! two dashboards (ADRs 0402, 0496).
//!
//! The UI's widget menu and the `lucidos widgets` CLI both land here. The
//! agent's grouped `widgets` tool runs the same checks in-process.

use super::error::ApiError;
use super::*;

use crate::core::apps::WidgetSize;
use crate::core::App;
use crate::engine::dashboard::{self, DashboardChange, DashboardView};
use crate::engine::thread_events::{ArrangedTile, Dashboard};
use crate::engine::widgets::{self, WidgetCheckFailed, WidgetInstance, WidgetInstanceArgs};

/// A card size body: the thread, the widget instance and its size.
#[derive(Debug, Deserialize)]
pub(super) struct CardSizeBody {
    thread_id: Option<String>,
    size: WidgetSize,
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

/// POST /api/v1/widgets/size - The size of a widget instance's cards in a
/// thread. Writes one event, no file.
async fn size_cards(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CardSizeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::user_actor(&headers, None);
    let thread_id = required_thread(body.thread_id.as_deref())?;
    let instance = WidgetInstance::try_from(body.instance).map_err(ApiError::bad_request)?;
    widgets::check_card_size(
        &state.pool,
        &state.app_manager,
        &instance,
        body.size,
        thread_id,
    )
    .await
    .map_err(refusal)?;
    widgets::emit_card_size(
        &state.engine.event_bus,
        instance,
        body.size,
        thread_id,
        actor,
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
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
pub(super) struct DashboardQuery {
    dashboard: Option<String>,
}

/// An add or remove body: the dashboard, and the widget instance. Only an
/// add reads `size`.
#[derive(Debug, Deserialize)]
pub(super) struct DashboardChangeBody {
    dashboard: Option<String>,
    #[serde(default)]
    size: Option<WidgetSize>,
    #[serde(flatten)]
    instance: WidgetInstanceArgs,
}

#[derive(Debug, Deserialize)]
pub(super) struct ArrangeBody {
    dashboard: Option<String>,
    tiles: Vec<ArrangedTile>,
}

/// The named dashboard, else the one the calling device shows.
async fn named_dashboard(
    state: &AppState,
    headers: &HeaderMap,
    raw: Option<&str>,
) -> Result<Dashboard, ApiError> {
    let device = super::actor::claimed_device_id(None, headers);
    dashboard::resolve_dashboard(&state.pool, raw, device)
        .await
        .map_err(ApiError::bad_request)
}

async fn read_dashboard(
    state: &AppState,
    which: Dashboard,
) -> Result<Json<DashboardView>, ApiError> {
    dashboard::dashboard(&state.pool, &state.app_manager, which)
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// GET /api/v1/dashboard?dashboard= - One dashboard's tiles.
async fn get_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DashboardQuery>,
) -> Result<Json<DashboardView>, ApiError> {
    let which = named_dashboard(&state, &headers, query.dashboard.as_deref()).await?;
    read_dashboard(&state, which).await
}

async fn change_dashboard(
    state: &AppState,
    headers: &HeaderMap,
    change: DashboardChange,
    body: DashboardChangeBody,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = super::actor::user_actor(headers, None);
    let which = named_dashboard(state, headers, body.dashboard.as_deref()).await?;
    let instance = WidgetInstance::try_from(body.instance).map_err(ApiError::bad_request)?;
    let plan = dashboard::check_dashboard_change(
        &state.pool,
        &state.app_manager,
        change,
        which,
        &instance,
    )
    .await
    .map_err(refusal)?;
    let made_reusable = dashboard::apply_dashboard_change(
        &state.app_manager,
        &state.engine.event_bus,
        plan,
        change,
        which,
        instance,
        actor,
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "dashboard": which,
        "made_reusable": made_reusable,
    })))
}

/// POST /api/v1/widgets/dashboard-add - "Add to dashboard". A widget that
/// belongs to a thread becomes reusable first.
async fn add_to_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DashboardChangeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let change = DashboardChange::Add { size: body.size };
    change_dashboard(&state, &headers, change, body).await
}

/// POST /api/v1/widgets/dashboard-remove - "Remove from dashboard".
async fn remove_from_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DashboardChangeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    change_dashboard(&state, &headers, DashboardChange::Remove, body).await
}

/// POST /api/v1/widgets/arrange - Arrange one dashboard. Writes one
/// `DashboardArranged` and answers the dashboard it gives.
async fn arrange_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArrangeBody>,
) -> Result<Json<DashboardView>, ApiError> {
    let actor = super::actor::user_actor(&headers, None);
    let which = named_dashboard(&state, &headers, body.dashboard.as_deref()).await?;
    dashboard::check_arrangement(&state.pool, &state.app_manager, which, &body.tiles)
        .await
        .map_err(refusal)?;
    dashboard::emit_arrangement(&state.engine.event_bus, which, body.tiles, actor)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    read_dashboard(&state, which).await
}

/// Routes for the `/widgets` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/widgets", get(list_reusable_widgets))
        .route("/widgets/size", post(size_cards))
        .route("/widgets/make-reusable", post(make_reusable))
        .route("/widgets/stop-reusing", post(stop_reusing))
        .route("/widgets/dashboard-add", post(add_to_dashboard))
        .route("/widgets/dashboard-remove", post(remove_from_dashboard))
        .route("/widgets/arrange", post(arrange_dashboard))
        .route("/dashboard", get(get_dashboard))
}
