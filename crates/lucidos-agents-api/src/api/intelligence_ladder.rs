//! `GET /api/v1/intelligence-ladder`: one *ladder surface*'s effective
//! *intelligence ladder*, and where a coding-agent thread stands on it (ADR
//! 0500, ADR 0525). `/coding-agent/intelligence-ladder` is its old name.
//! `POST /api/v1/intelligence-ladder/copy-from-chat` is Copy from Lucidos Agent.
//! `POST /api/v1/intelligence-ladder/sort` is Sort by intelligence.
//! `POST /api/v1/intelligence-ladder/default`, `/rungs` and `/remove-model`
//! set the default rung, the rungs that are on, and take a model off.
//!
//! Every edit route passes the owner. An agent edits only by spending the
//! owner's `edit-intelligence-ladder` permission (ADR 0500).

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
pub(super) struct LadderResponse {
    #[serde(flatten)]
    ladder: IntelligenceLadder,
    /// Absent without a thread, or when it recorded no model and effort.
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

/// The ladder of `surface`. A thread adds its current rung and the ladders
/// of the other backends.
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

/// Whether a request comes from an agent rather than a person: a Lucidos
/// subprocess, or an origin that declares agent mode. Only an agent's pick
/// is held to the ladder.
pub(crate) fn is_agent_caller(
    headers: &HeaderMap,
    origin: Option<&crate::engine::thread_events::MessageOrigin>,
) -> bool {
    matches!(
        crate::core::subprocess_origin::subprocess_origin(headers),
        crate::core::subprocess_origin::SubprocessOrigin::Subprocess { .. }
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

/// Who may edit a ladder: the owner, or an agent that spends the owner's
/// `edit-intelligence-ladder` permission (ADR 0750).
pub(crate) enum LadderEditGate {
    Act {
        actor: MessageOrigin,
        /// An agent's spent once grant, which a refused edit gives back.
        spent: Option<SpentOnce>,
    },
    /// An agent in this thread holds no live permission.
    NeedsPermission(uuid::Uuid),
}

/// A once grant an agent's thread spent on a ladder edit.
pub(crate) struct SpentOnce {
    thread: uuid::Uuid,
    permission: uuid::Uuid,
}

impl SpentOnce {
    /// Give the grant back: the edit it paid for changed nothing.
    pub(crate) async fn restore(self, state: &AppState) -> Result<(), ApiError> {
        restore_spent(&state.engine.event_bus, self.thread, self.permission).await
    }
}

/// Gate one ladder edit. An unattested caller waits on the owner's card.
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

/// Run one ladder edit behind [`ladder_edit_gate`], answering its result as
/// JSON. An agent with no permission gets a prepared card quoting `reason`,
/// and runs its command again once the owner allows it. A refused edit
/// changes nothing, so it gives back the once grant it spent.
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

/// Prepare the owner's ladder card quoting `reason`, and answer the request
/// id the caller asks with. Nothing changes until the owner allows it.
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
    /// The rungs now on, least to most intelligent.
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
    /// The coding-agent surface whose override the copy writes.
    surface: LadderSurface,
    /// Why an agent edits. The owner's card quotes it.
    #[serde(default)]
    reason: String,
}

/// `POST /api/v1/intelligence-ladder/copy-from-chat`: Copy from Lucidos Agent.
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
    /// The surface whose rungs to sort.
    surface: LadderSurface,
    #[serde(default)]
    reason: String,
}

/// `POST /api/v1/intelligence-ladder/sort`: Sort by intelligence.
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

/// `POST /api/v1/intelligence-ladder/default`: set the default rung.
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
    /// Exactly the rungs to turn on, least to most intelligent.
    #[serde(rename = "rungs")]
    intelligence_rungs: Vec<IntelligenceRung>,
    #[serde(default)]
    reason: String,
}

/// `POST /api/v1/intelligence-ladder/rungs`: turn on exactly these rungs.
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

/// `POST /api/v1/intelligence-ladder/remove-model`: turn off every rung that
/// runs one model.
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
