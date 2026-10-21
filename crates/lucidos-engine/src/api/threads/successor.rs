//! *Successor threads*: a coding-agent thread's context size, and the handoff
//! to a new thread that carries on with a fresh context.
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
//! The successor sits in the old thread's place (ADR 0168): top-level when the
//! old thread was, else a child of the same parent with the same wake mode.
//! A top-level plan card's handoff starts a *build chain* under the old thread
//! instead (ADR 0503). Its branch starts at the old thread's last commit
//! (`engine::successor::carried_base`).

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
use crate::api::thread_reach::{
    authority_held, pass_authority, HeldAuthority, PassedAuthority, ThreadReachError,
    ThreadReachVerb,
};
use crate::api::AppState;
use crate::api::ChatRequest;
use crate::core::store::thread_link;
use crate::engine::event_bus::BusEvent;
use crate::engine::handoff_estimate::{
    cache_warmth, card_verdict, handoff_verdict, record_checkpoint, CacheWarmth, CardVerdict,
    CheckpointRun, HandoffVerdict,
};
use crate::engine::successor::{
    build_chain, carried_consent, handoff_brief, missing_narrative_sections,
    pending_successor_request, record_handoff, resolve_implementer, Handoff, Implementer,
    ImplementerRequest, Placement, ThreadContext, HANDOFF_REQUEST,
};
use crate::engine::thread_events::{
    ActorMode, EventMeta, HandoffConsent, MessageOrigin, ThreadDirection, ThreadEvent,
};

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
    handoff: Option<HandoffReading>,
}

/// The verdict at one checkpoint run, and whether the handoff would need an
/// owner approval card from this caller.
#[derive(Debug, Serialize)]
pub(in crate::api) struct HandoffReading {
    #[serde(flatten)]
    verdict: HandoffVerdict,
    without_card: bool,
    /// The thread an owner approval card names, when the handoff needs one:
    /// the parent the successor would sit under.
    #[serde(skip_serializing_if = "Option::is_none")]
    approval_target: Option<Uuid>,
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
) -> Result<(bool, Option<String>, Option<Uuid>), ApiError> {
    sqlx::query_as(
        "SELECT is_coding_agent, coding_agent, parent_thread_id \
         FROM thread_summaries WHERE thread_id = $1",
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
    let (is_coding_agent, coding_agent, _) = context_row(&state, thread_id).await?;
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
    let (is_coding_agent, _, _) = context_row(&state, thread_id).await?;
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

/// A checkpoint run's body. `offered` marks a `--ask` run.
#[derive(Debug, Deserialize)]
pub(in crate::api) struct CheckpointRequest {
    name: String,
    #[serde(default)]
    progress: Option<String>,
    #[serde(default)]
    offered: bool,
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
    let (is_coding_agent, coding_agent, parent) = context_row(&state, thread_id).await?;
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
    let verdict = record_checkpoint(
        &state.engine.event_bus,
        &state.pool,
        thread_id,
        &run,
        request.offered,
        Some(actor),
    )
    .await
    .map_err(|e| ApiError::internal(format!("Could not record the checkpoint: {e}")))?;
    let without_card = match parent {
        None => true,
        Some(parent) => {
            hands_off_without_card(&state.pool, &headers, thread_id, parent, &run).await?
        }
    };
    let context = crate::engine::successor::thread_context(&state.pool, thread_id)
        .await
        .map_err(ApiError::db)?;
    Ok(Json(ThreadContextResponse {
        context,
        implementer: Some(thread_implementer(&state, thread_id, coding_agent.as_deref()).await?),
        card: None,
        handoff: Some(HandoffReading {
            verdict,
            without_card,
            approval_target: parent.filter(|_| !without_card),
        }),
    }))
}

/// The implementer a coding-agent thread runs on: its backend, and the model
/// and effort its calls use (`cc_thread_runs_on`). A successor carries these
/// over.
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

/// The agent's handoff: a *handoff narrative*, and optionally the successor's
/// implementer. An agent names none, so the implementer the user picked at
/// the *Successor card* applies (`pending_successor_request`).
#[derive(Debug, Deserialize)]
pub(in crate::api) struct SuccessorRequest {
    #[serde(default)]
    narrative: String,
    #[serde(flatten)]
    implementer: ImplementerRequest,
    /// A plan card's handoff: start a *build chain* under this thread when it
    /// is top-level. A child keeps its place, so its parent still gets the
    /// report.
    #[serde(default)]
    build_chain: bool,
    /// The checkpoint run the handoff happens at, for a standing consent:
    /// the build chain's, or a carried *handoff consent*.
    #[serde(flatten)]
    checkpoint: CheckpointFields,
}

/// The *Successor card*'s Continue: the user's first request for the
/// successor, if they typed one, and the implementer they picked. The body
/// names no path: the engine picks it by cache warmth (ADR 0539).
#[derive(Debug, Deserialize)]
pub(in crate::api) struct ContinueRequest {
    #[serde(default)]
    message: Option<String>,
    #[serde(flatten)]
    implementer: ImplementerRequest,
}

#[derive(Debug, Serialize)]
pub(in crate::api) struct SuccessorResponse {
    successor_thread_id: Uuid,
    /// A markdown link to the successor, as `lucidos spawn-thread` prints one.
    link: String,
    /// What the successor runs on, so the caller reports what it started.
    implementer: Implementer,
    /// The successor started a build chain under the old thread.
    build_chain: bool,
}

/// Which way Continue went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::api) enum HandoffPath {
    /// The cache was warm: the old agent is writing the narrative, and the
    /// successor follows when it spawns.
    Narrative,
    /// The cache was cold: the successor started at once from a brief.
    Brief,
}

impl HandoffPath {
    fn of_warmth(warmth: CacheWarmth) -> Self {
        match warmth {
            CacheWarmth::Warm => Self::Narrative,
            CacheWarmth::Cold => Self::Brief,
        }
    }
}

#[derive(Debug, Serialize)]
pub(in crate::api) struct ContinueResponse {
    path: HandoffPath,
    /// The successor, once it exists: at once on the brief path.
    #[serde(flatten)]
    successor: Option<SuccessorResponse>,
}

/// The old thread, as the handoff reads it.
#[derive(sqlx::FromRow)]
struct OldThread {
    is_coding_agent: bool,
    coding_agent: Option<String>,
    cc_repo_id: Option<String>,
    parent_thread_id: Option<Uuid>,
    wake_mode: Option<String>,
    spawning_event_id: Option<Uuid>,
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

/// How a successor opens.
enum Opening {
    /// The agent's *handoff narrative*, and the request the user recorded at
    /// the *Successor card*.
    Narrative {
        narrative: String,
        request: Option<String>,
    },
    /// No narrative: the *handoff brief*, quoting the user's request if any.
    Brief(Option<String>),
}

/// One handoff, as `start_successor` runs it.
struct SuccessorStart {
    old_id: Uuid,
    opening: Opening,
    implementer: ImplementerRequest,
    /// The agent itself named `implementer`, so it must be a rung.
    picked_by_agent: bool,
    build_chain: bool,
    checkpoint: Option<CheckpointRun>,
}

/// The old thread, once it may hand off: the refusals every handoff shares.
/// `require_idle` is the composer's: a stale tap must not hand off a thread
/// whose turn has started since. The agent's own handoff runs inside its turn
/// by design.
async fn hand_off_candidate(
    state: &AppState,
    old_id: Uuid,
    require_idle: bool,
) -> Result<(OldThread, String), ApiError> {
    let pool = &state.pool;
    let link = |title: &str, id: Uuid| {
        format!(
            "[{title}]({})",
            thread_link(&state.engine.workspace_name(), id)
        )
    };
    let old: OldThread = sqlx::query_as(
        "SELECT is_coding_agent, coding_agent, cc_repo_id, parent_thread_id, wake_mode, \
                spawning_event_id, archive_state, successor_thread_id, coding_agent_integrated, \
                apply_in_flight, status, \
                COALESCE(title, LEFT(first_message, 80)) AS title \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(old_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::db)?
    .ok_or_else(|| ApiError::not_found(format!("Thread {old_id} does not exist")))?;
    let old_title = old
        .title
        .clone()
        .unwrap_or_else(|| "the earlier thread".to_string());
    if !old.is_coding_agent {
        return Err(refusal(
            StatusCode::BAD_REQUEST,
            "not-coding-agent",
            format!("\"{old_title}\" is not a coding-agent thread, so it has no successor."),
        ));
    }
    if old.coding_agent_integrated {
        return Err(refusal(
            StatusCode::CONFLICT,
            "integrated",
            format!("\"{old_title}\" is integrated into its parent, whose agent merges its branch. It hands off to no successor."),
        ));
    }
    if let Some(successor) = old.successor_thread_id {
        return Err(refusal(
            StatusCode::CONFLICT,
            "already-handed-off",
            format!(
                "\"{old_title}\" already continued in {}.",
                link("its successor", successor)
            ),
        ));
    }
    if old.archive_state == "archived" {
        return Err(refusal(
            StatusCode::CONFLICT,
            "archived",
            format!("\"{old_title}\" is archived. Unarchive it first."),
        ));
    }
    if require_idle && old.status != "idle" {
        return Err(refusal(
            StatusCode::CONFLICT,
            "thread-busy",
            format!("\"{old_title}\" is working. Continue once it is idle."),
        ));
    }
    if old.apply_in_flight {
        return Err(refusal(
            StatusCode::CONFLICT,
            "applying",
            format!("\"{old_title}\" is applying its change. Hand off once the apply ends."),
        ));
    }
    if crate::engine::successor::awaits_landing(pool, old_id)
        .await
        .map_err(ApiError::db)?
    {
        return Err(refusal(
            StatusCode::CONFLICT,
            "awaiting-landing",
            format!(
                "\"{old_title}\" has a change waiting in the landing lane. Hand off once it lands."
            ),
        ));
    }
    Ok((old, old_title))
}

/// A thread hands off only itself: an agent cannot hand off another thread's
/// work. Answers whether the caller is the agent of `old_id`.
fn caller_is_the_agent(headers: &HeaderMap, old_id: Uuid) -> Result<bool, ApiError> {
    match crate::api::actor::subprocess_origin(headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } if source_thread_id != Some(old_id) => Err(refusal(
            StatusCode::FORBIDDEN,
            "not-your-thread",
            "A thread can hand off only itself. Run `lucidos successor spawn` in the thread that continues.",
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
    let old_id = parse_thread_id(&thread_id)?;
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
    let pending = pending_successor_request(&state.pool, old_id)
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
    start_successor(
        &state,
        &headers,
        SuccessorStart {
            old_id,
            opening: Opening::Narrative {
                narrative: request.narrative,
                request: pending.message,
            },
            implementer,
            picked_by_agent,
            build_chain: request.build_chain,
            checkpoint,
        },
    )
    .await
    .map(Json)
}

/// The *Successor card*'s Continue. The engine picks the path by the cache's
/// warmth now. Warm: record the request and ask the old agent for its
/// *handoff narrative*, and the successor follows when the agent spawns it.
/// Cold: start the successor at once from a *handoff brief*, and wake nothing
/// on the old thread (ADR 0539).
pub(in crate::api) async fn continue_in_successor(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ContinueRequest>,
) -> Result<Json<ContinueResponse>, ApiError> {
    let old_id = parse_thread_id(&thread_id)?;
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    if caller_is_the_agent(&headers, old_id)? {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "agent-caller",
            "Continue is the user's card. An agent hands off with `lucidos successor spawn`.",
        ));
    }
    let message = request
        .message
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    let (old, _) = hand_off_candidate(&state, old_id, true).await?;
    // Refuse a pick the backend does not offer now. A warm tap would
    // otherwise cost a narrative turn that the spawn then refuses.
    resolve_implementer(
        &thread_implementer(&state, old_id, old.coding_agent.as_deref()).await?,
        &request.implementer,
    )
    .map_err(|e| refusal(StatusCode::BAD_REQUEST, "implementer-not-offered", e))?;
    let path = HandoffPath::of_warmth(
        cache_warmth(&state.pool, old_id)
            .await
            .map_err(ApiError::db)?,
    );
    let successor = match path {
        HandoffPath::Brief => Some(
            start_successor(
                &state,
                &headers,
                SuccessorStart {
                    old_id,
                    opening: Opening::Brief(message),
                    implementer: request.implementer,
                    picked_by_agent: false,
                    build_chain: false,
                    checkpoint: None,
                },
            )
            .await?,
        ),
        HandoffPath::Narrative => {
            request_narrative(
                &state,
                &headers,
                old_id,
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

/// Record the user's request on the old thread, then ask its agent for the
/// *handoff narrative* as a user would.
async fn request_narrative(
    state: &AppState,
    headers: &HeaderMap,
    old_id: Uuid,
    actor: MessageOrigin,
    message: Option<String>,
    implementer: ImplementerRequest,
) -> Result<(), ApiError> {
    state
        .engine
        .event_bus
        .emit(BusEvent::Thread {
            thread_id: old_id,
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
        "thread_id": old_id.to_string(),
        "device_id": device_id,
    }))
    .map_err(|e| ApiError::internal(format!("Could not build the handoff request: {e}")))?;
    chat_submit(State(state.clone()), headers.clone(), Json(ask))
        .await
        .map(|_ack| ())
}

/// Start the successor of `start.old_id`: record the handoff and queue its
/// first message.
async fn start_successor(
    state: &AppState,
    headers: &HeaderMap,
    start: SuccessorStart,
) -> Result<SuccessorResponse, ApiError> {
    let SuccessorStart {
        old_id,
        opening,
        implementer: requested,
        picked_by_agent,
        build_chain: wants_build_chain,
        checkpoint,
    } = start;
    let engine = &state.engine;
    let pool = &state.pool;
    let workspace = engine.workspace_name();
    let link = |title: &str, id: Uuid| format!("[{title}]({})", thread_link(&workspace, id));

    let from_agent = caller_is_the_agent(headers, old_id)?;
    let (old, old_title) =
        hand_off_candidate(state, old_id, matches!(opening, Opening::Brief(_))).await?;
    let implementer = resolve_implementer(
        &thread_implementer(state, old_id, old.coding_agent.as_deref()).await?,
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
    let base_sha = carried_head(state, old_id, &old_title).await?;

    // Last before any write: an owner approval this spends must not be
    // wasted on a refusal above.
    let build_chain = wants_build_chain && old.parent_thread_id.is_none();
    let placement = if build_chain {
        Placement::build_chain_under(old_id)
    } else {
        Placement::of_old_thread(
            old.parent_thread_id,
            old.wake_mode.as_deref(),
            old.spawning_event_id,
        )
    };
    let actor = crate::api::actor::user_actor(headers, None);
    // A top-level successor is a top-thread, which needs no authority (ADR
    // 0475). A child one is an act in its parent's subtree, unless a standing
    // consent covers it (ADR 0503, 0507).
    let handoff_consent = match placement.parent_thread_id {
        None => None,
        Some(parent) => match standing_consent(pool, old_id, parent, checkpoint.as_ref()).await? {
            Some(StandingConsent::BuildChain) => None,
            Some(StandingConsent::Carried) => Some(HandoffConsent::Carried),
            None => {
                let passed = pass_authority(
                    &engine.event_bus,
                    headers,
                    Some(parent),
                    ThreadReachVerb::SpawnSuccessor,
                )
                .await
                .map_err(|e| authority_refusal(e, parent))?;
                // A build chain's own consent covers its members, so its
                // builder records none.
                recorded_consent(passed, actor.as_ref()).filter(|_| !build_chain)
            }
        },
    };

    let (mode, origin) = if from_agent {
        (
            ActorMode::Agent,
            Some(MessageOrigin::ThreadLink {
                thread_id: old_id,
                title: Some(old_title.clone()),
                spawning_event_id: None,
                mode: ActorMode::Agent,
                direction: ThreadDirection::Parent,
            }),
        )
    } else {
        // The first message speaks as the credential: a device is the user,
        // the local token is the machine. A mismatch would fail the spawn
        // after the handoff is recorded.
        let origin = actor.clone().ok_or_else(|| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "A handoff needs a credential that names who asks.",
            )
        })?;
        (origin.mode(), Some(origin))
    };
    let device_id = match &actor {
        Some(MessageOrigin::Device { device_id }) => Some(device_id.clone()),
        _ => None,
    };

    let old_link = link(&old_title, old_id);
    let message = match opening {
        Opening::Narrative { narrative, request } => narrative_opening(
            &old_link,
            checkpoint.as_ref(),
            &narrative,
            request.as_deref(),
        ),
        Opening::Brief(request) => handoff_brief(
            &old_link,
            old_id,
            request.as_deref(),
            last_applied_summary(pool, old_id).await.as_deref(),
        ),
    };

    // Every fallible read comes before the handoff is recorded, so a failure
    // here leaves the old thread as it was.
    let app_id: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'app_id' FROM events \
         WHERE thread_id = $1 AND event_type = 'SessionStarted' \
           AND payload->>'coding_agent_kind' = 'app' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(old_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::db)?
    .flatten();
    let successor_id = Uuid::new_v4();
    let successor_title = crate::engine::successor::successor_title(
        &old_title,
        checkpoint
            .as_ref()
            .map(CheckpointRun::title_label)
            .as_deref(),
    );
    let queued = crate::engine::thread_queue::ThreadQueueRequest::AgentChat {
        depth: 0,
        message,
        thread_id: successor_id,
        event_id: Some(Uuid::new_v4().to_string()),
        image_hashes: Vec::new(),
        device_id,
        model: None,
        reasoning_effort: implementer.reasoning_effort.clone(),
        provider: None,
        use_coding_agent: Some(true),
        repo_id: if app_id.is_some() {
            None
        } else {
            old.cc_repo_id
        },
        cc_model: implementer.model.clone(),
        coding_agent: Some(implementer.coding_agent),
        title: Some(successor_title.clone()),
        mode,
        origin: origin.clone(),
        parent_thread_id: placement.parent_thread_id,
        spawning_event_id: placement.spawning_event_id,
        wake_mode: placement.wake_mode,
        app_id,
        integrated: false,
    };

    // One handoff per thread, across engines: the lock serializes two
    // requests, and the second re-reads the link the first recorded.
    let mut claim = pool.begin().await.map_err(ApiError::db)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("successor-handoff:{old_id}"))
        .execute(&mut *claim)
        .await
        .map_err(ApiError::db)?;
    if let Some(successor) = crate::engine::successor::successor_thread_of(pool, old_id)
        .await
        .map_err(ApiError::db)?
    {
        return Err(refusal(
            StatusCode::CONFLICT,
            "already-handed-off",
            format!(
                "\"{old_title}\" already continued in {}.",
                link("its successor", successor)
            ),
        ));
    }
    record_handoff(
        &engine.event_bus,
        pool,
        old_id,
        Handoff {
            successor_thread_id: successor_id,
            base_sha,
            starts_build_chain: build_chain,
            handoff_consent,
        },
        actor.clone(),
    )
    .await
    .map_err(|e| ApiError::internal(format!("Could not record the handoff: {e}")))?;
    claim.commit().await.map_err(ApiError::db)?;
    engine.thread_queue.submit(queued, origin, None).await;

    Ok(SuccessorResponse {
        successor_thread_id: successor_id,
        link: link(&successor_title, successor_id),
        implementer,
        build_chain,
    })
}

/// A consent that lets a thread hand off under its parent with no owner
/// approval card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StandingConsent {
    /// The thread is a member of the parent's build chain (ADR 0503).
    BuildChain,
    /// The thread inherits the *handoff consent* its predecessor recorded
    /// (ADR 0507).
    Carried,
}

/// The standing consent `old_id` holds to hand off under `parent`. It holds
/// only at a checkpoint run whose handoff estimate pays, and covers no
/// other verb and no other target. A failed read is an error, never a yes.
async fn standing_consent(
    pool: &sqlx::PgPool,
    old_id: Uuid,
    parent: Uuid,
    checkpoint: Option<&CheckpointRun>,
) -> Result<Option<StandingConsent>, ApiError> {
    let Some(checkpoint) = checkpoint else {
        return Ok(None);
    };
    let in_chain = build_chain(pool, old_id)
        .await
        .map_err(ApiError::db)?
        .is_some_and(|chain| chain.planner == parent);
    let consent = if in_chain {
        StandingConsent::BuildChain
    } else if carried_consent(pool, old_id, parent)
        .await
        .map_err(ApiError::db)?
        .is_some()
    {
        StandingConsent::Carried
    } else {
        return Ok(None);
    };
    let pays = handoff_verdict(pool, old_id, checkpoint)
        .await
        .map_err(ApiError::db)?
        .pays
        == Some(true);
    if !pays {
        return Ok(None);
    }
    crate::log!(
        "[Successor] {} hands off under {} on a standing consent: {:?}",
        old_id,
        parent,
        consent
    );
    Ok(Some(consent))
}

/// The *handoff consent* a spawn records for the authority that let it
/// through. A caller with no token is the owner only on a device: the machine
/// token names nobody. A thread's own subtree is its own authority, never the
/// owner's.
fn recorded_consent(
    passed: PassedAuthority,
    actor: Option<&MessageOrigin>,
) -> Option<HandoffConsent> {
    match passed {
        PassedAuthority::Untokened => {
            matches!(actor, Some(MessageOrigin::Device { .. })).then_some(HandoffConsent::Owner)
        }
        PassedAuthority::Held(HeldAuthority::StandingInstruction) => Some(HandoffConsent::Owner),
        PassedAuthority::Held(HeldAuthority::OwnSubtree | HeldAuthority::NoneNeeded) => None,
        PassedAuthority::OwnerApproval => Some(HandoffConsent::OwnerApproval),
    }
}

/// Whether this caller may start `thread_id`'s successor under `parent` with
/// no owner approval card: a standing consent, or authority of its own. The
/// spawn will weigh the same headers.
async fn hands_off_without_card(
    pool: &sqlx::PgPool,
    headers: &HeaderMap,
    thread_id: Uuid,
    parent: Uuid,
    checkpoint: &CheckpointRun,
) -> Result<bool, ApiError> {
    if standing_consent(pool, thread_id, parent, Some(checkpoint))
        .await?
        .is_some()
    {
        return Ok(true);
    }
    let SubprocessOrigin::Subprocess {
        source_thread_id,
        emitting_trigger_id,
        ..
    } = crate::api::actor::subprocess_origin(headers)
    else {
        return Ok(true);
    };
    Ok(authority_held(
        pool,
        source_thread_id,
        emitting_trigger_id.as_deref(),
        Some(parent),
        ThreadReachVerb::SpawnSuccessor,
    )
    .await?
    .is_some())
}

/// A refused handoff names the approval that lets it through.
fn authority_refusal(e: ThreadReachError, parent: Uuid) -> ApiError {
    let approval = format!(
        "`lucidos ask-owner-approval spawn-successor --thread {parent} --reason \"<why>\"`"
    );
    match e {
        ThreadReachError::NoStandingInstruction(_) | ThreadReachError::OutOfReach { .. } => {
            refusal(
                StatusCode::FORBIDDEN,
                "needs-owner-approval",
                format!(
                    "A successor sits in this thread's place, which is the workspace owner's \
                     call in a turn they did not open. Run {approval}, ask with the id it \
                     prints, and retry once the owner allows it."
                ),
            )
        }
        other => other.into(),
    }
}

/// The old thread's last commit, which the successor's branch starts at.
/// Refuses uncommitted work rather than leave it behind, and refuses when
/// git cannot answer rather than guess.
async fn carried_head(state: &AppState, old_id: Uuid, old_title: &str) -> Result<String, ApiError> {
    let live = {
        let sessions = state.engine.agent_sessions.lock().await;
        crate::api::repositories::live_thread_worktree(sessions.get(&old_id))
    };
    let worktree = crate::api::repositories::resolve_diff_worktree(
        &state.pool,
        old_id,
        live,
        state.engine.workspace_path(),
    )
    .await
    .ok_or_else(|| {
        refusal(
            StatusCode::CONFLICT,
            "no-worktree",
            format!(
                "\"{old_title}\" has no worktree left on disk, so there is no branch to carry."
            ),
        )
    })?;
    let unknown = || {
        refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "git-unanswered",
            format!("Could not read the state of \"{old_title}\"'s worktree. Nothing changed; try again."),
        )
    };
    use crate::engine::git_ops::GitAnswer;
    match crate::engine::git_ops::worktree_dirtiness(&worktree).await {
        GitAnswer::No => {}
        GitAnswer::Yes => {
            return Err(refusal(
                StatusCode::CONFLICT,
                "uncommitted-work",
                format!("\"{old_title}\" has uncommitted changes. Commit them first, so the successor carries them."),
            ))
        }
        GitAnswer::Unknown => return Err(unknown()),
    }
    crate::engine::git_ops::current_head_sha(&worktree)
        .await
        .ok_or_else(unknown)
}

/// A narrative successor's opening message. A successor started at a
/// checkpoint begins at the step after it, so it never runs that one again.
/// A request the user recorded at the card comes last.
fn narrative_opening(
    old_link: &str,
    checkpoint: Option<&CheckpointRun>,
    narrative: &str,
    request: Option<&str>,
) -> String {
    let skip = checkpoint
        .map(|c| {
            format!(
                " That thread handed off at the `{}` checkpoint, so skip that checkpoint \
                 when your flow reaches it.",
                c.name.as_str()
            )
        })
        .unwrap_or_default();
    let request = request
        .map(|r| format!("\n\n---\nThe user's first request for you:\n\n{r}"))
        .unwrap_or_default();
    format!(
        "This thread continues {old_link}. Its worktree starts at that thread's last \
         commit, so it already holds all of its work. Read the handoff narrative below, \
         then carry on from its next step.{skip}\n\n{narrative}{request}"
    )
}

/// What the old thread's last applied change did, for the composer's
/// successor. Best effort: a successor without it is still whole.
async fn last_applied_summary(pool: &sqlx::PgPool, old_id: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(summary, description) FROM changes \
         WHERE thread_id = $1 AND status = 'applied' \
         ORDER BY resolved_at DESC NULLS LAST LIMIT 1",
    )
    .bind(old_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!("[Successor] last applied change of {}: {}", old_id, e);
        None
    })
}

#[cfg(test)]
#[path = "successor_tests.rs"]
mod tests;
