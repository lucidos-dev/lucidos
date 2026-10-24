//! `GET /api/v1/intelligence-ladder`: one *ladder surface*'s effective
//! *intelligence ladder*, and where a coding-agent thread stands on it (ADR
//! 0500, ADR 0525). `/coding-agent/intelligence-ladder` is its old name.
//! `POST /api/v1/intelligence-ladder/copy-from-chat` is Copy from Lucidos Agent.
//! `POST /api/v1/intelligence-ladder/sort` is Sort by intelligence.

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::error::ApiError;
use super::AppState;
use crate::runtime::intelligence_ladder::{
    self, IntelligenceLadder, IntelligenceRung, LadderSurface,
};
use crate::runtime::CodingAgent;

#[derive(Debug, Deserialize)]
pub(super) struct LadderQuery {
    #[serde(default)]
    surface: Option<LadderSurface>,
    /// The old name for a coding-agent `surface`.
    #[serde(default)]
    coding_agent: Option<CodingAgent>,
    /// A coding-agent thread whose backend and current rung to report. Its
    /// backend wins over `surface`.
    #[serde(default)]
    thread_id: Option<String>,
}

/// The implementer a thread's sessions last recorded.
#[derive(Debug, Serialize)]
struct CurrentIntelligenceRung {
    model: String,
    effort: String,
    /// Index into `rungs`, whose entry says whether the pair is on. `None`
    /// when no served rung names the pair.
    position: Option<usize>,
}

#[derive(Debug, Serialize)]
struct LadderResponse {
    #[serde(flatten)]
    ladder: IntelligenceLadder,
    /// Absent without a thread, or when it recorded no model and effort.
    current: Option<CurrentIntelligenceRung>,
}

async fn intelligence_ladder_route(
    State(state): State<AppState>,
    Query(query): Query<LadderQuery>,
) -> Result<Json<LadderResponse>, ApiError> {
    let thread_id = super::parse_optional_uuid(query.thread_id.as_deref())
        .map_err(|_| ApiError::bad_request("thread_id is not a uuid"))?;
    let surface = match thread_id {
        Some(tid) => state.engine.thread_coding_agent(tid).await.into(),
        None => query
            .surface
            .or(query.coding_agent.map(LadderSurface::from))
            .unwrap_or(LadderSurface::ClaudeCode),
    };
    let ladder =
        intelligence_ladder::load(&state.pool, &state.engine.model_routes(), surface).await;
    let current = match thread_id {
        Some(tid) => match state.engine.cc_thread_settings(tid).await {
            (Some(model), Some(effort)) => Some(CurrentIntelligenceRung {
                position: ladder.position_of(&IntelligenceRung::new(&model, &effort)),
                model,
                effort,
            }),
            _ => None,
        },
        None => None,
    };
    Ok(Json(LadderResponse { ladder, current }))
}

/// Whether a request comes from an agent rather than a person: a Lucidos
/// subprocess, or an origin that declares agent mode. Only an agent's pick
/// is held to the ladder.
pub(crate) fn is_agent_caller(
    headers: &HeaderMap,
    origin: Option<&crate::engine::thread_events::MessageOrigin>,
) -> bool {
    matches!(
        crate::api::actor::subprocess_origin(headers),
        crate::api::actor::SubprocessOrigin::Subprocess { .. }
    ) || origin.is_some_and(|o| o.mode() == crate::engine::thread_events::ActorMode::Agent)
}

/// [`intelligence_ladder::check_agent_pick`] as an HTTP refusal.
pub(crate) async fn refuse_off_ladder(
    state: &AppState,
    surface: LadderSurface,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<(), ApiError> {
    intelligence_ladder::check_agent_pick(
        &state.pool,
        &state.engine.model_routes(),
        surface,
        model,
        effort,
    )
    .await
    .map_err(|e| ApiError::bad_request(e).with_reason(intelligence_ladder::OFF_LADDER_REASON))
}

#[derive(Debug, Deserialize)]
pub(super) struct CopyFromChatRequest {
    /// The coding-agent surface whose override the copy writes.
    surface: LadderSurface,
}

#[derive(Debug, Serialize)]
struct IntelligenceRungsNowOn {
    /// The rungs now on, least to most intelligent.
    #[serde(rename = "rungs")]
    intelligence_rungs: Vec<IntelligenceRung>,
}

/// `POST /api/v1/intelligence-ladder/copy-from-chat`: Copy from Lucidos Agent.
async fn copy_from_chat_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CopyFromChatRequest>,
) -> Result<Json<IntelligenceRungsNowOn>, ApiError> {
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let intelligence_rungs = crate::runtime::chat_ladder::copy_chat_ladder_to(
        &state.pool,
        &state.engine.event_bus,
        &state.engine.model_routes(),
        request.surface,
        Some(actor),
    )
    .await
    .map_err(ApiError::bad_request)?;
    Ok(Json(IntelligenceRungsNowOn { intelligence_rungs }))
}

#[derive(Debug, Deserialize)]
pub(super) struct SortRequest {
    /// The surface whose rungs to sort.
    surface: LadderSurface,
}

/// `POST /api/v1/intelligence-ladder/sort`: Sort by intelligence.
async fn sort_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SortRequest>,
) -> Result<Json<IntelligenceRungsNowOn>, ApiError> {
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let intelligence_rungs = crate::runtime::chat_ladder::sort_ladder(
        &state.pool,
        &state.engine.event_bus,
        &state.engine.model_routes(),
        request.surface,
        Some(actor),
    )
    .await
    .map_err(ApiError::bad_request)?;
    Ok(Json(IntelligenceRungsNowOn { intelligence_rungs }))
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/intelligence-ladder", get(intelligence_ladder_route))
        .route(
            "/intelligence-ladder/copy-from-chat",
            post(copy_from_chat_route),
        )
        .route("/intelligence-ladder/sort", post(sort_route))
        .route(
            "/coding-agent/intelligence-ladder",
            get(intelligence_ladder_route),
        )
}
