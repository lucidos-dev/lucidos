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
//! - `POST /api/v1/threads/:thread_id/successor/withdraw`: a person chose
//!   *Return to the old session* on a pending ceiling handoff (ADR 0629).
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
use crate::engine::context_ceiling::{
    pending_ceiling_handoff, withdraw_ceiling_handoff, CeilingHandoff, ContextCeiling, Withdrawal,
};
use crate::engine::event_bus::BusEvent;
use crate::engine::handoff_estimate::{
    cache_warmth, card_verdict, record_checkpoint, CacheWarmth, CardVerdict, CheckpointRun,
    FloorRungs, HandoffVerdict,
};
use crate::engine::successor::{
    handoff_brief, last_applied_summary, missing_narrative_sections, narrative_opening,
    pending_successor_request, record_handoff, resolve_implementer, BriefCause, HandoffBlock,
    HandoffRecord, HandoffRequest, Implementer, ImplementerRequest, NewHandoff, ThreadContext,
    HANDOFF_REQUEST,
};
use crate::engine::thread_events::{
    EventMeta, HandoffOpening, MessageOrigin, SessionStart, ThreadEvent,
};
use crate::runtime::intelligence_ladder::{self, IntelligenceRung};

fn parse_thread_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))
}

/// The context reading, plus the implementer a coding-agent thread runs on,
/// which a plan card names beside the one it recommends. A checkpoint run
/// also carries the *handoff estimate*'s verdict. The plain reading carries
/// the Successor card's verdict, and any ceiling handoff waiting to start.
#[derive(Debug, Serialize)]
pub(in crate::api) struct ThreadContextResponse {
    #[serde(flatten)]
    context: ThreadContext,
    implementer: Option<Implementer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    card: Option<CardVerdict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    handoff: Option<HandoffVerdict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ceiling_handoff: Option<CeilingHandoff>,
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
    let (card, ceiling_handoff) = if is_coding_agent {
        (
            card_verdict(&state.pool, thread_id, &context)
                .await
                .map_err(ApiError::db)?,
            pending_ceiling_handoff(&state.pool, thread_id)
                .await
                .map_err(ApiError::db)?,
        )
    } else {
        (None, None)
    };
    Ok(Json(ThreadContextResponse {
        context,
        implementer,
        card,
        handoff: None,
        ceiling_handoff,
    }))
}

#[derive(Debug, Serialize)]
pub(in crate::api) struct WithdrawResponse {
    /// The thread's ceiling now, raised past the session's size.
    ceiling: ContextCeiling,
}

/// *Return to the old session* on a pending ceiling handoff: a person only.
/// It raises the thread's ceiling past the session's size, then withdraws the
/// handoff, so the next input resumes the old session.
pub(in crate::api) async fn withdraw_successor(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<WithdrawResponse>, ApiError> {
    let thread_id = parse_thread_id(&thread_id)?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    context_row(&state, thread_id).await?;
    let outcome = withdraw_ceiling_handoff(
        &state.engine.event_bus,
        &state.engine.agent_sessions,
        super::context_ceiling::ceiling_setter(&headers),
        thread_id,
        EventMeta::with_actor(Some(actor)),
    )
    .await
    .map_err(|e| ApiError::internal(format!("Could not withdraw the handoff: {e}")))?;
    match outcome {
        Withdrawal::Withdrawn(tokens) => Ok(Json(WithdrawResponse {
            ceiling: ContextCeiling::Tokens(tokens),
        })),
        Withdrawal::NeedsAPerson => Err(refusal(
            StatusCode::FORBIDDEN,
            "withdraw-needs-a-person",
            "Only a person returns a thread to its old session. Ask the user.",
        )),
        Withdrawal::NoCeilingHandoff => Err(refusal(
            StatusCode::CONFLICT,
            "no-ceiling-handoff",
            "The thread has no ceiling handoff waiting to start.",
        )),
        Withdrawal::CannotRaise => Err(refusal(
            StatusCode::CONFLICT,
            "ceiling-cannot-rise",
            "The session is larger than its context window lets a ceiling rise to. It hands off.",
        )),
        Withdrawal::Starting => Err(refusal(
            StatusCode::CONFLICT,
            "handoff-starting",
            "The successor session is already starting.",
        )),
    }
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
        ceiling_handoff: None,
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
    /// The cache was cold, or its lifetime unknown, or the provider blocked
    /// the session: the successor session starts at once from a brief.
    Brief,
}

impl HandoffPath {
    /// A blocked session is never asked for a narrative, whatever the cache's
    /// warmth: waking it would replay the history that tripped the block.
    fn of(warmth: CacheWarmth, blocking_policy: Option<&'static str>) -> Self {
        match (warmth, blocking_policy) {
            (CacheWarmth::Warm, None) => Self::Narrative,
            _ => Self::Brief,
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
    /// No narrative: the *handoff brief* for `cause`, and the user's request if
    /// any.
    Brief {
        cause: BriefCause,
        request: Option<HandoffRequest>,
    },
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
/// whose turn has started since. A failed turn has ended too, and a provider
/// block leaves its thread `failed`. The agent's own handoff runs inside its
/// turn by design.
async fn hand_off_candidate(
    state: &AppState,
    thread_id: Uuid,
    require_idle: bool,
) -> Result<(HandingOffThread, String), ApiError> {
    let pool = &state.pool;
    let thread: HandingOffThread = sqlx::query_as(
        "SELECT is_coding_agent, coding_agent, status, \
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
    if require_idle && !matches!(thread.status.as_str(), "idle" | "failed") {
        return Err(refusal(
            StatusCode::CONFLICT,
            "thread-busy",
            format!("\"{title}\" is working. Continue once it is idle."),
        ));
    }
    let block = crate::engine::successor::handoff_block(pool, thread_id)
        .await
        .map_err(ApiError::db)?;
    if let Some(block) = block {
        return Err(block_refusal(state, block, &title));
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

/// What the caller is told about a [`HandoffBlock`].
fn block_refusal(state: &AppState, block: HandoffBlock, title: &str) -> ApiError {
    let (reason, message) = match block {
        HandoffBlock::Integrated => (
            "integrated",
            format!("\"{title}\" is integrated into its parent, whose agent merges its branch. It hands off to no successor session."),
        ),
        HandoffBlock::LegacySuccessor(successor) => (
            "already-handed-off",
            format!(
                "\"{title}\" already continued in [its legacy successor thread]({}).",
                thread_link(&state.engine.workspace_name(), successor)
            ),
        ),
        HandoffBlock::Archived => (
            "archived",
            format!("\"{title}\" is archived. Unarchive it first."),
        ),
        HandoffBlock::Applying => (
            "applying",
            format!("\"{title}\" is applying its change. Hand off once the apply ends."),
        ),
        HandoffBlock::AwaitingLanding => (
            "awaiting-landing",
            format!(
                "\"{title}\" has a change waiting in the landing lane. Hand off once it lands."
            ),
        ),
    };
    refusal(StatusCode::CONFLICT, reason, message)
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
    let blocking_policy =
        crate::engine::agent_session::blocking_policy(&state.pool, thread_id).await;
    let path = HandoffPath::of(
        cache_warmth(&state.pool, thread_id)
            .await
            .map_err(ApiError::db)?,
        blocking_policy,
    );
    let successor = match path {
        HandoffPath::Brief => Some(
            hand_off(
                &state,
                &headers,
                HandoffStart {
                    thread_id,
                    opening: Opening::Brief {
                        cause: blocking_policy.map_or(BriefCause::CacheCold, |policy| {
                            BriefCause::ProviderBlocked { policy }
                        }),
                        request: typed,
                    },
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
        hand_off_candidate(state, thread_id, matches!(opening, Opening::Brief { .. })).await?;
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
        Opening::Brief { cause, request } => (
            HandoffOpening::Brief,
            handoff_brief(
                thread_id,
                cause,
                request.is_some(),
                last_applied_summary(pool, thread_id).await.as_deref(),
            ),
            request,
        ),
    };
    let crossing = crate::engine::context_ceiling::past_ceiling(pool, thread_id)
        .await
        .map_err(ApiError::db)?;
    let (cause, starts) = crate::engine::successor::cause_and_start(
        crossing.is_some(),
        checkpoint.is_some(),
        request.is_some(),
        from_agent,
    );
    let handoff = NewHandoff {
        opening: kind,
        message: engine.still_running(thread_id).await.after(message),
        checkpoint: checkpoint.as_ref().map(|c| c.name.as_str().to_string()),
        progress: checkpoint
            .as_ref()
            .and_then(|c| c.progress)
            .map(|p| p.to_string()),
        implementer: implementer.clone(),
        request,
        cause,
        starts,
        crossing,
    };
    let meta = EventMeta::with_actor(crate::api::actor::user_actor(headers, None));
    let record = record_handoff(&engine.event_bus, thread_id, handoff, meta)
        .await
        .map_err(|e| ApiError::internal(format!("Could not record the handoff: {e}")))?;
    match record {
        HandoffRecord::Recorded => {}
        HandoffRecord::Pending => return Err(already_handing_off(&title)),
        HandoffRecord::CeilingLifted => {
            return Err(refusal(
                StatusCode::CONFLICT,
                "ceiling-lifted",
                "A person raised this thread's context ceiling while it handed off. \
                 Hand off again if you still mean to.",
            ))
        }
    }
    engine.start_successor_session(thread_id);

    Ok(SuccessorResponse {
        implementer,
        starts,
    })
}
