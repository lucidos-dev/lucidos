use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::actor::{owner_attestation, require_user_actor, OwnerAttestation};
use super::error::ApiError;
use super::owner_door::unattested;
use super::permission_requests::{
    calling_agent_thread, ladder_ask, permission_required, prepare, restore_spent, spend_live_once,
    LADDER_BREADTH,
};
use super::{ApiResult, AppState};
use crate::engine::permission::{thread_user, Action};
use crate::engine::thread_events::MessageOrigin;
use crate::runtime::chat_ladder;
use crate::runtime::intelligence_ladder::{
    self, IntelligenceLadder, IntelligenceRung, LadderSurface,
};
use crate::runtime::CodingAgent;

#[derive(Debug, Deserialize)]
pub(super) struct LadderQuery {
    #[serde(default)]
    surface: Option<LadderSurface>,
    #[serde(default)]
    coding_agent: Option<CodingAgent>,
    #[serde(default)]
    thread_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct CurrentIntelligenceRung {
    model: String,
    effort: String,
    position: Option<usize>,
}

#[derive(Debug, Serialize)]
pub(super) struct LadderResponse {
    #[serde(flatten)]
    ladder: IntelligenceLadder,
    current: Option<CurrentIntelligenceRung>,
    other_backend_ladders: Vec<IntelligenceLadder>,
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
    Ok(Json(read_ladder(&state, surface, thread_id).await))
}

pub(super) async fn read_ladder(
    state: &AppState,
    surface: LadderSurface,
    thread_id: Option<uuid::Uuid>,
) -> LadderResponse {
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
    let other_backend_ladders = if thread_id.is_some() {
        intelligence_ladder::other_backend_ladders(
            &state.pool,
            &state.engine.model_routes(),
            &ladder,
        )
        .await
    } else {
        Vec::new()
    };
    LadderResponse {
        ladder,
        current,
        other_backend_ladders,
    }
}

pub(crate) fn is_agent_caller(
    headers: &HeaderMap,
    origin: Option<&crate::engine::thread_events::MessageOrigin>,
) -> bool {
    matches!(
        crate::core::subprocess_origin::subprocess_origin(headers),
        crate::core::subprocess_origin::SubprocessOrigin::Subprocess { .. }
    ) || origin.is_some_and(|o| o.mode() == crate::engine::thread_events::ActorMode::Agent)
}

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

pub(crate) enum LadderEditGate {
    Act {
        actor: MessageOrigin,
        spent: Option<SpentOnce>,
    },
    NeedsPermission(uuid::Uuid),
}

pub(crate) struct SpentOnce {
    thread: uuid::Uuid,
    permission: uuid::Uuid,
}

impl SpentOnce {
    pub(crate) async fn restore(self, state: &AppState) -> Result<(), ApiError> {
        restore_spent(&state.engine.event_bus, self.thread, self.permission).await
    }
}

pub(crate) async fn ladder_edit_gate(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<LadderEditGate, ApiError> {
    let actor = require_user_actor(headers, &state.pool, None).await?;
    match owner_attestation(headers) {
        OwnerAttestation::Owner => Ok(LadderEditGate::Act { actor, spent: None }),
        OwnerAttestation::Agent => {
            let caller = calling_agent_thread(headers)?;
            let user = thread_user(caller);
            let bus = &state.engine.event_bus;
            Ok(
                match spend_live_once(bus, &user, Action::EditIntelligenceLadder, LADDER_BREADTH)
                    .await
                {
                    Some(once) => LadderEditGate::Act {
                        actor,
                        spent: once.map(|permission| SpentOnce {
                            thread: caller,
                            permission,
                        }),
                    },
                    None => LadderEditGate::NeedsPermission(caller),
                },
            )
        }
        OwnerAttestation::Unattested => Err(unattested()),
    }
}

async fn gated_edit<T, F, Fut>(
    state: &AppState,
    headers: &HeaderMap,
    reason: &str,
    edit: F,
) -> Result<Response, ApiError>
where
    T: Serialize,
    F: FnOnce(MessageOrigin) -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    match ladder_edit_gate(state, headers).await? {
        LadderEditGate::Act { actor, spent } => match edit(actor).await {
            Ok(done) => Ok(Json(done).into_response()),
            Err(refusal) => {
                if let Some(spent) = spent {
                    spent.restore(state).await?;
                }
                Err(ApiError::bad_request(refusal))
            }
        },
        LadderEditGate::NeedsPermission(caller) => ask_to_edit_ladder(state, caller, reason).await,
    }
}

pub(crate) async fn ask_to_edit_ladder(
    state: &AppState,
    caller: uuid::Uuid,
    reason: &str,
) -> Result<Response, ApiError> {
    let request_id = prepare(&state.engine.event_bus, caller, ladder_ask(reason)?).await?;
    Ok(permission_required(&request_id))
}

#[derive(Debug, Serialize)]
struct IntelligenceRungsNowOn {
    #[serde(rename = "rungs")]
    intelligence_rungs: Vec<IntelligenceRung>,
}

impl From<Vec<IntelligenceRung>> for IntelligenceRungsNowOn {
    fn from(intelligence_rungs: Vec<IntelligenceRung>) -> Self {
        Self { intelligence_rungs }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct CopyFromChatRequest {
    surface: LadderSurface,
    #[serde(default)]
    reason: String,
}

async fn copy_from_chat_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CopyFromChatRequest>,
) -> Result<Response, ApiError> {
    gated_edit(&state, &headers, &request.reason, |actor| async {
        chat_ladder::copy_chat_ladder_to(
            &state.pool,
            &state.engine.event_bus,
            &state.engine.model_routes(),
            request.surface,
            Some(actor),
        )
        .await
        .map(IntelligenceRungsNowOn::from)
    })
    .await
}

#[derive(Debug, Deserialize)]
pub(super) struct SortRequest {
    surface: LadderSurface,
    #[serde(default)]
    reason: String,
}

async fn sort_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SortRequest>,
) -> Result<Response, ApiError> {
    gated_edit(&state, &headers, &request.reason, |actor| async {
        chat_ladder::sort_ladder(
            &state.pool,
            &state.engine.event_bus,
            &state.engine.model_routes(),
            request.surface,
            Some(actor),
        )
        .await
        .map(IntelligenceRungsNowOn::from)
    })
    .await
}

#[derive(Debug, Deserialize)]
struct SetDefaultRequest {
    surface: LadderSurface,
    #[serde(flatten)]
    intelligence_rung: IntelligenceRung,
    #[serde(default)]
    reason: String,
}

async fn set_default_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SetDefaultRequest>,
) -> Result<Response, ApiError> {
    gated_edit(&state, &headers, &request.reason, |actor| async {
        chat_ladder::set_default_intelligence_rung(
            &state.pool,
            &state.engine.event_bus,
            &state.engine.model_routes(),
            request.surface,
            &request.intelligence_rung,
            actor,
        )
        .await
        .map(|()| ApiResult::ok().0)
    })
    .await
}

#[derive(Debug, Deserialize)]
struct SetIntelligenceRungsRequest {
    surface: LadderSurface,
    #[serde(rename = "rungs")]
    intelligence_rungs: Vec<IntelligenceRung>,
    #[serde(default)]
    reason: String,
}

async fn set_intelligence_rungs_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SetIntelligenceRungsRequest>,
) -> Result<Response, ApiError> {
    gated_edit(&state, &headers, &request.reason, |actor| async {
        chat_ladder::set_intelligence_rungs(
            &state.pool,
            &state.engine.event_bus,
            &state.engine.model_routes(),
            request.surface,
            &request.intelligence_rungs,
            Some(actor),
        )
        .await
        .map(IntelligenceRungsNowOn::from)
    })
    .await
}

#[derive(Debug, Deserialize)]
struct RemoveModelRequest {
    surface: LadderSurface,
    model: String,
    #[serde(default)]
    reason: String,
}

async fn remove_model_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RemoveModelRequest>,
) -> Result<Response, ApiError> {
    gated_edit(&state, &headers, &request.reason, |actor| async {
        chat_ladder::remove_model_from_ladder(
            &state.pool,
            &state.engine.event_bus,
            &state.engine.model_routes(),
            request.surface,
            &request.model,
            Some(actor),
        )
        .await
        .map(IntelligenceRungsNowOn::from)
    })
    .await
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/intelligence-ladder", get(intelligence_ladder_route))
        .route(
            "/intelligence-ladder/copy-from-chat",
            post(copy_from_chat_route),
        )
        .route("/intelligence-ladder/sort", post(sort_route))
        .route("/intelligence-ladder/default", post(set_default_route))
        .route(
            "/intelligence-ladder/rungs",
            post(set_intelligence_rungs_route),
        )
        .route(
            "/intelligence-ladder/remove-model",
            post(remove_model_route),
        )
        .route(
            "/coding-agent/intelligence-ladder",
            get(intelligence_ladder_route),
        )
}

#[cfg(test)]
#[path = "intelligence_ladder_tests.rs"]
mod tests;
