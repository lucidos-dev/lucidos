//! *Successor sessions*: a coding-agent thread's context size, and the handoff
//! to a fresh session in the same thread.
//!
//! - `GET /api/v1/threads/:thread_id/context`: the size and what it calls for.
//! - `POST /api/v1/threads/:thread_id/handoff-checkpoints`: record a *handoff
//!   checkpoint* run and answer with its *handoff estimate* (ADR 0505).
//! - `POST /api/v1/threads/:thread_id/successor`: the agent's handoff, after it
//!   wrote its *handoff narrative*.
//! - `POST /api/v1/threads/:thread_id/successor-offer/continue`: the user chose
//!   *Continue with context* on the Successor card.
//! - `POST /api/v1/threads/:thread_id/successor-offer/decline`: the user chose
//!   *Stay here* on the Successor card.
//!
//! A handoff creates no thread, no branch and no change. It records
//! `SessionHandedOff`, and the thread's next session starts fresh from its
//! opening once the current run lets go of the thread.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::actor::SubprocessOrigin;
use crate::api::chat::chat_submit;
use crate::api::error::ApiError;
use crate::api::AppState;
use crate::api::ChatRequest;
use crate::core::store::thread_link;
use crate::engine::event_bus::BusEvent;
use crate::engine::handoff_estimate::{
    cache_warmth, card_verdict, record_checkpoint, CacheWarmth, CardVerdict, CheckpointRun,
    FloorRungs, HandoffVerdict,
};
use crate::engine::successor::{
    handoff_brief, missing_narrative_sections, narrative_opening, pending_successor_request,
    resolve_implementer, BriefCause, HandoffRequest, Implementer, ImplementerRequest,
    ThreadContext, HANDOFF_REQUEST,
};
use crate::engine::thread_events::{EventMeta, HandoffOpening, MessageOrigin, ThreadEvent};
use crate::runtime::intelligence_ladder::{self, IntelligenceRung};

fn parse_thread_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))
}

/// The context reading, plus the implementer a coding-agent thread runs on,
/// which a plan card names beside the one it recommends. A checkpoint run
/// also carries the *handoff estimate*'s verdict. The plain reading carries
/// the Successor card's verdict.
#[derive(Debug, Serialize)]
pub(in crate::api) struct ThreadContextResponse {
    #[serde(flatten)]
    context: ThreadContext,
    implementer: Option<Implementer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    card: Option<CardVerdict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    handoff: Option<HandoffVerdict>,
}

/// A checkpoint run as a flow names it: a name, and optionally the progress
/// as `<done>/<total>`.
#[derive(Debug, Default, Deserialize)]
pub(in crate::api) struct CheckpointFields {
    #[serde(default)]
    checkpoint: Option<String>,
    #[serde(default)]
    progress: Option<String>,
}

impl CheckpointFields {
    fn run(&self) -> Result<Option<CheckpointRun>, ApiError> {
        match (self.checkpoint.as_deref(), self.progress.as_deref()) {
            (None, None) => Ok(None),
            (None, Some(_)) => Err(ApiError::bad_request("A `progress` needs a `checkpoint`.")),
            (Some(name), progress) => CheckpointRun::parse(name, progress)
                .map(Some)
                .map_err(ApiError::bad_request),
        }
    }
}

/// The thread's columns the context reading needs, or a 404.
async fn context_row(
    state: &AppState,
    thread_id: Uuid,
) -> Result<(bool, Option<String>), ApiError> {
    sqlx::query_as(
        "SELECT is_coding_agent, coding_agent FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::db)?
    .ok_or_else(|| ApiError::not_found(format!("Thread {thread_id} does not exist")))
}

pub(in crate::api) async fn thread_context(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Json<ThreadContextResponse>, ApiError> {
    let thread_id = parse_thread_id(&thread_id)?;
    let (is_coding_agent, coding_agent) = context_row(&state, thread_id).await?;
    let context = crate::engine::successor::thread_context(&state.pool, thread_id)
        .await
        .map_err(ApiError::db)?;
    let implementer = if is_coding_agent {
        Some(thread_implementer(&state, thread_id, coding_agent.as_deref()).await?)
    } else {
        None
    };
    let card = if is_coding_agent {
        card_verdict(&state.pool, thread_id, &context)
            .await
            .map_err(ApiError::db)?
    } else {
        None
    };
    Ok(Json(ThreadContextResponse {
        context,
        implementer,
        card,
        handoff: None,
    }))
}

/// Record *Stay here* on a coding-agent thread's Successor card, at the
/// context size it holds now. The card stays hidden until the thread grows.
pub(in crate::api) async fn decline_successor_offer(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let thread_id = parse_thread_id(&thread_id)?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let (is_coding_agent, _) = context_row(&state, thread_id).await?;
    if !is_coding_agent {
        return Err(refusal(
            StatusCode::BAD_REQUEST,
            "not-coding-agent",
            "Only a coding-agent thread has a Successor card.",
        ));
    }
    // A tap on a card the thread outgrew (a turn started) records nothing.
    hand_off_candidate(&state, thread_id, true).await?;
    let input_tokens = crate::engine::successor::thread_context(&state.pool, thread_id)
        .await
        .map_err(ApiError::db)?
        .input_tokens
        .ok_or_else(|| {
            refusal(
                StatusCode::CONFLICT,
                "no-context-reading",
                "The thread has made no model call yet, so it holds no card to decline.",
            )
        })?;
    state
        .engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SuccessorOfferDeclined { input_tokens },
            meta: EventMeta::with_actor(Some(actor)),
        })
        .await
        .map_err(|e| ApiError::internal(format!("Could not record the decline: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// A checkpoint run's body. `offered` marks a `--ask` run. `default_floor`
/// marks a run that declares the *default floor*.
#[derive(Debug, Deserialize)]
pub(in crate::api) struct CheckpointRequest {
    name: String,
    #[serde(default)]
    progress: Option<String>,
    #[serde(default)]
    offered: bool,
    #[serde(default)]
    default_floor: bool,
}

/// The rungs `thread_id`'s surface ladder offers it at a run that declares
/// the default floor.
async fn floor_rungs(state: &AppState, thread_id: Uuid) -> Result<FloorRungs, ApiError> {
    let surface = state.engine.thread_coding_agent(thread_id).await.into();
    let ladder =
        intelligence_ladder::load(&state.pool, &state.engine.model_routes(), surface).await;
    let thread = match state
        .engine
        .cc_thread_runs_on(thread_id)
        .await
        .map_err(ApiError::db)?
    {
        (Some(model), Some(effort)) => Some(IntelligenceRung::new(&model, &effort)),
        _ => None,
    };
    Ok(FloorRungs::on_ladder(&ladder, thread.as_ref()))
}

/// Record a checkpoint run on a coding-agent thread and answer with the
/// context reading and the verdict. A thread records only its own runs.
pub(in crate::api) async fn record_handoff_checkpoint(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CheckpointRequest>,
) -> Result<Json<ThreadContextResponse>, ApiError> {
    let thread_id = parse_thread_id(&thread_id)?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let run = CheckpointRun::parse(&request.name, request.progress.as_deref())
        .map_err(ApiError::bad_request)?;
    let (is_coding_agent, coding_agent) = context_row(&state, thread_id).await?;
    if !is_coding_agent {
        return Err(refusal(
            StatusCode::BAD_REQUEST,
            "not-coding-agent",
            "Only a coding-agent thread runs handoff checkpoints.",
        ));
    }
    if let SubprocessOrigin::Subprocess {
        source_thread_id, ..
    } = crate::api::actor::subprocess_origin(&headers)
    {
        if source_thread_id != Some(thread_id) {
            return Err(refusal(
                StatusCode::FORBIDDEN,
                "not-your-thread",
                "A thread records only its own checkpoints. Run `lucidos successor offer` in that thread.",
            ));
        }
    }
    let floor = match request.default_floor {
        true => Some(floor_rungs(&state, thread_id).await?),
        false => None,
    };
    let verdict = record_checkpoint(
        &state.engine.event_bus,
        &state.pool,
        thread_id,
        &run,
        request.offered,
        floor,
        Some(actor),
    )
    .await
    .map_err(|e| ApiError::internal(format!("Could not record the checkpoint: {e}")))?;
    let context = crate::engine::successor::thread_context(&state.pool, thread_id)
        .await
        .map_err(ApiError::db)?;
    Ok(Json(ThreadContextResponse {
        context,
        implementer: Some(thread_implementer(&state, thread_id, coding_agent.as_deref()).await?),
        card: None,
        handoff: Some(verdict),
    }))
}

/// The implementer a coding-agent thread runs on: its backend, and the model
/// and effort its calls use (`cc_thread_runs_on`). A successor session carries
/// these over.
async fn thread_implementer(
    state: &AppState,
    thread_id: Uuid,
    coding_agent: Option<&str>,
) -> Result<Implementer, ApiError> {
    let (model, reasoning_effort) = state
        .engine
        .cc_thread_runs_on(thread_id)
        .await
        .map_err(ApiError::db)?;
    Ok(Implementer {
        coding_agent: coding_agent
            .map(crate::runtime::CodingAgent::parse)
            .unwrap_or(crate::runtime::CodingAgent::ClaudeCode),
        model,
        reasoning_effort,
    })
}

/// The agent's handoff: a *handoff narrative*, and optionally the successor
/// session's implementer. An agent names none, so the implementer the user
/// picked at the *Successor card* applies (`pending_successor_request`).
#[derive(Debug, Deserialize)]
pub(in crate::api) struct SuccessorRequest {
    #[serde(default)]
    narrative: String,
    #[serde(flatten)]
    implementer: ImplementerRequest,
    /// The checkpoint run the handoff happens at, which the opening names so
    /// the successor session skips it.
    #[serde(flatten)]
    checkpoint: CheckpointFields,
}

/// The *Successor card*'s Continue: the user's first request for the
/// successor session, if they typed one, and the implementer they picked. The
/// body names no path: the engine picks it by cache warmth (ADR 0539).
#[derive(Debug, Deserialize)]
pub(in crate::api) struct ContinueRequest {
    #[serde(default)]
    message: Option<String>,
    #[serde(flatten)]
    implementer: ImplementerRequest,
}

/// When the successor session starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::api) enum SessionStart {
    /// The calling agent's turn holds the thread: the session starts once
    /// that turn ends.
    AfterThisTurn,
    /// Nothing holds the thread, so the session starts now.
    Now,
}

#[derive(Debug, Serialize)]
pub(in crate::api) struct SuccessorResponse {
    /// What the successor session runs on, so the caller reports what it
    /// started.
    implementer: Implementer,
    starts: SessionStart,
}

/// Which way Continue went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::api) enum HandoffPath {
    /// The cache was warm: the agent is writing the narrative, and the
    /// successor session follows its turn.
    Narrative,
    /// The cache was cold, or its lifetime unknown: the successor session
    /// starts at once from a brief.
    Brief,
}

impl HandoffPath {
    fn of_warmth(warmth: CacheWarmth) -> Self {
        match warmth {
            CacheWarmth::Warm => Self::Narrative,
            CacheWarmth::Cold | CacheWarmth::Unknown => Self::Brief,
        }
    }
}

#[derive(Debug, Serialize)]
pub(in crate::api) struct ContinueResponse {
    path: HandoffPath,
    /// The handoff, once it is recorded: at once on the brief path.
    #[serde(flatten)]
    successor: Option<SuccessorResponse>,
}

/// The thread, as the handoff reads it.
#[derive(sqlx::FromRow)]
struct HandingOffThread {
    is_coding_agent: bool,
    coding_agent: Option<String>,
    archive_state: String,
    successor_thread_id: Option<Uuid>,
    coding_agent_integrated: bool,
    apply_in_flight: bool,
    status: String,
    title: Option<String>,
}

fn refusal(status: StatusCode, reason: &'static str, message: impl Into<String>) -> ApiError {
    ApiError::new(status, message).with_reason(reason)
}

/// How a successor session opens.
enum Opening {
    /// The agent's *handoff narrative*, and the request the user recorded at
    /// the *Successor card*.
    Narrative {
        narrative: String,
        request: Option<HandoffRequest>,
    },
    /// No narrative: the *handoff brief*, and the user's request if any.
    Brief(Option<HandoffRequest>),
}

/// One handoff, as `hand_off` runs it.
struct HandoffStart {
    thread_id: Uuid,
    opening: Opening,
    implementer: ImplementerRequest,
    /// The agent itself named `implementer`, so it must be a rung.
    picked_by_agent: bool,
    checkpoint: Option<CheckpointRun>,
}

/// The thread, once it may hand off: the refusals every handoff shares.
/// `require_idle` is the composer's: a stale tap must not hand off a thread
/// whose turn has started since. The agent's own handoff runs inside its turn
/// by design.
async fn hand_off_candidate(
    state: &AppState,
    thread_id: Uuid,
    require_idle: bool,
) -> Result<(HandingOffThread, String), ApiError> {
    let pool = &state.pool;
    let thread: HandingOffThread = sqlx::query_as(
        "SELECT is_coding_agent, coding_agent, archive_state, successor_thread_id, \
                coding_agent_integrated, apply_in_flight, status, \
                COALESCE(title, LEFT(first_message, 80)) AS title \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::db)?
    .ok_or_else(|| ApiError::not_found(format!("Thread {thread_id} does not exist")))?;
    let title = thread
        .title
        .clone()
        .unwrap_or_else(|| "the thread".to_string());
    if !thread.is_coding_agent {
        return Err(refusal(
            StatusCode::BAD_REQUEST,
            "not-coding-agent",
            format!("\"{title}\" is not a coding-agent thread, so it has no successor session."),
        ));
    }
    if thread.coding_agent_integrated {
        return Err(refusal(
            StatusCode::CONFLICT,
            "integrated",
            format!("\"{title}\" is integrated into its parent, whose agent merges its branch. It hands off to no successor session."),
        ));
    }
    if let Some(successor) = thread.successor_thread_id {
        let link = format!(
            "[its legacy successor thread]({})",
            thread_link(&state.engine.workspace_name(), successor)
        );
        return Err(refusal(
            StatusCode::CONFLICT,
            "already-handed-off",
            format!("\"{title}\" already continued in {link}."),
        ));
    }
    if thread.archive_state == "archived" {
        return Err(refusal(
            StatusCode::CONFLICT,
            "archived",
            format!("\"{title}\" is archived. Unarchive it first."),
        ));
    }
    if require_idle && thread.status != "idle" {
        return Err(refusal(
            StatusCode::CONFLICT,
            "thread-busy",
            format!("\"{title}\" is working. Continue once it is idle."),
        ));
    }
    if thread.apply_in_flight {
        return Err(refusal(
            StatusCode::CONFLICT,
            "applying",
            format!("\"{title}\" is applying its change. Hand off once the apply ends."),
        ));
    }
    if crate::engine::successor::awaits_landing(pool, thread_id)
        .await
        .map_err(ApiError::db)?
    {
        return Err(refusal(
            StatusCode::CONFLICT,
            "awaiting-landing",
            format!(
                "\"{title}\" has a change waiting in the landing lane. Hand off once it lands."
            ),
        ));
    }
    if crate::engine::successor::pending_handoff(pool, thread_id)
        .await
        .map_err(ApiError::db)?
        .is_some()
    {
        return Err(already_handing_off(&title));
    }
    Ok((thread, title))
}

fn already_handing_off(title: &str) -> ApiError {
    refusal(
        StatusCode::CONFLICT,
        "handoff-pending",
        format!("\"{title}\" already handed off. Its successor session starts once the current turn ends."),
    )
}

/// A thread hands off only itself: an agent cannot hand off another thread's
/// work. Answers whether the caller is the agent of `thread_id`.
fn caller_is_the_agent(headers: &HeaderMap, thread_id: Uuid) -> Result<bool, ApiError> {
    match crate::api::actor::subprocess_origin(headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } if source_thread_id != Some(thread_id) => Err(refusal(
            StatusCode::FORBIDDEN,
            "not-your-thread",
            "A thread can hand off only itself. Run `lucidos successor start` in the thread that continues.",
        )),
        SubprocessOrigin::Subprocess { .. } => Ok(true),
        SubprocessOrigin::NotSubprocess => Ok(false),
    }
}

/// The agent's handoff, after it wrote its *handoff narrative*.
pub(in crate::api) async fn spawn_successor(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SuccessorRequest>,
) -> Result<Json<SuccessorResponse>, ApiError> {
    let thread_id = parse_thread_id(&thread_id)?;
    let checkpoint = request.checkpoint.run()?;
    let missing = missing_narrative_sections(&request.narrative);
    if !missing.is_empty() {
        return Err(refusal(
            StatusCode::BAD_REQUEST,
            "narrative-incomplete",
            format!(
                "The handoff narrative lacks these sections: {}. Add a heading for each, then try again.",
                missing.join(", ")
            ),
        ));
    }
    // What the user asked for at the card, while the agent wrote this. The
    // agent names no implementer of its own, so the user's pick stands.
    let pending = pending_successor_request(&state.pool, thread_id)
        .await
        .map_err(ApiError::db)?
        .unwrap_or_default();
    let picked_by_agent = request.implementer.coding_agent_model.is_some()
        || request.implementer.reasoning_effort.is_some();
    let implementer = if picked_by_agent {
        request.implementer
    } else {
        pending.implementer
    };
    hand_off(
        &state,
        &headers,
        HandoffStart {
            thread_id,
            opening: Opening::Narrative {
                narrative: request.narrative,
                request: pending.request,
            },
            implementer,
            picked_by_agent,
            checkpoint,
        },
    )
    .await
    .map(Json)
}

/// The *Successor card*'s Continue. The engine picks the path by the cache's
/// warmth now. Warm: record the request and ask the agent for its *handoff
/// narrative*, and the successor session follows when the agent hands off.
/// Cold: start the successor session at once from a *handoff brief*, and wake
/// nothing on the current session (ADR 0539).
pub(in crate::api) async fn continue_in_successor(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ContinueRequest>,
) -> Result<Json<ContinueResponse>, ApiError> {
    let thread_id = parse_thread_id(&thread_id)?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    if caller_is_the_agent(&headers, thread_id)? {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "agent-caller",
            "Continue is the user's card. An agent hands off with `lucidos successor start`.",
        ));
    }
    let message = request
        .message
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    let typed = message.clone().map(|message| HandoffRequest {
        message,
        by: actor.clone(),
    });
    let (thread, _) = hand_off_candidate(&state, thread_id, true).await?;
    // Refuse a pick the backend does not offer now. A warm tap would
    // otherwise cost a narrative turn that the handoff then refuses.
    resolve_implementer(
        &thread_implementer(&state, thread_id, thread.coding_agent.as_deref()).await?,
        &request.implementer,
    )
    .map_err(|e| refusal(StatusCode::BAD_REQUEST, "implementer-not-offered", e))?;
    let path = HandoffPath::of_warmth(
        cache_warmth(&state.pool, thread_id)
            .await
            .map_err(ApiError::db)?,
    );
    let successor = match path {
        HandoffPath::Brief => Some(
            hand_off(
                &state,
                &headers,
                HandoffStart {
                    thread_id,
                    opening: Opening::Brief(typed),
                    implementer: request.implementer,
                    picked_by_agent: false,
                    checkpoint: None,
                },
            )
            .await?,
        ),
        HandoffPath::Narrative => {
            request_narrative(
                &state,
                &headers,
                thread_id,
                actor,
                message,
                request.implementer,
            )
            .await?;
            None
        }
    };
    Ok(Json(ContinueResponse { path, successor }))
}

/// Record the user's request on the thread, then ask its agent for the
/// *handoff narrative* as a user would.
async fn request_narrative(
    state: &AppState,
    headers: &HeaderMap,
    thread_id: Uuid,
    actor: MessageOrigin,
    message: Option<String>,
    implementer: ImplementerRequest,
) -> Result<(), ApiError> {
    state
        .engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SuccessorRequested {
                message,
                coding_agent: implementer.coding_agent,
                coding_agent_model: implementer.coding_agent_model,
                reasoning_effort: implementer.reasoning_effort,
            },
            meta: EventMeta::with_actor(Some(actor.clone())),
        })
        .await
        .map_err(|e| ApiError::internal(format!("Could not record the request: {e}")))?;
    let device_id = match &actor {
        MessageOrigin::Device { device_id } => Some(device_id.clone()),
        _ => None,
    };
    let ask: ChatRequest = serde_json::from_value(serde_json::json!({
        "message": HANDOFF_REQUEST,
        "mode": actor.mode(),
        "use_coding_agent": true,
        "thread_id": thread_id.to_string(),
        "device_id": device_id,
    }))
    .map_err(|e| ApiError::internal(format!("Could not build the handoff request: {e}")))?;
    chat_submit(State(state.clone()), headers.clone(), Json(ask))
        .await
        .map(|_ack| ())
}

/// Record `start.thread_id`'s handoff, then start its successor session once
/// nothing holds the thread. A handoff is an act in the thread's own subtree,
/// so it needs no authority.
async fn hand_off(
    state: &AppState,
    headers: &HeaderMap,
    start: HandoffStart,
) -> Result<SuccessorResponse, ApiError> {
    let HandoffStart {
        thread_id,
        opening,
        implementer: requested,
        picked_by_agent,
        checkpoint,
    } = start;
    let engine = &state.engine;
    let pool = &state.pool;

    let from_agent = caller_is_the_agent(headers, thread_id)?;
    let (thread, title) =
        hand_off_candidate(state, thread_id, matches!(opening, Opening::Brief(_))).await?;
    let implementer = resolve_implementer(
        &thread_implementer(state, thread_id, thread.coding_agent.as_deref()).await?,
        &requested,
    )
    .map_err(|e| refusal(StatusCode::BAD_REQUEST, "implementer-not-offered", e))?;
    // An agent's own pick must be a rung. Carrying the thread's pair over
    // is no pick: the owner may have chosen it off the ladder (ADR 0500).
    if from_agent && picked_by_agent {
        crate::api::intelligence_ladder::refuse_off_ladder(
            state,
            implementer.coding_agent.into(),
            implementer.model.as_deref(),
            implementer.reasoning_effort.as_deref(),
        )
        .await?;
    }

    let (kind, message, request) = match opening {
        Opening::Narrative { narrative, request } => (
            HandoffOpening::Narrative,
            narrative_opening(checkpoint.as_ref().map(|c| c.name.as_str()), &narrative),
            request,
        ),
        Opening::Brief(request) => (
            HandoffOpening::Brief,
            handoff_brief(
                thread_id,
                BriefCause::CacheCold,
                request.is_some(),
                last_applied_summary(pool, thread_id).await.as_deref(),
            ),
            request,
        ),
    };

    // One handoff per session, across engines: the lock serializes two
    // requests, and the second re-reads the handoff the first recorded.
    let mut claim = pool.begin().await.map_err(ApiError::db)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("successor-handoff:{thread_id}"))
        .execute(&mut *claim)
        .await
        .map_err(ApiError::db)?;
    if crate::engine::successor::pending_handoff(pool, thread_id)
        .await
        .map_err(ApiError::db)?
        .is_some()
    {
        return Err(already_handing_off(&title));
    }
    engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionHandedOff {
                opening: kind,
                message,
                checkpoint: checkpoint.as_ref().map(|c| c.name.as_str().to_string()),
                progress: checkpoint
                    .as_ref()
                    .and_then(|c| c.progress)
                    .map(|p| p.to_string()),
                coding_agent: implementer.coding_agent,
                coding_agent_model: implementer.model.clone(),
                reasoning_effort: implementer.reasoning_effort.clone(),
                request,
            },
            meta: EventMeta::with_actor(crate::api::actor::user_actor(headers, None)),
        })
        .await
        .map_err(|e| ApiError::internal(format!("Could not record the handoff: {e}")))?;
    claim.commit().await.map_err(ApiError::db)?;
    engine.start_successor_session(thread_id);

    Ok(SuccessorResponse {
        implementer,
        starts: if from_agent {
            SessionStart::AfterThisTurn
        } else {
            SessionStart::Now
        },
    })
}

/// What the thread's last applied change did, for a brief. Best effort: a
/// brief without it is still whole.
async fn last_applied_summary(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(summary, description) FROM changes \
         WHERE thread_id = $1 AND status = 'applied' \
         ORDER BY resolved_at DESC NULLS LAST LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!("[Successor] last applied change of {}: {}", thread_id, e);
        None
    })
}
