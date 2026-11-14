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
use crate::engine::widget_fit::FitBoxReport;
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

/// A widget fit check: the widget, and the thread its request rides on.
#[derive(Debug, Deserialize)]
pub(super) struct FitBody {
    app_id: String,
    thread_id: Option<String>,
    renderer_session_id: Option<String>,
    device_id: Option<String>,
}

/// Register only under this thread's verified subprocess origin. The returned
/// secret belongs in launcher memory and an exact host-only report channel.
#[derive(Deserialize)]
struct FitSessionBody {
    thread_id: uuid::Uuid,
    app_id: String,
    device_id: String,
}

const FIT_RENDERER_HEADER: &str = "x-lucidos-fit-renderer";

fn require_fit_source(headers: &HeaderMap, thread: uuid::Uuid) -> Result<(), ApiError> {
    match super::actor::subprocess_origin(headers) {
        super::actor::SubprocessOrigin::Subprocess {
            source_thread_id: Some(source),
            ..
        } if source == thread => Ok(()),
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "A verified origin for this thread is required",
        )
        .with_reason("fit-renderer-origin-required")),
    }
}

async fn register_fit_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FitSessionBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_fit_source(&headers, body.thread_id)?;
    let app = state
        .app_manager
        .get_app(&body.app_id)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if !app.is_widget() {
        return Err(ApiError::bad_request("Fit renderers require a widget"));
    }
    let grant = state
        .engine
        .widget_fit_sessions
        .register(
            Some(body.thread_id),
            crate::engine::widget_fit_sessions::FitScope {
                thread: body.thread_id,
                app: body.app_id,
                device: body.device_id,
            },
        )
        .map_err(|reason| ApiError::new(StatusCode::CONFLICT, reason))?;
    Ok(Json(
        serde_json::json!({ "renderer_session_id": grant.handle, "credential": grant.credential, "expires_in_seconds": crate::engine::widget_fit_sessions::SESSION_TTL.as_secs() }),
    ))
}

/// A page's answer to a widget fit check: what each box measured.
#[derive(Debug, Deserialize)]
pub(super) struct FitReportBody {
    request_id: String,
    device_id: Option<String>,
    boxes: Vec<FitBoxReport>,
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

/// POST /api/v1/widgets/fit - Render a widget at every size it declares in an
/// open Lucidos window, and answer `{"note"}`: what each box cuts off. The
/// caller's own window renders it when it is visible.
async fn check_fit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FitBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caller = super::actor::user_actor(&headers, None);
    let thread_id = required_thread(body.thread_id.as_deref())?;
    if let Some(handle) = body.renderer_session_id.as_deref() {
        require_fit_source(&headers, thread_id)?;
        let device = body
            .device_id
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("device_id is required for a renderer"))?;
        let actor = super::actor::require_user_actor(&headers, &state.pool, None).await?;
        let note = state
            .engine
            .check_widget_fit_with_renderer(&body.app_id, thread_id, handle, device, actor)
            .await;
        return Ok(Json(serde_json::json!({ "note": note })));
    }
    let preferred = match &caller {
        Some(
            crate::engine::thread_events::MessageOrigin::Device { device_id }
            | crate::engine::thread_events::MessageOrigin::SessionDevice { device_id },
        ) => Some(device_id.as_str()),
        _ => None,
    };
    let note = state
        .engine
        .check_widget_fit(&body.app_id, thread_id, preferred)
        .await;
    Ok(Json(serde_json::json!({ "note": note })))
}

/// POST /api/v1/widgets/fit-report - A page answers a widget fit check. Only
/// the device the check names may answer; any other is refused and the check
/// stays open for that one.
async fn submit_fit_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FitReportBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use crate::engine::pending_answers::{resolve_answer, AnswerRefusal};
    if let Some(credential) = headers.get(FIT_RENDERER_HEADER) {
        // Keep the ordinary door credential and attribution gate. This fit
        // secret is never admitted as a general actor or owner credential.
        super::actor::require_user_actor(&headers, &state.pool, body.device_id.as_deref()).await?;
        let credential = credential
            .to_str()
            .map_err(|_| ApiError::bad_request("Invalid fit renderer credential"))?;
        state
            .engine
            .widget_fit_sessions
            .answer(
                credential,
                &body.request_id,
                body.device_id.as_deref().unwrap_or(""),
                body.boxes,
            )
            .map_err(|reason| {
                ApiError::new(StatusCode::CONFLICT, reason)
                    .with_reason("fit-check-for-another-renderer")
            })?;
        return Ok(Json(serde_json::json!({ "ok": true })));
    }
    let claimed = body.device_id.as_deref();
    let answering = match super::actor::require_user_actor(&headers, &state.pool, claimed).await? {
        crate::engine::thread_events::MessageOrigin::Device { device_id }
        | crate::engine::thread_events::MessageOrigin::SessionDevice { device_id } => {
            Some(device_id)
        }
        _ => None,
    };
    match resolve_answer(
        &state.engine.pending_widget_fits,
        &body.request_id,
        answering.as_deref(),
        body.boxes,
    ) {
        Ok(()) => Ok(Json(serde_json::json!({ "ok": true }))),
        Err(AnswerRefusal::Unknown) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!(
                "No widget fit check is waiting under request {}",
                body.request_id
            ),
        )),
        Err(AnswerRefusal::OtherDevice) => Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("Widget fit check {} is for another device", body.request_id),
        )
        .with_reason("fit-check-for-another-device")),
    }
}

/// Routes for the `/widgets` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/widgets", get(list_reusable_widgets))
        .route("/widgets/size", post(size_cards))
        .route("/widgets/fit", post(check_fit))
        .route("/widgets/fit-session", post(register_fit_session))
        .route("/widgets/fit-report", post(submit_fit_report))
        .route("/widgets/make-reusable", post(make_reusable))
        .route("/widgets/stop-reusing", post(stop_reusing))
        .route("/widgets/dashboard-add", post(add_to_dashboard))
        .route("/widgets/dashboard-remove", post(remove_from_dashboard))
        .route("/widgets/arrange", post(arrange_dashboard))
        .route("/dashboard", get(get_dashboard))
}

#[cfg(test)]
mod fit_source_tests {
    use super::*;
    use crate::api::actor::{
        init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
    };

    #[test]
    fn only_a_valid_origin_for_the_requested_thread_can_register_or_select() {
        init_agent_origin_secret("fit-origin-test-secret".into());
        let thread = uuid::Uuid::new_v4();
        let mut headers = HeaderMap::new();
        assert!(require_fit_source(&headers, thread).is_err());
        headers.insert("x-lucidos-device-id", "registered-device".parse().unwrap());
        headers.insert("x-lucidos-local-token", "local-token".parse().unwrap());
        let forged_stamp = format!(
            "v1.{}.{}",
            lucidos_gateway_stamp::now_unix(),
            "0".repeat(64)
        );
        headers.insert(
            lucidos_gateway_stamp::HEADER_GATEWAY_STAMP,
            forged_stamp.parse().unwrap(),
        );
        assert!(require_fit_source(&headers, thread).is_err());
        headers.insert(
            "x-lucidos-agent-origin-token",
            "forged-token".parse().unwrap(),
        );
        assert!(require_fit_source(&headers, thread).is_err());
        let token = mint_agent_origin_token(Some(uuid::Uuid::new_v4()), 0, None).unwrap();
        headers.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
        assert!(require_fit_source(&headers, thread).is_err());
        let token = mint_agent_origin_token(Some(thread), 0, None).unwrap();
        headers.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
        assert!(require_fit_source(&headers, thread).is_ok());
    }

    #[tokio::test]
    async fn a_live_fit_credential_never_authorizes_an_owner_operation() {
        use crate::api::owner_door::OnUnattested;
        use crate::engine::widget_fit_sessions::{FitScope, FitSessions};

        let (pool, db) = crate::test_support::setup_test_db().await;
        init_agent_origin_secret("fit-origin-test-secret".into());
        let thread = uuid::Uuid::new_v4();
        let sessions = FitSessions::default();
        let scope = FitScope {
            thread,
            app: "example-widget".into(),
            device: "fit-renderer".into(),
        };
        let grant = sessions.register(Some(thread), scope.clone()).unwrap();
        let wait = sessions.select(&grant.handle, &scope, "request").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(FIT_RENDERER_HEADER, grant.credential.parse().unwrap());
        let refusal =
            super::super::actor::require_owner_device(&headers, &pool, OnUnattested::Refuse)
                .await
                .expect_err("a fit credential provides no general identity");
        assert_eq!(refusal.status, StatusCode::UNAUTHORIZED);

        let token = mint_agent_origin_token(Some(thread), 0, None).unwrap();
        headers.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
        let refusal =
            super::super::actor::require_owner_device(&headers, &pool, OnUnattested::Refuse)
                .await
                .expect_err("an identified renderer agent is still not the owner");
        assert_eq!(refusal.status, StatusCode::FORBIDDEN);
        sessions
            .answer(&grant.credential, "request", &scope.device, vec![])
            .unwrap();
        assert!(wait.await.unwrap().is_empty());
        crate::test_support::teardown_test_db(&db).await;
    }
}
