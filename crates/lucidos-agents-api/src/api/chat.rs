use super::actor::build_message_origin;
use super::*;
use crate::engine::agent_session::withdraw::WithdrawExt;
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::chat::rung::RungExt;
use crate::engine::thread_events::{ActorMode, EventMeta, MessageOrigin, WakeMode};
use crate::engine::thread_lifecycle::ThreadType;
use crate::engine::thread_state::ThreadState;
use crate::engine::InjectedPrompt;
use crate::engine::PreEmittedOrigin;
use crate::engine::ThreadAgent;
use crate::engine::{
    overtaken_message, InputAdmission, InputOrder, InputPosition, OVERTAKEN_REASON,
};
use crate::runtime::InputWithdrawal;
use std::collections::VecDeque;
use std::path::PathBuf;

pub(crate) fn subprocess_chat_legitimate(
    mode: ActorMode,
    source_thread_id: Option<Uuid>,
    target_thread_id: Option<Uuid>,
    parent_thread_id: Option<Uuid>,
    target_exists: bool,
) -> bool {
    let target_matches_source = target_thread_id.is_some() && target_thread_id == source_thread_id;
    let parent_matches_source = match (parent_thread_id, source_thread_id) {
        (Some(p), Some(s)) => p == s,
        _ => false,
    };
    match mode {
        ActorMode::Human => false,
        ActorMode::Agent | ActorMode::Engine => {
            target_matches_source || (parent_matches_source && !target_exists)
        }
    }
}

pub(crate) fn subprocess_gate_applies(
    caller_workspace_present: bool,
    target_exists: bool,
    mode: ActorMode,
) -> bool {
    !(caller_workspace_present && !target_exists && mode != ActorMode::Human)
}

pub(crate) const HUMAN_MODE_UNATTRIBUTED: &str = "\
`mode: \"human\"` requires a registered device, and this request has none, so \
the engine cannot record it as something the user typed. If you are an agent: \
do not post to the engine API as the user. Use `follow_up_child_thread` (or \
`lucidos threads follow-up`) to reach one of your own child threads, and if no \
tool covers what you were asked to do, tell the user it is not possible rather \
than working around the tool.";

pub(crate) fn human_mode_is_attributed(
    mode: ActorMode,
    device_attributed: bool,
    caller_workspace_present: bool,
) -> bool {
    match mode {
        ActorMode::Human => device_attributed || caller_workspace_present,
        ActorMode::Agent | ActorMode::Engine => true,
    }
}

fn sending_device_id<'a>(
    request: &'a ChatRequest,
    headers: &'a axum::http::HeaderMap,
) -> Option<&'a str> {
    super::actor::authenticated_device_id(request.device_id.as_deref(), headers)
}

/// INVARIANT: Resolve the device evidence for `request` and refuse a `mode: human` claim
/// that has none. `chat_submit` calls it before any other work, so a refusal
/// writes nothing.
///
/// The device id comes from [`sending_device_id`], which the origin builder
/// reads too. A stamped id attributes the send with no row, the way
/// `actor::require_user_actor` accepts it.
///
/// A database error counts as attributed. The probe sits on the user's own send
/// path, so failing closed would refuse real messages.
/// `DeviceStore::is_registered` returns the error so this fallback is chosen
/// here rather than swallowed at the query.
async fn require_human_mode_is_attributed(
    pool: &sqlx::PgPool,
    headers: &axum::http::HeaderMap,
    request: &ChatRequest,
) -> Result<(), ApiError> {
    if request.mode != ActorMode::Human {
        return Ok(());
    }
    let device_id = sending_device_id(request, headers);
    let device_attributed = match device_id {
        Some(id) if super::gateway_stamp::device_is_stamped(headers, id) => true,
        Some(id) => match crate::core::DeviceStore::is_registered(pool, id).await {
            Ok(exists) => exists,
            Err(e) => {
                log_legacy!(
                    "[Chat] device lookup for '{}' failed ({}); treating the request as \
                     device-attributed so a database blip cannot refuse a real send",
                    id,
                    e
                );
                true
            }
        },
        None => false,
    };
    if human_mode_is_attributed(
        request.mode,
        device_attributed,
        request.caller_workspace.is_some(),
    ) {
        return Ok(());
    }
    log_legacy!(
        "[Chat] Refusing unattributed mode=human POST: device_id={:?}, user_agent={:?}, thread={:?}",
        device_id,
        headers.get("user-agent").and_then(|v| v.to_str().ok()),
        request.thread_id
    );
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        HUMAN_MODE_UNATTRIBUTED,
    ))
}

/// INVARIANT: Refuse a body `mode` that the request's credential contradicts.
///
/// `make_message_received` treats an origin whose mode differs from the
/// request's as a broken invariant and panics. So the mismatch must stop here
/// as a 400, before anything is written. A subprocess token claiming
/// `mode: engine` is the known shape: its origin is always an agent's.
pub(crate) fn origin_agrees_with_mode(
    origin: Option<&MessageOrigin>,
    mode: ActorMode,
) -> Result<(), ApiError> {
    match origin {
        Some(origin) if origin.mode() != mode => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "mode \"{}\" does not match this request's credential, which identifies \
                 the sender as \"{}\". Send the mode your credential carries.",
                mode.as_str(),
                origin.mode().as_str()
            ),
        )),
        _ => Ok(()),
    }
}

pub(crate) fn unknown_thread_message(thread_id: Uuid) -> String {
    format!(
        "No thread {thread_id} exists in this workspace, and this request does not say it is \
         creating one, so nothing was written. If you meant to start a new thread, send \
         `new_thread: true`. If you meant to reach an existing thread, you are addressing the \
         wrong engine or the wrong id: several Lucidos engines run on one machine, each serving \
         a different workspace, so check GET /api/v1/health before retrying."
    )
}

pub(crate) fn thread_target_is_addressable(
    thread_exists: bool,
    admitted_input: bool,
    new_thread: Option<bool>,
    parent_thread_id: Option<Uuid>,
    caller_workspace_present: bool,
) -> bool {
    thread_exists
        || admitted_input
        || new_thread == Some(true)
        || parent_thread_id.is_some()
        || caller_workspace_present
}

pub(crate) const NOTHING_TO_START_THE_THREAD_WITH: &str = "\
    Nothing to start the thread with: the message has no text and no image, so no thread was \
    created. A spawner that builds the message from a file should check the file exists first.";

pub(crate) fn thread_has_started(state: Option<&str>) -> bool {
    state.is_some_and(|s| s != ThreadState::Composing.as_str())
}

pub(crate) fn require_input_to_create(
    has_started: bool,
    message: &str,
    images: &[ChatImage],
) -> Result<(), ApiError> {
    if has_started || !images.is_empty() || !message.trim().is_empty() {
        return Ok(());
    }
    Err(ApiError::bad_request(NOTHING_TO_START_THE_THREAD_WITH))
}

pub(crate) fn require_offered_coding_agent_model(
    has_started: bool,
    use_coding_agent: Option<bool>,
    coding_agent: Option<crate::runtime::CodingAgent>,
    cc_model: Option<&str>,
) -> Result<(), ApiError> {
    if has_started || use_coding_agent != Some(true) {
        return Ok(());
    }
    let agent = coding_agent.unwrap_or(crate::runtime::CodingAgent::ClaudeCode);
    crate::runtime::validate_coding_agent_model(agent, cc_model)
        .map(drop)
        .map_err(ApiError::bad_request)
}

/// INVARIANT: Announce every orphan in `batch` as `PromptInjected`, anchored on the
/// batch's first message (the turn the re-process runs under).
///
/// The batch is announced WHOLE, not `[1..]`, and two things ride on that.
///
/// **The projection.** An orphan's `MessageReceived` was persisted before the
/// terminator that ended the previous turn, so the re-process passes
/// `PreEmittedOrigin::EngineReentry` and emits no starter event. That leaves
/// `PromptInjected` as the only event in the turn whose lifecycle rule is
/// `StatusRule::Set(ThreadStatus::Running)`. Skip the first orphan and a
/// working thread projects idle for the whole turn.
///
/// **The timeline.** The client absorbs a PromptInjected into the `MessageReceived` it
/// names via `injected_message_id` and re-anchors that exchange to the
/// ingestion point. Without the first orphan's PromptInjected, a follow-up that arrived
/// before a `ResponseCanceled` stays above the cancel boundary. It then renders
/// as done while it is the turn being worked on.
///
/// See `docs/plans/2026-08-04-chat-stop-honored-during-turn-setup.md`.
///
/// Returns the orphans it announced: one the user took back meanwhile is left
/// out, so the re-run never answers it.
async fn announce_orphan_batch(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    batch: Vec<InjectedPrompt>,
) -> Vec<InjectedPrompt> {
    let mut announced: Vec<InjectedPrompt> = Vec::with_capacity(batch.len());
    for orphan in batch {
        let base_meta = EventMeta {
            request_event_id: announced
                .first()
                .map_or(orphan.event_id, |first| first.event_id),
            ..EventMeta::NONE
        };
        if crate::engine::agentic_loop::emit_prompt_injected_event(
            bus, thread_id, &base_meta, &orphan,
        )
        .await
        {
            announced.push(orphan);
        }
    }
    announced
}

async fn admit_orphan_batch(
    engine: &SharedEngine,
    thread_id: Uuid,
    orphans: Vec<InjectedPrompt>,
) -> (Vec<InjectedPrompt>, Option<crate::engine::PreAdmission>) {
    use crate::engine::OrphanAdmission;
    let is_user_text = orphans
        .first()
        .is_some_and(|first| matches!(first.kind, crate::engine::InjectedPromptKind::UserText));
    if !is_user_text {
        return (orphans, None);
    }
    let (registration, batch) = match engine.hand_over_or_admit(thread_id, orphans) {
        OrphanAdmission::HandedOver => return (Vec::new(), None),
        OrphanAdmission::Admitted(registration, batch) => (registration, batch),
        OrphanAdmission::Closing(batch) => (engine.register_thread_queued(thread_id).await, batch),
    };
    let recover_engine = engine.clone();
    let pending = crate::engine::PreAdmission::new(registration, move |prompts| {
        recover_engine.resubmit_orphans_of_failed_turn(thread_id, prompts);
    });
    (batch, Some(pending))
}

/// INVARIANT: Re-process orphaned injections sequentially until the chain settles.
///
/// `process_message_with_steps` for a re-processed orphan can itself produce
/// orphans: a follow-up `MessageReceived` that landed on `injection_rx` after
/// the loop's final `try_recv()`. Iterating here keeps the chain bounded by the
/// available orphans, and serializes it per thread. One exception: a batch
/// whose turn fails re-submits its own orphans in a chain of their own, which
/// runs beside this one. Admission and injection still route each message
/// into exactly one turn. A fire-and-forget spawn per
/// orphan would drop the inner `orphaned_injections`, leaving an
/// orphan-of-orphan unanswered and stamping the next reply with the wrong
/// `request_event_id`.
pub(crate) async fn process_orphan_chain(
    engine: SharedEngine,
    thread_id: Uuid,
    initial_orphans: Vec<InjectedPrompt>,
) {
    drain_orphan_queue(initial_orphans, |orphans| {
        let engine = engine.clone();
        async move {
            let orphans = crate::engine::agentic_loop::filter_removed_queued_prompts(
                engine.pool(),
                thread_id,
                orphans,
            )
            .await;
            if orphans.is_empty() {
                return Vec::new();
            }
            match crate::engine::thread_agent(engine.pool(), thread_id).await {
                Ok(Some(ThreadAgent::LucidosAgent)) => {
                    process_lucidos_agent_orphans(&engine, thread_id, orphans).await
                }
                Ok(Some(ThreadAgent::CodingAgent)) => {
                    process_coding_agent_orphans(&engine, thread_id, orphans).await
                }
                Ok(None) => {
                    refuse_orphans(&engine, thread_id, orphans.len(), "it has no row").await
                }
                Err(e) => refuse_orphans(&engine, thread_id, orphans.len(), &e.to_string()).await,
            }
        }
    })
    .await;
}

/// INVARIANT: An unknown lane never runs the Lucidos Agent, which would spend a chat
/// credential on a coding-agent thread. The messages stay unanswered.
async fn refuse_orphans(
    engine: &SharedEngine,
    thread_id: Uuid,
    count: usize,
    why: &str,
) -> Vec<InjectedPrompt> {
    log_legacy!("[Chat] Could not read which agent runs thread {thread_id} ({why}), so {count} orphaned input(s) did not re-run");
    let error = "Lucidos could not tell which agent runs this thread, so a message \
                 the agent never read did not run. Send it again.";
    if let Err(e) = crate::engine::emit_routing_failure(&engine.event_bus, thread_id, error).await {
        log_legacy!("[Chat] Could not record the refused orphans on thread {thread_id}: {e}");
    }
    Vec::new()
}

/// INVARIANT: Re-run the first orphan and queue its batch before newer follow-ups.
/// Each message stays its own input and gets its own `CodingAgentPromptSent`.
async fn process_coding_agent_orphans(
    engine: &SharedEngine,
    thread_id: Uuid,
    orphans: Vec<InjectedPrompt>,
) -> Vec<InjectedPrompt> {
    let mut runnable = orphans
        .into_iter()
        .filter(|orphan| coding_agent_orphan_starter(&orphan.kind).is_some());
    let Some(first) = runnable.next() else {
        return Vec::new();
    };
    let starter = coding_agent_orphan_starter(&first.kind).expect("a runnable orphan");
    let followups = runnable
        .map(|orphan| crate::engine::AgentUserInput {
            text: orphan.text,
            images: orphan.images,
            origin_event_id: orphan.event_id,
            kind: if matches!(orphan.kind, crate::engine::InjectedPromptKind::UserText) {
                crate::engine::AgentInputKind::User
            } else {
                crate::engine::AgentInputKind::ReentryFromEngine
            },
            completion: None,
        })
        .collect();
    match engine
        .process_orphan(
            &first.text,
            first.images.as_deref(),
            thread_id,
            first.spawning_event_id,
            first.mode,
            first.event_id.map(starter),
            crate::engine::chat::process::OrphanLane::CodingAgent(followups),
        )
        .await
    {
        Ok(res) => res.orphaned_injections,
        Err(e) => {
            log_legacy!("[Chat] Failed to re-process orphaned inputs for coding-agent thread {thread_id}: {e}");
            Vec::new()
        }
    }
}

fn coding_agent_orphan_starter(
    kind: &crate::engine::InjectedPromptKind,
) -> Option<fn(Uuid) -> PreEmittedOrigin> {
    use crate::engine::InjectedPromptKind;
    match kind {
        InjectedPromptKind::UserText => Some(PreEmittedOrigin::Message),
        InjectedPromptKind::ReentryFromEngine => Some(PreEmittedOrigin::EngineReentry),
        InjectedPromptKind::ReentryFromWait => Some(PreEmittedOrigin::WaitReentry),
        InjectedPromptKind::SpokenAside | InjectedPromptKind::SpokenContinuation => None,
    }
}

async fn process_lucidos_agent_orphans(
    engine: &SharedEngine,
    thread_id: Uuid,
    orphans: Vec<InjectedPrompt>,
) -> Vec<InjectedPrompt> {
    let (mut orphans, admission) = admit_orphan_batch(engine, thread_id, orphans).await;
    if orphans
        .first()
        .is_some_and(|first| matches!(first.kind, crate::engine::InjectedPromptKind::UserText))
    {
        orphans = announce_orphan_batch(&engine.event_bus, thread_id, orphans).await;
    }
    let Some(first) = orphans.first() else {
        return Vec::new();
    };

    let (text, images, mode, spawning_event_id, origin_event_id) =
        if matches!(&first.kind, crate::engine::InjectedPromptKind::UserText) {
            (
                crate::engine::agentic_loop::coalesced_user_text_for_reprocess(&orphans),
                crate::engine::agentic_loop::coalesced_images_for_reprocess(&orphans),
                first.mode,
                first.spawning_event_id,
                first.event_id,
            )
        } else {
            (
                first.text.clone(),
                first.images.clone(),
                first.mode,
                first.spawning_event_id,
                first.event_id,
            )
        };

    match engine
        .process_orphan(
            &text,
            images.as_deref(),
            thread_id,
            spawning_event_id,
            mode,
            origin_event_id.map(PreEmittedOrigin::EngineReentry),
            crate::engine::chat::process::OrphanLane::LucidosAgent(admission),
        )
        .await
    {
        Ok(res) => res.orphaned_injections,
        Err(e) => {
            log_legacy!(
                "[Chat] Failed to re-process {} orphaned injection(s) for thread {}: {}",
                orphans.len(),
                thread_id,
                e
            );
            Vec::new()
        }
    }
}

async fn drain_orphan_queue<F, Fut>(initial: Vec<InjectedPrompt>, mut process_one: F)
where
    F: FnMut(Vec<InjectedPrompt>) -> Fut,
    Fut: std::future::Future<Output = Vec<InjectedPrompt>>,
{
    let mut queue: VecDeque<InjectedPrompt> = initial.into();
    while let Some(first) = queue.pop_front() {
        let mut batch = vec![first];
        if matches!(&batch[0].kind, crate::engine::InjectedPromptKind::UserText) {
            while queue
                .front()
                .is_some_and(|p| matches!(&p.kind, crate::engine::InjectedPromptKind::UserText))
            {
                if let Some(next) = queue.pop_front() {
                    batch.push(next);
                }
            }
        }

        let new_orphans = process_one(batch).await;
        queue.extend(new_orphans);
    }
}

fn resolve_file_ctx(
    file_context: Option<&super::FileContext>,
    repo_file_context: Option<&super::RepoFileContext>,
) -> Option<String> {
    file_context
        .map(|ctx| match ctx.lines {
            Some((start, end)) => format!("{}:{}-{}", ctx.path, start, end),
            None => ctx.path.clone(),
        })
        .or_else(|| {
            repo_file_context.map(|ctx| {
                if let Some((start, end)) = ctx.lines {
                    format!("[repo:{}] {}:{}-{}", ctx.repo_id, ctx.path, start, end)
                } else {
                    format!("[repo:{}] {}", ctx.repo_id, ctx.path)
                }
            })
        })
}

pub(crate) const PARENT_IS_THE_THREAD_ITSELF: &str = "\
`parent_thread_id` equals `thread_id`, so this request asks a thread to be its \
own parent. A thread family is a tree, and that row would put a cycle in it. \
Send the id of the thread that spawned this one, or drop `parent_thread_id` if \
this is a follow-up on the thread itself.";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedSpawn {
    thread_id: Option<Uuid>,
    parent_thread_id: Option<Uuid>,
    spawning_event_id: Option<Uuid>,
    declared_wake_mode: Option<WakeMode>,
    caller_thread_id: Option<Uuid>,
    caller_event_id: Option<Uuid>,
}

const WAKE_MODE_WITHOUT_A_PARENT: &str = "`wake_mode` needs `parent_thread_id`: only a child \
reports back to a parent, so a thread with none has no wake mode.";

fn validate_mode_and_spawn(request: &ChatRequest) -> Result<ValidatedSpawn, ApiError> {
    let has_caller = request.caller_workspace.is_some()
        || request.caller_thread_id.is_some()
        || request.caller_event_id.is_some();

    if has_caller && request.caller_workspace.is_none() {
        return Err(StatusCode::BAD_REQUEST.into());
    }

    let has_parent = request.parent_thread_id.is_some() || request.spawning_event_id.is_some();

    if has_caller && has_parent {
        return Err(StatusCode::BAD_REQUEST.into());
    }

    let thread_id = parse_optional_uuid(request.thread_id.as_deref())?;
    let caller_thread_id = parse_optional_uuid(request.caller_thread_id.as_deref())?;
    let caller_event_id = parse_optional_uuid(request.caller_event_id.as_deref())?;
    let parent_thread_id = parse_optional_uuid(request.parent_thread_id.as_deref())?;
    let spawning_event_id = parse_optional_uuid(request.spawning_event_id.as_deref())?;

    let declared_wake_mode =
        WakeMode::from_arg(request.wake_mode.as_deref()).map_err(ApiError::bad_request)?;
    if declared_wake_mode.is_some() && parent_thread_id.is_none() {
        return Err(ApiError::bad_request(WAKE_MODE_WITHOUT_A_PARENT));
    }

    if parent_thread_id.is_some() && parent_thread_id == thread_id {
        log_legacy!(
            "[Chat] Refusing self-parented thread {:?}: mode={:?}",
            thread_id,
            request.mode
        );
        return Err(ApiError::bad_request(PARENT_IS_THE_THREAD_ITSELF));
    }

    match request.mode {
        ActorMode::Human => {
            if parent_thread_id.is_some() || spawning_event_id.is_some() {
                return Err(StatusCode::BAD_REQUEST.into());
            }
        }
        ActorMode::Agent | ActorMode::Engine => {
            if parent_thread_id.is_none() && request.caller_workspace.is_none() {
                return Err(StatusCode::BAD_REQUEST.into());
            }
        }
    }

    Ok(ValidatedSpawn {
        thread_id,
        parent_thread_id,
        spawning_event_id,
        declared_wake_mode,
        caller_thread_id,
        caller_event_id,
    })
}

async fn refuse_unfit_integrated_spawn(
    pool: &sqlx::PgPool,
    parent_thread_id: Option<Uuid>,
    coding_agent: bool,
    app_folder: bool,
    repo_id: Option<&str>,
    thread_exists: bool,
    human_mode: bool,
) -> Result<(), ApiError> {
    use crate::engine::integrated::{integrated_spawn_refusal, IntegratedSpawn};
    let read_failed = |e: sqlx::Error| {
        log_legacy!("[Chat] integrated spawn check failed: {}", e);
        ApiError::from(StatusCode::INTERNAL_SERVER_ERROR)
    };
    let lucidos_repo = crate::core::repositories::RepositoryStore::get_by_name(
        pool,
        crate::engine::LucidosEngine::DEFAULT_REPO_NAME,
    )
    .await
    .map_err(|e| {
        log_legacy!("[Chat] integrated spawn: Lucidos repo lookup failed: {}", e);
        ApiError::from(StatusCode::INTERNAL_SERVER_ERROR)
    })?;
    let lucidos_source = !app_folder
        && repo_id.is_none_or(|id| {
            lucidos_repo
                .as_ref()
                .is_some_and(|r| r.id.to_string() == id)
        });
    let parent_is_lucidos_coding_agent = match parent_thread_id {
        Some(parent) => sqlx::query_scalar::<_, bool>(
            "SELECT is_coding_agent AND COALESCE(coding_agent_kind, 'lucidos') = 'lucidos' \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(parent)
        .fetch_optional(pool)
        .await
        .map_err(read_failed)?
        .unwrap_or(false),
        None => false,
    };
    match integrated_spawn_refusal(&IntegratedSpawn {
        has_parent: parent_thread_id.is_some(),
        coding_agent,
        lucidos_source,
        parent_is_lucidos_coding_agent,
        thread_exists,
        human_mode,
    }) {
        Some(refusal) => Err(ApiError::bad_request(refusal)),
        None => Ok(()),
    }
}

pub(super) fn validate_thread_continuity(
    existing_source: Option<&str>,
    existing_repo_id: Option<&str>,
    existing_coding_agent: Option<&str>,
    requested_use_cc: Option<bool>,
    requested_repo_id: Option<&str>,
    requested_coding_agent: Option<crate::runtime::CodingAgent>,
) -> Result<(), (StatusCode, String)> {
    let Some(source) = existing_source else {
        return Ok(());
    };
    let existing_is_cc = ThreadType::from_source(source) == ThreadType::CodingAgent;
    let requested_is_cc = requested_use_cc == Some(true);
    if existing_is_cc != requested_is_cc {
        let from = if existing_is_cc {
            "coding-agent"
        } else {
            "Lucidos Agent"
        };
        let to = if requested_is_cc {
            "coding-agent"
        } else {
            "Lucidos Agent"
        };
        return Err((
            StatusCode::CONFLICT,
            format!("Thread is locked to {from} mode; cannot switch to {to}"),
        ));
    }
    if existing_is_cc {
        if let (Some(req_repo), Some(existing_repo)) = (requested_repo_id, existing_repo_id) {
            if req_repo != existing_repo {
                return Err((
                    StatusCode::CONFLICT,
                    format!(
                        "Thread is locked to repo {existing_repo}; cannot switch to {req_repo}"
                    ),
                ));
            }
        }
        // INVARIANT: Backend lock: the new backend has no session to resume, so a flip
        // would silently lose the whole conversation context. NULL stored
        // value = legacy Claude Code thread (CodingAgent::parse semantics).
        if let Some(req_agent) = requested_coding_agent {
            let existing_agent =
                crate::runtime::CodingAgent::parse(existing_coding_agent.unwrap_or("claude-code"));
            if req_agent != existing_agent {
                return Err((
                    StatusCode::CONFLICT,
                    format!(
                        "Thread is locked to the {} coding agent; cannot switch to {}",
                        existing_agent.as_str(),
                        req_agent.as_str()
                    ),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
pub(super) struct ChatSubmitResponse {
    event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    compose: Option<crate::core::store::ComposeReport>,
}

#[derive(Deserialize)]
struct RemoveQueuedMessageRequest {
    thread_id: String,
    message_id: String,
}

fn already_read(recipient: &str) -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        format!("This message has already been delivered to {recipient}, so it can no longer be taken back."),
    )
    .with_reason("already_read")
}

async fn hook_submit_error(
    engine: &crate::engine::LucidosEngine,
    refusal: crate::engine::event_bus::HookRefusal,
    sender: Option<(Uuid, Uuid, &MessageOrigin)>,
) -> Result<ApiError, ApiError> {
    let reason = refusal.to_string();
    let Some(input) = refusal.input else {
        return Ok(ApiError::new(StatusCode::CONFLICT, reason).with_reason("hook_refused"));
    };
    if let Some((message, thread, actor)) = sender {
        engine
            .event_bus
            .hold_hook_input(message, thread, actor, &input.hook)
            .await
            .map_err(|error| ApiError::new(StatusCode::CONFLICT, error.to_string()))?;
    }
    Ok(ApiError::needs_hook_input(reason, *input))
}

async fn ordinary_submit_replay(
    engine: &crate::engine::LucidosEngine,
    intent: &crate::engine::event_bus::OrdinarySubmissionIntent,
    actor: &MessageOrigin,
) -> Result<Option<ChatSubmitResponse>, ApiError> {
    let receipt = engine
        .event_bus
        .ordinary_submission_receipt(intent, actor)
        .await
        .map_err(|error| {
            let message = error.to_string();
            if message == "ordinary_retry_identity_conflict" {
                ApiError::new(StatusCode::CONFLICT, message)
            } else {
                ApiError::internal(format!("Could not read the accepted input: {error}"))
            }
        })?;
    Ok(receipt.map(|receipt| ChatSubmitResponse {
        event_id: receipt.event_id.to_string(),
        compose: receipt.compose,
    }))
}

async fn remove_queued_message(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<RemoveQueuedMessageRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let thread_id = Uuid::parse_str(&request.thread_id)
        .map_err(|_| ApiError::bad_request(format!("Invalid thread_id {:?}", request.thread_id)))?;
    let message_id = Uuid::parse_str(&request.message_id).map_err(|_| {
        ApiError::bad_request(format!("Invalid message_id {:?}", request.message_id))
    })?;
    crate::api::thread_reach::refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(thread_id),
        crate::engine::permission::Action::Control,
    )
    .await?;

    let already_removed =
        crate::engine::consumption_claim::was_removed(&state.pool, thread_id, message_id)
            .await
            .map_err(ApiError::db)?;
    if already_removed {
        return queued_removal_response(&state.pool, thread_id, message_id).await;
    }

    let actor = super::actor::user_actor(&headers, None);
    let accepted_request: Option<
        sqlx::types::Json<crate::engine::thread_queue::ThreadQueueRequest>,
    > = sqlx::query_scalar(
        "SELECT request FROM thread_queue WHERE thread_id=$1 AND request->>'event_id'=$2",
    )
    .bind(thread_id)
    .bind(message_id.to_string())
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::db)?;
    let engine_command =
        accepted_request.is_some_and(|request| request.0.coding_agent_command().is_some());
    if !engine_command
        && crate::engine::thread_agent(&state.pool, thread_id)
            .await
            .map_err(ApiError::db)?
            == Some(ThreadAgent::CodingAgent)
    {
        return match state
            .engine
            .withdraw_coding_agent_input(thread_id, message_id, actor)
            .await
        {
            InputWithdrawal::Withdrawn => {
                queued_removal_response(&state.pool, thread_id, message_id).await
            }
            InputWithdrawal::AlreadyRead
                if crate::engine::consumption_claim::was_removed(
                    &state.pool,
                    thread_id,
                    message_id,
                )
                .await
                .map_err(ApiError::db)? =>
            {
                queued_removal_response(&state.pool, thread_id, message_id).await
            }
            InputWithdrawal::AlreadyRead => Err(already_read("the coding agent")),
            InputWithdrawal::Refused(why) => {
                Err(ApiError::new(StatusCode::CONFLICT, why).with_reason("withdraw_refused"))
            }
        };
    }

    use crate::engine::consumption_claim::{take_back_message, TakeBackOutcome};
    match take_back_message(&state.engine.event_bus, thread_id, message_id, actor).await {
        Ok(TakeBackOutcome::Removed | TakeBackOutcome::AlreadyRemoved) => {
            queued_removal_response(&state.pool, thread_id, message_id).await
        }
        Ok(TakeBackOutcome::Consumed) => Err(already_read("its recipient")),
        Err(error) => Err(ApiError::internal(format!(
            "Could not record the removal: {error}"
        ))),
    }
}

/// INVARIANT: An idempotent removal may have been won by Stop, which already restored
/// compose. Edit must reconcile that report instead of appending again.
async fn queued_removal_response(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    message_id: Uuid,
) -> Result<Json<serde_json::Value>, ApiError> {
    let compose: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'compose' FROM events WHERE thread_id=$1 \
         AND event_type='QueuedMessageRemoved' AND payload->>'removed_message_id'=$2",
    )
    .bind(thread_id)
    .bind(message_id.to_string())
    .fetch_optional(pool)
    .await
    .map_err(ApiError::db)?
    .flatten();
    Ok(Json(serde_json::json!({ "compose": compose })))
}

async fn message_was_already_accepted(
    engine: &crate::engine::LucidosEngine,
    event_id: Uuid,
) -> Result<bool, ApiError> {
    let recorded = crate::engine::chat_event_id_is_recorded(engine.pool(), event_id)
        .await
        .map_err(|e| {
            ApiError::internal(format!(
                "Could not check whether message {event_id} was already recorded: {e}"
            ))
        })?;
    Ok(recorded || !engine.accepted_messages.admit(event_id))
}

/// INVARIANT: POST endpoint for chat with progress updates.
/// Returns immediately with an `event_id` (the response's only field; the
/// legacy `message_id` name survives solely as a request-body serde alias on
/// `ChatRequest::event_id`). All progress events are sent via the global SSE
/// stream as ThreadEvent events. Human inputs acknowledge durable acceptance;
/// their receipt replays before mutable settings or draft fences. Recording
/// remains execution-time work, including question and coding-agent routing.
pub(super) async fn chat_submit(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(mut request): Json<ChatRequest>,
) -> Result<Json<ChatSubmitResponse>, ApiError> {
    // INVARIANT: Capture frontend origin from the browser Origin header so the LLM
    // system prompt can show the user-facing Lucidos URL. Skip cross-workspace
    // calls (a cross-workspace caller — `caller_workspace` set in body) so a
    // server-to-server caller can't poison the cache with an unrelated host.
    if request.caller_workspace.is_none() {
        if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
            let mut stored = state.engine.frontend_origin.lock().unwrap();
            if stored.is_none() {
                *stored = Some(origin.to_string());
            }
        }
    }
    // INVARIANT: The human-attribution gate runs before every other check, so a refusal
    // writes nothing at all. See `human_mode_is_attributed` for why it sits in
    // front of the (stricter, but opt-in) subprocess gate further down.
    require_human_mode_is_attributed(&state.pool, &headers, &request).await?;
    let wire_intent =
        serde_json::to_value(&request).map_err(|error| ApiError::internal(error.to_string()))?;

    let ValidatedSpawn {
        thread_id: target_thread_id,
        parent_thread_id,
        spawning_event_id,
        caller_thread_id,
        caller_event_id,
        declared_wake_mode,
    } = validate_mode_and_spawn(&request)?;
    let mode = request.mode;
    let wake_mode = match parent_thread_id {
        Some(parent) => Some(WakeMode::for_spawn(
            declared_wake_mode,
            crate::engine::home_thread::spawner_is_home(&state.pool, parent).await,
        )),
        None => None,
    };

    // INVARIANT: The target's projection row, read ONCE and used twice: by the subprocess
    // gate, which needs to know whether the target exists, and by the mode/repo
    // continuity lock further down. Reading it here is what lets the gate run
    // before any work happens.
    //
    // Keyed on `target_thread_id`, the id the CALLER named. The continuity
    // check's `thread_id` is the same value in every case that can match a
    // row: it is `target_thread_id` outright on two branches, and on the app
    // branch it is `target_thread_id.unwrap_or_else(Uuid::new_v4)`, whose
    // fresh uuid can never be in `thread_summaries`.
    let existing_thread_row: Option<(String, String, Option<String>, Option<String>)> =
        match target_thread_id {
            Some(tid) => sqlx::query_as(
                "SELECT state, source, cc_repo_id, coding_agent \
                 FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(tid)
            .fetch_optional(state.engine.pool())
            .await
            .map_err(|e| {
                log_legacy!("[Chat] thread_summaries lookup failed for {}: {}", tid, e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
            None => None,
        };
    let thread_exists = existing_thread_row.is_some();
    if thread_exists && super::workspace_operations::routed_origin().is_some() {
        return Err(super::workspace_operations::existing_routed_spawn());
    }

    let input_position = InputPosition::from_wire(
        request.input_chain.as_deref(),
        request.input_seq,
        request
            .event_id
            .as_deref()
            .and_then(|raw| Uuid::parse_str(raw).ok()),
    )
    .map_err(ApiError::bad_request)?;
    let admitted_input = match target_thread_id {
        Some(tid)
            if !thread_exists && input_position.is_some() && request.new_thread != Some(true) =>
        {
            InputOrder::thread_has_admitted_input(state.engine.pool(), tid)
                .await
                .map_err(|e| {
                    ApiError::internal(format!(
                        "Could not check whether thread {tid} has an admitted input: {e}"
                    ))
                })?
        }
        _ => false,
    };

    if let Some(tid) = target_thread_id {
        if !thread_target_is_addressable(
            thread_exists,
            admitted_input,
            request.new_thread,
            parent_thread_id,
            request.caller_workspace.is_some(),
        ) {
            log_legacy!(
                "[Chat] Refusing to address unknown thread {} (no create signal): mode={:?}, user_agent={:?}",
                tid,
                mode,
                headers.get("user-agent").and_then(|v| v.to_str().ok())
            );
            return Err(ApiError::not_found(unknown_thread_message(tid)));
        }
    }
    let has_started = thread_has_started(
        existing_thread_row
            .as_ref()
            .map(|(state, ..)| state.as_str()),
    );

    if subprocess_gate_applies(request.caller_workspace.is_some(), thread_exists, mode) {
        if let crate::core::subprocess_origin::SubprocessOrigin::Subprocess {
            source_thread_id,
            ..
        } = crate::core::subprocess_origin::subprocess_origin(&headers)
        {
            if !subprocess_chat_legitimate(
                mode,
                source_thread_id,
                target_thread_id,
                parent_thread_id,
                thread_exists,
            ) {
                log_legacy!(
                    "[Chat] Rejecting subprocess chat POST: mode={:?}, source_thread={:?}, target_thread={:?}, parent_thread={:?}, target_exists={}",
                    mode,
                    source_thread_id,
                    target_thread_id,
                    parent_thread_id,
                    thread_exists
                );
                return Err(StatusCode::FORBIDDEN.into());
            }
        }
    }

    let engine_clone = state.engine.clone();
    let message = request.message.clone();
    let model = request.model.clone();
    let reasoning_effort = request.reasoning_effort.clone();
    let provider = request.provider.clone();
    let device_id = sending_device_id(&request, &headers).map(str::to_string);

    let parent_thread_title = match parent_thread_id {
        Some(ptid) => state
            .engine
            .event_store()
            .get_thread_title(ptid)
            .await
            .unwrap_or_else(|e| {
                log_legacy!(
                    "[Chat] Failed to load parent thread title for {}: {}",
                    ptid,
                    e
                );
                None
            }),
        None => None,
    };

    let caller =
        request
            .caller_workspace
            .as_ref()
            .map(|workspace| crate::api::actor::CallerOrigin {
                workspace: workspace.clone(),
                thread_id: caller_thread_id,
                event_id: caller_event_id,
                mode,
            });

    let origin = build_message_origin(
        &headers,
        mode,
        device_id.as_deref(),
        parent_thread_id,
        parent_thread_title,
        spawning_event_id,
        caller,
    );
    origin_agrees_with_mode(origin.as_ref(), mode)?;

    // INVARIANT: An accepted retry is independent of today's provider registry, running
    // rung, image blobs and compose slot. Authenticate first, then compare the
    // original wire intent before resolving any of those mutable inputs.
    if mode == ActorMode::Human {
        if let (Some(tid), Some(id), Some(actor)) = (
            target_thread_id,
            request
                .event_id
                .as_deref()
                .and_then(|id| Uuid::parse_str(id).ok()),
            origin.as_ref(),
        ) {
            let intent =
                crate::engine::event_bus::OrdinarySubmissionIntent::new(tid, id, &wire_intent)
                    .map_err(|error| ApiError::bad_request(error.to_string()))?;
            if let Some(replay) = ordinary_submit_replay(&state.engine, &intent, actor).await? {
                return Ok(Json(replay));
            }
            // INVARIANT: Inputs recorded before durable receipts existed keep their
            // original idempotency contract. This read claims no new id and
            // cannot consume a newer draft. A different target is a conflict.
            if crate::engine::chat_event_id_is_recorded(state.engine.pool(), id)
                .await
                .map_err(ApiError::db)?
            {
                let same_thread: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM events WHERE id=$1 AND thread_id=$2)",
                )
                .bind(id)
                .bind(tid)
                .fetch_one(state.engine.pool())
                .await
                .map_err(ApiError::db)?;
                if !same_thread {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "ordinary_retry_identity_conflict",
                    ));
                }
                return Ok(Json(ChatSubmitResponse {
                    event_id: id.to_string(),
                    compose: None,
                }));
            }
        }
    }

    if request.use_coding_agent == Some(true)
        && matches!(
            origin,
            Some(MessageOrigin::UnattestedCaller { .. })
                | Some(MessageOrigin::Workspace {
                    verdict: Some(crate::engine::thread_events::SpawnVerdict::Unattested),
                    ..
                })
        )
    {
        return Err(super::owner_door::unattested());
    }

    refuse_unroutable_provider(
        &state,
        request.model.as_deref(),
        request.provider.as_deref(),
    )?;

    let chat_thread = existing_thread_row
        .as_ref()
        .filter(|(_, source, ..)| source != crate::engine::thread_lifecycle::CODING_AGENT_SOURCE)
        .and(target_thread_id);
    if let (Some(tid), false) = (chat_thread, request.use_coding_agent == Some(true)) {
        let overrides = crate::core::ResolvedModelSelection {
            model: request.model.clone(),
            reasoning_effort: request.reasoning_effort.clone(),
            provider: request.provider.clone(),
        };
        if state
            .engine
            .mid_turn_override_differs(tid, overrides)
            .await
            .map_err(|e| ApiError::internal(format!("Could not read thread {tid}: {e}")))?
        {
            return Err(turn_running_refusal());
        }
    }

    let app_ctx = request.app_context;
    let file_ctx = resolve_file_ctx(
        request.file_context.as_ref(),
        request.repo_file_context.as_ref(),
    );
    let url_ctx = request.url_context;
    // INVARIANT: Every image reaches the blob store before anything reads it. Inline
    // `images` are stored exactly as an upload is. So one sniff, one allowlist
    // and one refusal cover both wire fields, and a refusal lands before
    // anything has started. `image_hashes` wins when a caller sends both.
    //
    // The hashes then resolve to ChatImage, at original resolution and with
    // the sniffed mime. `ChatImage::fit_for_llm` fits them at the LLM
    // boundary, so compression lives in exactly one place. The Thread Queue
    // branch below persists these hashes, never inline base64.
    let supplied_image_hashes = match (request.image_hashes.take(), request.images.take()) {
        (Some(hashes), _) => Some(hashes),
        (None, Some(images)) => {
            Some(super::blobs::store_inline_images(state.engine.workspace_path(), images).await?)
        }
        (None, None) => None,
    };
    let chat_images = supplied_image_hashes
        .as_ref()
        .map(|hashes| state.engine.resolve_message_image_hashes(hashes))
        .transpose()
        .map_err(|error| ApiError::bad_request(error.to_string()))?
        .flatten();
    require_input_to_create(
        has_started,
        &message,
        chat_images.as_deref().unwrap_or_default(),
    )?;
    let use_coding_agent = request.use_coding_agent;
    let cc_model = request.cc_model;
    let coding_agent = request.coding_agent;
    if coding_agent.is_some() && use_coding_agent != Some(true) {
        log_legacy!("[Chat] chat_submit received `coding_agent` without `use_coding_agent=true` — rejecting");
        return Err(StatusCode::BAD_REQUEST.into());
    }
    require_offered_coding_agent_model(
        has_started,
        use_coding_agent,
        coding_agent,
        cc_model.as_deref(),
    )?;
    if !has_started && super::intelligence_ladder::is_agent_caller(&headers, origin.as_ref()) {
        let (surface, picked_model) = match use_coding_agent {
            Some(true) => (
                coding_agent
                    .unwrap_or(crate::runtime::CodingAgent::ClaudeCode)
                    .into(),
                cc_model.as_deref(),
            ),
            _ => (
                crate::runtime::intelligence_ladder::LadderSurface::Chat,
                model.as_deref(),
            ),
        };
        super::intelligence_ladder::refuse_off_ladder(
            &state,
            surface,
            picked_model,
            reasoning_effort.as_deref(),
        )
        .await?;
    }
    let event_id = request
        .event_id
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let event_uuid =
        Uuid::parse_str(&event_id).map_err(|_| ApiError::bad_request("event_id must be a UUID"))?;
    let thread_id = target_thread_id;
    let conflict_change_id = parse_optional_uuid(request.conflict_change_id.as_deref())?;
    let repo_id = match request.repo_id.as_deref() {
        Some(s) if !s.is_empty() && Uuid::parse_str(s).is_err() => {
            match crate::core::repositories::RepositoryStore::get_by_name(state.engine.pool(), s)
                .await
            {
                Ok(Some(repo)) => Some(repo.id.to_string()),
                Ok(None) => {
                    log_legacy!("[Chat] Unknown repo name '{}' in chat_submit", s);
                    return Err(StatusCode::BAD_REQUEST.into());
                }
                Err(e) => {
                    log_legacy!("[Chat] Failed to look up repo '{}': {}", s, e);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR.into());
                }
            }
        }
        _ => request.repo_id,
    };

    let folder_in = request
        .folder
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (repo_id, thread_id, app_id_to_stash) = if let Some(folder_str) = folder_in {
        if repo_id.as_deref().is_some_and(|s| !s.is_empty()) {
            log_legacy!("[Chat] chat_submit received both `folder` and `repo_id` — rejecting");
            return Err(StatusCode::BAD_REQUEST.into());
        }
        if use_coding_agent != Some(true) {
            log_legacy!(
                "[Chat] chat_submit received `folder` without `use_coding_agent=true` — rejecting"
            );
            return Err(StatusCode::BAD_REQUEST.into());
        }
        // INVARIANT: Canonicalize workspace_root too — classify_resolved_folder's
        // `.starts_with(workspace_root.join("data"))` check uses this prefix
        // and `folder_abs` is already canonicalized, so a symlinked
        // workspace (macOS LUCIDOS_WORKSPACE=/var/folders/T/... canonicalizes
        // to /private/var/...) would otherwise miss the App branch and
        // 400 with "not the Lucidos source repo".
        let workspace_root = {
            let raw = state.engine.workspace_path().to_path_buf();
            match std::fs::canonicalize(&raw) {
                Ok(p) => p,
                Err(e) => {
                    log_legacy!("[Chat] canonicalize workspace_root {:?} failed ({}); falling back to raw path — symlinked workspaces may misclassify", raw, e);
                    raw
                }
            }
        };
        let registered_repos =
            match crate::core::repositories::RepositoryStore::list(state.engine.pool()).await {
                Ok(v) => v,
                Err(e) => {
                    log_legacy!("[Chat] failed to list repos for folder resolution: {}", e);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR.into());
                }
            };
        // INVARIANT: Canonicalize each registered path once. Both `resolve_folder_input`
        // and `classify_resolved_folder` canonicalize their working
        // `folder_abs`. Without the same treatment here, a registered repo
        // under a symlinked ancestor would miss the External branch and fall
        // through to the unrecognised-path refusal.
        let registered_canonical: Vec<(uuid::Uuid, PathBuf)> = registered_repos
            .iter()
            .map(|r| {
                let raw = PathBuf::from(&r.path);
                let canon = match std::fs::canonicalize(&raw) {
                    Ok(p) => p,
                    Err(e) => {
                        log_legacy!("[Chat] canonicalize repo {} path {:?} failed ({}); using raw — match may miss for symlinked / deleted paths", r.id, raw, e);
                        raw
                    }
                };
                (r.id, canon)
            })
            .collect();
        let lucidos_repo_root = registered_repos
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(crate::engine::LucidosEngine::DEFAULT_REPO_NAME))
            .map(|repo| {
                let raw = PathBuf::from(&repo.path);
                match std::fs::canonicalize(&raw) {
                    Ok(p) => p,
                    Err(e) => {
                        log_legacy!("[Chat] canonicalize Lucidos repo path {:?} failed ({}); using raw — Lucidos classification may misfire", raw, e);
                        raw
                    }
                }
            });
        let lookup_repo_path = |id_or_name: &str| -> Result<Option<PathBuf>, String> {
            if let Ok(uuid) = Uuid::parse_str(id_or_name) {
                if let Some(r) = registered_repos.iter().find(|r| r.id == uuid) {
                    return Ok(Some(PathBuf::from(&r.path)));
                }
            }
            Ok(registered_repos
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case(id_or_name))
                .map(|r| PathBuf::from(&r.path)))
        };
        let folder_abs = match crate::engine::agent_session::coding_agent_kind::resolve_folder_input(
            folder_str,
            &workspace_root,
            lookup_repo_path,
        ) {
            Ok(p) => p,
            Err(e) => {
                log_legacy!("[Chat] folder `{}` failed to resolve: {}", folder_str, e);
                return Err(StatusCode::BAD_REQUEST.into());
            }
        };
        let external_repo_match = |path: &std::path::Path| -> Option<PathBuf> {
            registered_canonical.iter().find_map(|(_, canon)| {
                if canon == path {
                    Some(canon.clone())
                } else {
                    None
                }
            })
        };
        use crate::engine::agent_session::coding_agent_kind::{
            classify_resolved_folder, FolderClassification,
        };
        let classification = match classify_resolved_folder(
            &folder_abs,
            &workspace_root,
            lucidos_repo_root.as_deref(),
            external_repo_match,
        ) {
            Ok(c) => c,
            Err(e) => {
                log_legacy!(
                    "[Chat] folder `{}` classification failed: {}",
                    folder_str,
                    e
                );
                return Err(StatusCode::BAD_REQUEST.into());
            }
        };
        match classification {
            FolderClassification::Lucidos { .. } => (None, thread_id, None),
            FolderClassification::External { repo_root } => {
                let repo = registered_canonical
                    .iter()
                    .find(|(_, canon)| canon == &repo_root);
                let repo_uuid = match repo {
                    Some((id, _)) => id.to_string(),
                    None => {
                        log_legacy!(
                            "[Chat] external repo {:?} not found in canonical snapshot — \
                             external_repo_match invariant violated",
                            repo_root
                        );
                        return Err(StatusCode::INTERNAL_SERVER_ERROR.into());
                    }
                };
                (Some(repo_uuid), thread_id, None)
            }
            FolderClassification::App { app_id, .. } => {
                let tid = thread_id.unwrap_or_else(Uuid::new_v4);
                (None, Some(tid), Some((tid, app_id)))
            }
        }
    } else {
        (repo_id, thread_id, None)
    };

    if let (Some(tid), Some((state_str, source, existing_repo, existing_agent))) =
        (thread_id, existing_thread_row)
    {
        let existing_state = ThreadState::from_db_str(&state_str).map_err(|e| {
            log_legacy!("[Chat] thread_summaries.state for {} invalid: {}", tid, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        if !existing_state.can_change_mode() {
            if let Err((sc, msg)) = validate_thread_continuity(
                Some(&source),
                existing_repo.as_deref(),
                existing_agent.as_deref(),
                use_coding_agent,
                repo_id.as_deref(),
                coding_agent,
            ) {
                log_legacy!("[Chat] Reject follow-up on thread {}: {}", tid, msg);
                return Err(ApiError::new(sc, msg));
            }
        }
    }

    if request.integrated {
        refuse_unfit_integrated_spawn(
            state.engine.pool(),
            parent_thread_id,
            use_coding_agent == Some(true),
            app_id_to_stash.is_some(),
            repo_id.as_deref(),
            thread_exists,
            mode == ActorMode::Human,
        )
        .await?;
    }

    // INVARIANT: Checked before the dispatch, so a refused mention records nothing.
    let user_addressing = super::user_only_messages::addressing_of_send(
        &state.pool,
        thread_id,
        request.mentions,
        request.agent_mentioned,
        mode,
        origin.as_ref(),
    )
    .await?;
    super::user_only_messages::refuse_user_only_start(
        user_addressing.as_ref(),
        use_coding_agent == Some(true) && !has_started,
    )?;

    let request_title = request.title;
    let hook_args = request.hook_args;
    let request_integrated = request.integrated;
    let spawning_trigger_id = match crate::core::subprocess_origin::subprocess_origin(&headers) {
        crate::core::subprocess_origin::SubprocessOrigin::Subprocess {
            emitting_trigger_id,
            ..
        } => emitting_trigger_id,
        crate::core::subprocess_origin::SubprocessOrigin::NotSubprocess => None,
    };
    if mode == ActorMode::Human {
        let tid = thread_id.unwrap_or_else(Uuid::new_v4);
        let actor = origin
            .clone()
            .ok_or_else(|| ApiError::bad_request("The input has no authenticated author"))?;
        let intent =
            crate::engine::event_bus::OrdinarySubmissionIntent::new(tid, event_uuid, &wire_intent)
                .map_err(|error| ApiError::bad_request(error.to_string()))?;
        let queue_request = crate::engine::thread_queue::ThreadQueueRequest::AgentChat {
            hook_args,
            message,
            user_addressing: user_addressing.map(Box::new),
            thread_id: tid,
            depth: 0,
            event_id: Some(event_id),
            image_hashes: supplied_image_hashes.unwrap_or_default(),
            device_id,
            app_context: app_ctx,
            file_context: file_ctx,
            url_context: url_ctx.map(Box::new),
            conflict_change_id,
            model,
            reasoning_effort,
            provider,
            use_coding_agent,
            repo_id,
            cc_model,
            coding_agent,
            title: request_title,
            mode,
            origin,
            parent_thread_id,
            spawning_event_id,
            wake_mode,
            app_id: app_id_to_stash.map(|(_, app_id)| app_id),
            integrated: request_integrated,
            spawning_trigger_id,
        };
        if !has_started {
            if let Err(refusal) =
                crate::engine::hook_input::check(&state.engine, &queue_request).await
            {
                let error =
                    hook_submit_error(&state.engine, refusal, Some((event_uuid, tid, &actor)))
                        .await;
                if let Some(replay) = ordinary_submit_replay(&state.engine, &intent, &actor).await?
                {
                    return Ok(Json(replay));
                }
                return Err(error?);
            }
        }
        let acceptor = state.engine.clone();
        let input_chain = request.input_chain;
        let (input_seq, compose_epoch) = (request.input_seq, request.compose_epoch);
        let accepted = tokio::spawn(async move {
            acceptor
                .accept_ordinary_submission(
                    queue_request,
                    actor,
                    input_chain.as_deref(),
                    input_seq,
                    compose_epoch,
                    intent,
                )
                .await
        })
        .await
        .map_err(|error| ApiError::internal(format!("The input acceptance failed: {error}")))?
        .map_err(|error| {
            let reason = error.to_string();
            match reason.as_str() {
                "overtaken" => ApiError::new(StatusCode::CONFLICT, overtaken_message(tid)).with_reason(OVERTAKEN_REASON),
                "compose_snapshot_stale" => ApiError::new(StatusCode::CONFLICT, "The saved draft changed before this send was accepted. Its content was not sent.").with_reason("compose_snapshot_stale"),
                "compose_fence_required" => ApiError::new(StatusCode::CONFLICT, "This send has no draft epoch after a saved-draft submission. Its content was not sent.").with_reason("compose_fence_required"),
                "hook_input_overtaken" => ApiError::new(StatusCode::CONFLICT, "Another message reached this thread first. Return your message to the composer.").with_reason("hook_input_overtaken"),
                "ordinary_retry_identity_conflict" => ApiError::new(StatusCode::CONFLICT, reason),
                _ => ApiError::internal(format!("Could not accept the input: {error}")),
            }
        })?;
        let engine = state.engine.clone();
        tokio::spawn(async move {
            engine.thread_queue.drain().await;
        });
        return Ok(Json(ChatSubmitResponse {
            event_id: accepted.receipt.event_id.to_string(),
            compose: accepted.receipt.compose,
        }));
    }
    let routed_spawn = super::workspace_operations::routed_origin().is_some();
    let dispatch = tokio::spawn(async move {
        let engine = state.engine.clone();
        let routed_thread = if routed_spawn {
            Some(thread_id.ok_or_else(|| {
                ApiError::bad_request("A routed spawn requires a thread identity.")
            })?)
        } else {
            None
        };
        // INVARIANT: Re-asks before admission must not record a refusal for a
        // spawn already accepted. Retries wait for the first request to settle.
        let spawn_preflight = if mode == ActorMode::Agent && !thread_exists {
            Some(
                engine
                    .accepted_messages
                    .serialize_spawn_preflight(routed_thread.unwrap_or(event_uuid))
                    .await,
            )
        } else {
            None
        };
        let ask_spawn_hooks = spawn_preflight.is_some()
            && !matches!(
                engine
                    .event_bus
                    .hooks_armed_on(crate::core::event_hooks::FIRST_MESSAGE_EVENT_TYPE)
                    .await,
                Ok(false)
            );
        if ask_spawn_hooks {
            if let Some(thread) = routed_thread {
                if super::workspace_operations::has_spawn_record(engine.pool(), thread).await? {
                    return Err(super::workspace_operations::existing_routed_spawn());
                }
            } else if engine.accepted_messages.contains(event_uuid)
                || crate::engine::chat_event_id_is_recorded(engine.pool(), event_uuid)
                    .await
                    .map_err(ApiError::db)?
            {
                return Ok(Json(ChatSubmitResponse {
                    event_id,
                    compose: None,
                }));
            }
        }
        let (mut app_ctx, mut file_ctx, mut url_ctx) = (app_ctx, file_ctx, url_ctx);
        let (mut hook_args, mut app_id_to_stash) = (hook_args, app_id_to_stash);
        let new_thread = if !thread_exists {
            let queue_thread_id = thread_id.unwrap_or_else(Uuid::new_v4);
            let image_hashes = supplied_image_hashes.unwrap_or_default();
            let queue_request = crate::engine::thread_queue::ThreadQueueRequest::AgentChat {
                hook_args: std::mem::take(&mut hook_args),
                user_addressing: None,
                depth: 0,
                message: message.clone(),
                thread_id: queue_thread_id,
                event_id: Some(event_id.clone()),
                image_hashes,
                device_id: device_id.clone(),
                app_context: app_ctx.take(),
                file_context: file_ctx.take(),
                url_context: url_ctx.take().map(Box::new),
                conflict_change_id,
                model: model.clone(),
                reasoning_effort: reasoning_effort.clone(),
                provider: provider.clone(),
                use_coding_agent,
                repo_id: repo_id.clone(),
                cc_model: cc_model.clone(),
                coding_agent,
                title: request_title.clone(),
                mode,
                origin: origin.clone(),
                parent_thread_id,
                spawning_event_id,
                wake_mode,
                app_id: app_id_to_stash.take().map(|(_, app_id)| app_id),
                integrated: request_integrated,
                spawning_trigger_id,
            };
            if ask_spawn_hooks {
                let result = if routed_spawn {
                    crate::engine::hook_input::check_with_budget(
                        &engine,
                        &queue_request,
                        super::workspace_operations::ROUTED_HOOK_BUDGET,
                    )
                    .await
                } else {
                    crate::engine::hook_input::check(&engine, &queue_request).await
                };
                if let Err(refusal) = result {
                    return Err(hook_submit_error(&engine, refusal, None).await?);
                }
            }
            Some((queue_thread_id, queue_request))
        } else {
            None
        };
        // INVARIANT: Keep the reservation with detached submission, including on disconnect.
        let _routed_spawn_guard = if let Some(thread) = routed_thread {
            Some(super::workspace_operations::reserve_spawn(engine.pool(), thread).await?)
        } else {
            None
        };
        // INVARIANT: Fix this input's place in the thread's order before the ack. An input
        // a later one from its page overtook is refused here, never recorded.
        // It runs before the dedupe below. A refused send then never claims an
        // event id that a Retry of the same message gets acked against.
        let mut input_turn = match thread_id {
            Some(tid) => match engine
                .input_order
                .admit(engine.pool(), tid, input_position, Some(event_uuid))
                .await
            {
                Ok(InputAdmission::Admitted(turn)) => Some(turn),
                Ok(InputAdmission::Overtaken) => {
                    log_legacy!(
                        "[Chat] Refusing an overtaken input on thread {}: {:?}",
                        tid,
                        input_position
                    );
                    return Err(ApiError::new(StatusCode::CONFLICT, overtaken_message(tid))
                        .with_reason(OVERTAKEN_REASON));
                }
                Err(e) => {
                    return Err(ApiError::internal(format!(
                        "Could not admit the message to thread {tid}: {e}"
                    )));
                }
            },
            None => None,
        };

        // INVARIANT: A re-post of a message this engine already accepted gets the same ack
        // and starts nothing. Placed after every refusal, so a refused request
        // leaves its id free for the corrected retry. A failure further down that
        // starts nothing forgets `accepted_event_id` for the same reason.
        if message_was_already_accepted(&engine, event_uuid).await? {
            log_legacy!(
                "[Chat] Event {} was already accepted; acking the re-post",
                event_uuid
            );
            return Ok(Json(ChatSubmitResponse {
                event_id,
                compose: None,
            }));
        }
        let accepted_event_id = event_uuid;

        if let Some((queue_thread_id, queue_request)) = new_thread {
            let outcome = engine.thread_queue.submit(queue_request, origin).await;
            if routed_spawn
                && !super::workspace_operations::has_spawn_record(engine.pool(), queue_thread_id)
                    .await?
            {
                return Err(ApiError::internal("The spawn may have started but durable acceptance could not be verified. Inspect the requested thread before retrying."));
            }
            if !outcome.admitted {
                log_legacy!(
                    "[Chat] Agent-mode spawn queued at position {} (thread {})",
                    outcome.position,
                    queue_thread_id
                );
            }
            return Ok(Json(ChatSubmitResponse {
                event_id,
                compose: None,
            }));
        }

        let compose = None;
        let thread_id = thread_id.unwrap_or_else(Uuid::new_v4);
        // INVARIANT: Stash the app spawn AFTER every early-return gate above. An entry
        // stamped earlier would leak in the in-memory map when a gate rejects:
        // `run_direct_agent` is the only popper, and it is never reached on the
        // 4xx/5xx paths.
        // On mutex poisoning the handler returns a 500, matching the rest of the
        // engine's `std::sync::Mutex` guard shape rather than crashing the worker.
        if let Some((tid, app_id)) = app_id_to_stash {
            match engine.pending_app_spawn.lock() {
                Ok(mut guard) => {
                    guard.insert(tid, app_id);
                }
                Err(e) => {
                    log_legacy!("[Chat] pending_app_spawn poisoned, cannot stash: {}", e);
                    engine.accepted_messages.forget(accepted_event_id);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR.into());
                }
            }
        }

        let title = request_title;
        let response_event_id = event_id.clone();

        let pre_emitted_origin: Option<PreEmittedOrigin> = None;

        let engine_for_panic = engine.clone();
        let thread_id_for_panic = thread_id;
        let actor_for_apply = origin.clone();
        let handle = tokio::spawn(async move {
            // INVARIANT: Wait for this input's turn BEFORE taking a capacity slot, so a later
            // input never holds a slot idle while an earlier one waits.
            if let Some(turn) = input_turn.as_mut() {
                turn.wait_for_predecessor().await;
            }
            let _user_slot = {
                let summary = crate::engine::thread_queue::truncate_summary(message.trim());
                engine_clone
                    .thread_queue
                    .acquire_user_slot(thread_id, summary)
                    .await
            };
            let result = engine_clone
                .process_message_with_steps(
                    &message,
                    model.as_deref(),
                    app_ctx,
                    file_ctx,
                    reasoning_effort.as_deref(),
                    provider.as_deref(),
                    chat_images.as_deref(),
                    device_id.as_deref(),
                    use_coding_agent,
                    Some(event_id.as_str()),
                    Some(thread_id),
                    conflict_change_id,
                    repo_id.as_deref(),
                    url_ctx,
                    parent_thread_id,
                    spawning_event_id,
                    wake_mode,
                    mode,
                    cc_model.as_deref(),
                    coding_agent,
                    pre_emitted_origin,
                    title.as_deref(),
                    origin,
                    None,
                    crate::engine::FollowUpUrgency::Normal,
                    input_turn,
                    Some(&hook_args),
                )
                .await;

            match result {
                Ok(ref res) => {
                    if res.proposed_change {
                        if res.auto_apply {
                            let pending = match engine_clone.changes().list_pending().await {
                                Ok(v) => v,
                                Err(e) => {
                                    log_legacy!(
                                        "[Chat] auto-apply: list_pending: {} — skipping auto-apply",
                                        e
                                    );
                                    Vec::new()
                                }
                            };
                            if let Some(change) =
                                pending.iter().find(|c| c.request_id == res.request_id)
                            {
                                match engine_clone
                                    .apply_change(change.id, actor_for_apply.clone())
                                    .await
                                {
                                    Ok(result) => {
                                        log_legacy!(
                                            "[Chat] Auto-applied change: {}",
                                            result.message
                                        );
                                    }
                                    Err(e) => {
                                        log_legacy!("[Chat] Failed to auto-apply change: {}", e);
                                        engine_clone
                                            .event_bus
                                            .emit_or_log(
                                                crate::engine::event_bus::BusEvent::System(
                                                    crate::engine::event_bus::SystemEvent::Toast {
                                                        message: format!(
                                                            "Failed to apply change: {}",
                                                            e
                                                        ),
                                                        level: "error".to_string(),
                                                    },
                                                ),
                                                "[Chat] auto-apply error Toast",
                                            )
                                            .await;
                                    }
                                }
                            }
                        }
                        engine_clone.broadcast_changes_updated().await;
                    }

                    if !res.orphaned_injections.is_empty() {
                        let engine = engine_clone.clone();
                        let tid = res.thread_id;
                        let orphans = res.orphaned_injections.clone();
                        tokio::spawn(async move {
                            process_orphan_chain(engine, tid, orphans).await;
                        });
                    }
                }
                // INVARIANT: No terminator here. `process_message_with_steps` settles the
                // exchange itself on every error exit, anchored to the turn's
                // originating event. This branch used to emit its own unanchored
                // `ResponseFailed`. The idempotency gate could not match that, so
                // an in-loop failure wrote the event twice and fired every
                // subscribed trigger twice.
                Err(ref e) => log_legacy!("[Chat] Chat error: {}", e),
            }
        });

        drop(crate::LucidosEngine::monitor_cc_task(
            engine_for_panic,
            thread_id_for_panic,
            handle,
        ));

        Ok::<_, ApiError>(Json(ChatSubmitResponse {
            event_id: response_event_id,
            compose,
        }))
    });
    dispatch
        .await
        .map_err(|e| ApiError::internal(format!("The message could not be dispatched: {e}")))?
}

fn refuse_unroutable_provider(
    state: &AppState,
    model: Option<&str>,
    provider: Option<&str>,
) -> Result<(), ApiError> {
    let Some(provider) = provider else {
        return Ok(());
    };
    let Some(kind) = crate::llm::ProviderKind::from_name(provider) else {
        return Err(ApiError::bad_request(
            crate::llm::model_registry::unknown_provider_message(provider),
        ));
    };
    match model {
        Some(model)
            if crate::llm::model_registry::route_on(state.engine.model_registry(), model, kind)
                .is_none() =>
        {
            Err(ApiError::bad_request(format!(
                "Model '{model}' has no route on provider '{provider}'. Pick one of the \
                 model's own providers, or leave provider unset"
            )))
        }
        _ => Ok(()),
    }
}

/// INVARIANT: Body of `POST /api/v1/chat/settings`. One request carries the whole pair,
/// so a turn cannot start between its halves.
#[derive(Deserialize)]
pub(super) struct ChatSettingsRequest {
    thread_id: String,
    model: String,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    provider: Option<String>,
}

pub(super) async fn chat_settings(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<ChatSettingsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let thread_id = Uuid::parse_str(&request.thread_id)
        .map_err(|_| ApiError::bad_request("Invalid thread_id"))?;
    crate::api::thread_reach::refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        Some(thread_id),
        crate::engine::permission::Action::Control,
    )
    .await?;
    let actor = super::actor::user_actor(&headers, None);
    let model = request.model.trim();
    if model.is_empty() {
        return Err(ApiError::bad_request("model must name a model"));
    }
    if let Some(effort) = request.reasoning_effort.as_deref() {
        if crate::llm::reasoning::tier(effort).is_none() {
            return Err(ApiError::bad_request(format!(
                "reasoning_effort '{effort}' is not a level. Use one of: {}",
                crate::llm::reasoning::EFFORT_LADDER.join(", ")
            )));
        }
    }
    refuse_unroutable_provider(&state, Some(model), request.provider.as_deref())?;
    if super::intelligence_ladder::is_agent_caller(&headers, actor.as_ref()) {
        super::intelligence_ladder::refuse_off_ladder(
            &state,
            crate::runtime::intelligence_ladder::LadderSurface::Chat,
            Some(model),
            request.reasoning_effort.as_deref(),
        )
        .await?;
    }
    let source: Option<String> =
        sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(state.engine.pool())
            .await
            .map_err(|e| ApiError::internal(format!("Could not read thread {thread_id}: {e}")))?;
    match source.as_deref() {
        None => return Err(ApiError::not_found(format!("No thread {thread_id}"))),
        Some(crate::engine::thread_lifecycle::CODING_AGENT_SOURCE) => {
            return Err(ApiError::bad_request(
                "A coding-agent thread picks its model through /api/v1/claude-code/control",
            ))
        }
        Some(_) => {}
    }
    let pick = crate::core::ResolvedModelSelection {
        model: Some(model.to_string()),
        reasoning_effort: request.reasoning_effort,
        provider: request.provider,
    };
    match state
        .engine
        .record_chat_settings(thread_id, pick, actor)
        .await
        .map_err(|e| ApiError::internal(format!("Could not record the pick: {e}")))?
    {
        crate::engine::chat::rung::ChatSettingsOutcome::Recorded => {
            Ok(Json(serde_json::json!({ "ok": true })))
        }
    }
}

pub(super) fn turn_running_refusal() -> ApiError {
    use crate::engine::chat::rung::TURN_RUNNING_MESSAGE;
    use crate::engine::continuation_reason::TURN_RUNNING_REASON;
    ApiError::new(StatusCode::CONFLICT, TURN_RUNNING_MESSAGE).with_reason(TURN_RUNNING_REASON)
}

#[derive(Deserialize)]
pub(super) struct CancelChatQuery {
    thread_id: Option<String>,
}

pub(super) async fn cancel_chat(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<CancelChatQuery>,
) -> Result<Json<super::ChatCancelResponse>, ApiError> {
    let thread_id = parse_optional_uuid(query.thread_id.as_deref())?;
    // INVARIANT: Before the question card is cancel-stamped, so a refusal writes nothing.
    // A thread-bound caller with no `thread_id` is refused rather than reread
    // as "cancel yourself": the unscoped form stops the whole workspace, so it
    // takes the owner's standing instruction like any other root-wide act.
    crate::api::thread_reach::refuse_without_authority(
        &state.engine.event_bus,
        &headers,
        thread_id,
        crate::engine::permission::Action::Cancel,
    )
    .await?;
    let actor = super::actor::user_actor(&headers, None);
    // INVARIANT: Persist take-backs before any question resolution or cancel token can
    // wake/finalize the live loop. Keep cancellation available on a read error.
    let taken_back = crate::engine::consumption_claim::take_back_unconsumed(
        &state.engine.event_bus,
        thread_id,
        actor.clone(),
    )
    .await;
    // INVARIANT: Resolve any pending question card before firing the cancel token.
    // Otherwise the chat agent's `ask_user_question` tool stays blocked on
    // `walk_question_batch.recv()`, which has no cancel-aware select, and the
    // cancel token is never observed. Mirrors `claude_code_stop`. A resolved
    // card counts toward `canceled`: it is a status-changing event the client
    // will receive.
    let question_resolved = if let Some(tid) = thread_id {
        crate::engine::agent_question::resolve_pending_question_as_canceled(
            &state.engine,
            tid,
            actor.clone(),
        )
        .await
    } else {
        false
    };
    let canceled = match thread_id {
        Some(uuid) => {
            if state.engine.cancel_thread(uuid, actor.clone()) {
                true
            } else {
                match crate::engine::claude_code::settle_stuck_running_thread(
                    &state.pool,
                    &state.engine.event_bus,
                    uuid,
                    actor,
                    if question_resolved {
                        crate::engine::claude_code::SettleTerminal::CanceledQuestion
                    } else {
                        crate::engine::claude_code::SettleTerminal::StuckProjection
                    },
                )
                .await
                {
                    Ok(settled) => settled,
                    Err(e) => {
                        crate::log_legacy!("[API] cancel_chat settle failed for {}: {}", uuid, e);
                        false
                    }
                }
            }
        }
        None => state.engine.cancel_all_threads(actor),
    };
    let take_back = taken_back.map_err(|error| {
        ApiError::internal(format!(
            "Cancellation was requested, but unread inputs could not be taken back: {error}"
        ))
    })?;
    Ok(Json(super::ChatCancelResponse {
        canceled: question_resolved || canceled,
        take_back,
    }))
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/chat/stream", post(chat_submit))
        .route("/chat/cancel", post(cancel_chat))
        .route("/chat/settings", post(chat_settings))
        .route("/chat/queued-message/remove", post(remove_queued_message))
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
