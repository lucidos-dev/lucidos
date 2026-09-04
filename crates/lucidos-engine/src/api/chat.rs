use super::actor::build_message_origin;
use super::*;
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::engine::thread_lifecycle::ThreadType;
use crate::engine::thread_state::ThreadState;
use crate::engine::InjectedPrompt;
use crate::engine::PreEmittedOrigin;
use std::collections::VecDeque;
use std::path::PathBuf;

/// Allow/deny matrix for a subprocess-originated chat POST. Allowed:
/// `lucidos spawn-thread` (`mode = Agent|Engine`, `parent_thread_id == source`,
/// target does not exist yet) and same-thread follow-up
/// (`target_thread_id == source`). Denied: any `mode = Human`, since an agent
/// never claims user identity, and cross-thread agent posts.
///
/// `source_thread_id` is authenticated: it comes off the thread-bound origin
/// token, which a subprocess cannot re-point at another thread (`api::actor`).
///
/// The spawn arm also requires `!target_exists`. `parent_matches_source` says
/// "I am spawning a child" and constrains nothing about the target. Without
/// `!target_exists`, a subprocess in thread S could post into any existing
/// thread by naming itself the parent. That is the last route by which an
/// authenticated subprocess could write into a thread it does not own. It would
/// also make `POST /threads/:thread_id/follow-up`'s refusal ladder bypassable.
///
/// `lucidos spawn-thread` sends a fresh client-side uuid with
/// `parent_thread_id`, so its target provably does not exist.
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

/// What the engine tells a caller that tried to record a turn as the user
/// without any evidence of being the user. Written for the agent that will read
/// it: what was refused, why, and what to do instead. A bare status code would
/// reach the caller as nothing at all: `statusText` is always empty over HTTP/2.
pub(crate) const HUMAN_MODE_UNATTRIBUTED: &str = "\
`mode: \"human\"` requires a registered device, and this request has none, so \
the engine cannot record it as something the user typed. If you are an agent: \
do not post to the engine API as the user. Use `follow_up_child_thread` (or \
`lucidos threads follow-up`) to reach one of your own child threads, and if no \
tool covers what you were asked to do, tell the user it is not possible rather \
than working around the tool.";

/// May this request be recorded as a turn the *user* authored?
///
/// The engine holds three kinds of identity evidence, and only two can vouch
/// for a human (ADR 0050):
///
/// - **Device attribution**: a `device_id` (body field, else the
///   `x-lucidos-device-id` header) that resolves to a row in `devices`. This is
///   the user's own client, which sends it on every mutating fetch.
/// - **A `caller_workspace`**: another workspace vouching for its own human.
///   Still only a display hint (`api::actor`), but it is the existing
///   cross-workspace contract and this gate does not renegotiate it.
/// - The **thread-bound origin token**, which vouches for a *subprocess* and
///   therefore proves the opposite: `subprocess_chat_legitimate` refuses
///   `mode: Human` outright.
///
/// Anything else is an *unattributed caller* and may not claim to be the user.
/// This gate sits in front of `subprocess_chat_legitimate`, which only runs for
/// a caller that presents a token: dropping the token must not buy more
/// privilege than presenting it.
pub(crate) fn human_mode_is_attributed(
    mode: ActorMode,
    device_attributed: bool,
    caller_workspace_present: bool,
) -> bool {
    match mode {
        ActorMode::Human => device_attributed || caller_workspace_present,
        // Agent / Engine make no claim about a human, so there is nothing to
        // substantiate. `validate_mode_and_spawn` requires their provenance and
        // `subprocess_chat_legitimate` constrains their reach.
        ActorMode::Agent | ActorMode::Engine => true,
    }
}

/// Resolve the device evidence for `request` and refuse a `mode: human` claim
/// that has none. Called by BOTH chat entry points before any other work, so a
/// refusal writes nothing.
///
/// The device id precedence mirrors `actor::user_actor_resolved`: the body
/// field wins, then the `x-lucidos-device-id` header. The two must agree or the
/// gate would accept a shape the origin builder then stamps differently.
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
    // Blank-filter EACH source BEFORE falling back, not the winner afterwards.
    // `Some("")` is a present-but-empty body field and `Option::or` keeps it.
    // Filtering after the fallback would let an empty `device_id` shadow a good
    // `x-lucidos-device-id` header and refuse a device-attributed request.
    // A nested `fn`, not a closure: it borrows from its argument, keeping both
    // sources `&str`.
    fn blank_filtered(s: &str) -> Option<&str> {
        let t = s.trim();
        (!t.is_empty()).then_some(t)
    }
    let device_id = request
        .device_id
        .as_deref()
        .and_then(blank_filtered)
        .or_else(|| {
            headers
                .get(super::actor::HEADER_DEVICE_ID)
                .and_then(|v| v.to_str().ok())
                .and_then(blank_filtered)
        });
    let device_attributed = match device_id {
        Some(id) => match crate::core::DeviceStore::is_registered(pool, id).await {
            Ok(exists) => exists,
            Err(e) => {
                log!(
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
    log!(
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

/// What the engine tells a caller that addressed a `thread_id` naming nothing.
pub(crate) fn unknown_thread_message(thread_id: Uuid) -> String {
    format!(
        "No thread {thread_id} exists in this workspace, and this request does not say it is \
         creating one, so nothing was written. If you meant to start a new thread, send \
         `new_thread: true`. If you meant to reach an existing thread, you are addressing the \
         wrong engine or the wrong id: several Lucidos engines run on one machine, each serving \
         a different workspace, so check GET /api/v1/health before retrying."
    )
}

/// May this request address `thread_id`?
///
/// A thread has no creation event: it exists because an event exists on its
/// aggregate id, and the `MessageReceived` projection
/// (`engine::event_bus_projection_thread`) is an upsert. An id naming nothing
/// therefore took the insert arm and created the thread. That made a
/// client-supplied id self-fulfilling: a wrong-target write was
/// indistinguishable from a correct one (ADR 0050).
///
/// So a create must be explicit. Three things count as saying so:
///
/// - `new_thread: true`, the frontend's raw-new and compose-first sends;
/// - `parent_thread_id`, a same-workspace spawn with callback
///   (`lucidos spawn-thread --relation child`);
/// - `caller_workspace`, a cross-workspace spawn
///   (`engine::http::workspace_client`, `lucidos spawn-thread --to`).
///
/// A request with no `thread_id` at all is not this function's business: the
/// engine mints the id itself.
pub(crate) fn thread_target_is_addressable(
    thread_exists: bool,
    new_thread: Option<bool>,
    parent_thread_id: Option<Uuid>,
    caller_workspace_present: bool,
) -> bool {
    thread_exists
        || new_thread == Some(true)
        || parent_thread_id.is_some()
        || caller_workspace_present
}

/// Announce every orphan in `batch` as `UserPromptInjected`, anchored on the
/// batch's first message (the turn the re-process runs under).
///
/// The batch is announced WHOLE, not `[1..]`, and two things ride on that.
///
/// **The projection.** An orphan's `MessageReceived` was persisted before the
/// terminator that ended the previous turn, so the re-process passes
/// `PreEmittedOrigin::EngineReentry` and emits no starter event. That leaves
/// `UserPromptInjected` as the only event in the turn whose lifecycle rule is
/// `StatusRule::Set(ThreadStatus::Running)`. Skip the first orphan and a
/// working thread projects idle for the whole turn.
///
/// **The timeline.** The client absorbs a UPI into the `MessageReceived` it
/// names via `injected_message_id` and re-anchors that exchange to the
/// ingestion point. Without the first orphan's UPI, a follow-up that arrived
/// before a `ResponseCanceled` stays above the cancel boundary. It then renders
/// as done while it is the turn being worked on.
///
/// See `docs/plans/2026-08-04-chat-stop-honored-during-turn-setup.md`.
async fn announce_orphan_batch(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    batch: &[InjectedPrompt],
) {
    let Some(first) = batch.first() else {
        return;
    };
    let base_meta = EventMeta {
        request_event_id: first.event_id,
        ..EventMeta::NONE
    };
    for orphan in batch {
        crate::engine::emit_user_prompt_injected_event(bus, thread_id, &base_meta, orphan).await;
    }
}

/// Re-process orphaned injections sequentially until the chain settles.
///
/// `process_message_with_steps` for a re-processed orphan can itself produce
/// orphans: a follow-up `MessageReceived` that landed on `injection_rx` after
/// the loop's final `try_recv()`. Iterating here keeps the chain bounded by the
/// available orphans, and serializes per thread. A fire-and-forget spawn per
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
            let orphans =
                crate::engine::filter_removed_queued_prompts(engine.pool(), thread_id, orphans)
                    .await;
            let Some(first) = orphans.first() else {
                return Vec::new();
            };

            let (text, images, mode, spawning_event_id, origin_event_id) =
                if matches!(&first.kind, crate::engine::InjectedPromptKind::UserText) {
                    announce_orphan_batch(&engine.event_bus, thread_id, &orphans).await;
                    (
                        crate::engine::coalesced_user_text_for_reprocess(&orphans),
                        crate::engine::coalesced_images_for_reprocess(&orphans),
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
                .process_message_with_steps(
                    &text,
                    None,
                    None,
                    None,
                    None,
                    images.as_deref(),
                    None,
                    None,
                    None,
                    Some(thread_id),
                    None,
                    None,
                    None,
                    None,
                    spawning_event_id,
                    mode,
                    None,
                    None,
                    // The first orphan was already emitted as MessageReceived
                    // before it entered the injection channel. Flagged as an
                    // engine re-entry, not a message: this is the engine
                    // replaying work the user already saw acknowledged, so a
                    // second acknowledgment would double up.
                    origin_event_id.map(PreEmittedOrigin::EngineReentry),
                    None,
                    None,
                    None,
                    crate::engine::FollowUpUrgency::Normal,
                )
                .await
            {
                Ok(res) => res.orphaned_injections,
                Err(e) => {
                    log!(
                        "[Chat] Failed to re-process {} orphaned injection(s) for thread {}: {}",
                        orphans.len(),
                        thread_id,
                        e
                    );
                    Vec::new()
                }
            }
        }
    })
    .await;
}

/// Iterate over `initial`, calling `process_one` per item; items the processor
/// returns are appended to the queue and processed in turn.
///
/// Split out of `process_orphan_chain` so the orphan-of-orphan invariant is
/// unit-testable without a real engine: if A's re-process produces orphan B,
/// B is processed too.
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

/// Convert API request contexts into engine file context string.
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

/// What the engine tells a caller that named a thread as its own parent.
pub(crate) const PARENT_IS_THE_THREAD_ITSELF: &str = "\
`parent_thread_id` equals `thread_id`, so this request asks a thread to be its \
own parent. A thread family is a tree, and that row would put a cycle in it. \
Send the id of the thread that spawned this one, or drop `parent_thread_id` if \
this is a follow-up on the thread itself.";

/// Parsed target / spawn / caller UUIDs returned by `validate_mode_and_spawn`.
///
/// Carrying the parsed UUIDs out of validation lets the handler avoid
/// re-parsing (which would silently drop on regression — see CLAUDE.md
/// "no silent defaults").
#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedSpawn {
    thread_id: Option<Uuid>,
    parent_thread_id: Option<Uuid>,
    spawning_event_id: Option<Uuid>,
    caller_thread_id: Option<Uuid>,
    caller_event_id: Option<Uuid>,
}

/// Validate `mode` against the spawning / caller context from a `ChatRequest`.
///
/// `mode` is mandatory: callers state who originated the message, so a thread's
/// source is never silently inferred.
///
/// - `mode = Human` must NOT supply `parent_thread_id` or `spawning_event_id`.
///   Human-originated threads have no spawning context. It may supply
///   `caller_workspace` (a human curl from another workspace).
/// - `mode = Agent | Engine` REQUIRES provenance: either `parent_thread_id`
///   (same-workspace spawn with callback) or `caller_workspace`
///   (cross-workspace origin, fire-and-forget).
///
/// Cross-workspace `caller_*` fields are mutually exclusive with same-workspace
/// `parent_thread_id` / `spawning_event_id`: they describe incompatible
/// relationships (origin without callback, parent with callback). A
/// `caller_thread_id` or `caller_event_id` without `caller_workspace` is
/// malformed.
///
/// This also refuses a `parent_thread_id` equal to `thread_id`, and it is the
/// only reachable write path that could produce one. Every other spawn either
/// mints the child id itself or carries no parent at all. Such a row is a
/// one-node cycle in `parent_thread_id`, and the projection's ancestor walks
/// are what pay for it.
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

    if parent_thread_id.is_some() && parent_thread_id == thread_id {
        log!(
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
        caller_thread_id,
        caller_event_id,
    })
}

/// A thread is locked to the (mode, repo) it picked on its first message.
/// Switching either makes the executor card disagree with the commands menu,
/// and breaks the assumption that the thread's worktree branch lives in one
/// repo.
///
/// `existing_source` is the thread's `thread_summaries.source`; `None` means no
/// row yet, so there is nothing to lock against. `existing_repo_id` is the
/// bound `cc_repo_id`, only meaningful when the source is `claude_code`.
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
    // One reader of the source string, shared with the voice admission and the
    // bus projection, so the three cannot spell `claude_code` differently.
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
        // Backend lock: the new backend has no session to resume, so a flip
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
}

#[derive(Deserialize)]
struct RemoveQueuedMessageRequest {
    thread_id: String,
    message_id: String,
}

async fn queued_message_already_removed(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    message_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
              FROM events
             WHERE aggregate = 'thread'
               AND aggregate_id = $1
               AND event_type = 'QueuedMessageRemoved'
               AND payload->>'removed_message_id' = $2
        )",
    )
    .bind(thread_id.to_string())
    .bind(message_id.to_string())
    .fetch_one(pool)
    .await
}

async fn queued_message_already_injected(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    message_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
              FROM events
             WHERE aggregate = 'thread'
               AND aggregate_id = $1
               AND event_type = 'UserPromptInjected'
               AND payload->>'injected_message_id' = $2
        )",
    )
    .bind(thread_id.to_string())
    .bind(message_id.to_string())
    .fetch_one(pool)
    .await
}

async fn remove_queued_message(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<RemoveQueuedMessageRequest>,
) -> Result<StatusCode, StatusCode> {
    let thread_id = Uuid::parse_str(&request.thread_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let message_id = Uuid::parse_str(&request.message_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    let already_removed = queued_message_already_removed(&state.pool, thread_id, message_id)
        .await
        .map_err(|e| {
            log!(
                "[Chat] queued-message remove: failed to check removal marker for {}: {}",
                message_id,
                e
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if already_removed {
        return Ok(StatusCode::OK);
    }

    let already_injected = queued_message_already_injected(&state.pool, thread_id, message_id)
        .await
        .map_err(|e| {
            log!(
                "[Chat] queued-message remove: failed to check injection marker for {}: {}",
                message_id,
                e
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if already_injected {
        return Err(StatusCode::CONFLICT);
    }

    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    state
        .engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: ThreadEvent::QueuedMessageRemoved {
                removed_message_id: message_id,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                actor,
                ..EventMeta::NONE
            },
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}

/// POST endpoint for chat with progress updates.
/// Returns immediately with an `event_id` (the response's only field; the
/// legacy `message_id` name survives solely as a request-body serde alias on
/// `ChatRequest::event_id`). All progress events are sent via the global SSE
/// stream as ThreadEvent events.
pub(super) async fn chat_submit(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(mut request): Json<ChatRequest>,
) -> Result<Json<ChatSubmitResponse>, ApiError> {
    // Capture frontend origin from the browser Origin header so the LLM
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
    // The human-attribution gate runs before every other check, so a refusal
    // writes nothing at all. See `human_mode_is_attributed` for why it sits in
    // front of the (stricter, but opt-in) subprocess gate further down.
    require_human_mode_is_attributed(&state.pool, &headers, &request).await?;

    // The validator parses the target id too, so the subprocess gate below and
    // the self-parent refusal read the same value.
    let ValidatedSpawn {
        thread_id: target_thread_id,
        parent_thread_id,
        spawning_event_id,
        caller_thread_id,
        caller_event_id,
    } = validate_mode_and_spawn(&request)?;
    let mode = request.mode;

    // The target's projection row, read ONCE and used twice: by the subprocess
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
                log!("[Chat] thread_summaries lookup failed for {}: {}", tid, e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
            None => None,
        };
    let thread_exists = existing_thread_row.is_some();

    // An id-carrying create must say so. Runs before the subprocess gate
    // deliberately: a caller naming a thread that does not exist here has most
    // likely reached the wrong engine, and the 404 says so. The subprocess
    // gate's 403 would instead blame the caller's relationship to a thread it
    // never meant. See `thread_target_is_addressable`.
    if let Some(tid) = target_thread_id {
        if !thread_target_is_addressable(
            thread_exists,
            request.new_thread,
            parent_thread_id,
            request.caller_workspace.is_some(),
        ) {
            log!(
                "[Chat] Refusing to address unknown thread {} (no create signal): mode={:?}, user_agent={:?}",
                tid,
                mode,
                headers.get("user-agent").and_then(|v| v.to_str().ok())
            );
            return Err(ApiError::not_found(unknown_thread_message(tid)));
        }
    }

    // Subprocess gate — see `subprocess_chat_legitimate` for the matrix.
    // Skipped on the cross-workspace path: those requests have their own
    // origin contract (Workspace variant) and don't route through the
    // per-engine subprocess token channel.
    if request.caller_workspace.is_none() {
        if let crate::api::actor::SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } = crate::api::actor::subprocess_origin(&headers)
        {
            if !subprocess_chat_legitimate(
                mode,
                source_thread_id,
                target_thread_id,
                parent_thread_id,
                thread_exists,
            ) {
                log!(
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

    // A create with no parent is a TOP-thread, which sits directly under the
    // workspace root. The root is not a thread and gets no row, so no place in
    // the tree reaches it: only the owner's standing instruction does (ADR 0168
    // clauses 1 and 4).
    //
    // Outside the `caller_workspace` guard above, deliberately. `lucidos
    // spawn-thread --relation top` sends that field even for THIS workspace, so
    // a gate inside it would miss every same-workspace top-thread spawn. A call
    // from another workspace's engine holds no token this engine minted, so it
    // is unaffected and keeps vouching for its own human.
    if !thread_exists && parent_thread_id.is_none() {
        crate::api::thread_reach::refuse_without_authority(
            &state.pool,
            &headers,
            None,
            crate::api::thread_reach::ThreadReachVerb::CreateTopThread,
        )
        .await?;
    }

    let engine_clone = state.engine.clone();
    let message = request.message.clone();
    let model = request.model.clone();
    let reasoning_effort = request.reasoning_effort.clone();
    let device_id = request.device_id.clone();

    // A cross-workspace caller takes precedence over `device_id` in
    // `build_message_origin`, so skip the device-name lookup when one is
    // present. The lookups are otherwise independent, so run them concurrently.
    let workspace_caller_present = request.caller_workspace.is_some();
    let device_lookup = async {
        match device_id.as_deref() {
            Some(did) if !workspace_caller_present => {
                crate::core::DeviceStore::display_name(state.engine.pool(), did).await
            }
            _ => None,
        }
    };
    // Parent title is best-effort display metadata stamped onto the new thread's
    // origin — a transient DB error shouldn't fail the user's send.
    let parent_lookup = async {
        match parent_thread_id {
            Some(ptid) => state
                .engine
                .event_store()
                .get_thread_title(ptid)
                .await
                .unwrap_or_else(|e| {
                    log!(
                        "[Chat] Failed to load parent thread title for {}: {}",
                        ptid,
                        e
                    );
                    None
                }),
            None => None,
        }
    };
    let (device_label, parent_thread_title) = tokio::join!(device_lookup, parent_lookup);

    let caller =
        request
            .caller_workspace
            .as_ref()
            .map(|workspace| crate::api::actor::CallerOrigin {
                workspace: workspace.clone(),
                thread_id: caller_thread_id,
                event_id: caller_event_id,
                // For a cross-workspace POST, the body `mode` describes the upstream
                // (calling workspace's) actor — that's exactly what CallerOrigin.mode wants.
                mode,
            });

    let origin = build_message_origin(
        &headers,
        mode,
        device_id.as_deref(),
        device_label,
        parent_thread_id,
        parent_thread_title,
        spawning_event_id,
        caller,
    );

    let app_ctx = request.app_context;
    let file_ctx = resolve_file_ctx(
        request.file_context.as_ref(),
        request.repo_file_context.as_ref(),
    );
    let url_ctx = request.url_context;
    // Resolve `image_hashes` to ChatImage by reading and base64-encoding the
    // blobs once per send. Mutually exclusive with the legacy `images` body.
    // Both carry the image at original resolution: the fit-to-model-size step
    // happens at the LLM boundary via `ChatImage::fit_for_llm`, so compression
    // lives in exactly one place.
    // Keep the caller-supplied hashes. The Thread Queue branch below persists
    // hashes, never inline base64, and by that point both wire fields have
    // been consumed here.
    let supplied_image_hashes = request.image_hashes.take();
    let chat_images = if let Some(hashes) = &supplied_image_hashes {
        let mut resolved = Vec::with_capacity(hashes.len());
        for hash in hashes {
            match crate::core::blobs::read_blob_as_base64(state.engine.workspace_path(), hash) {
                Some((data, mime_type)) => resolved.push(ChatImage {
                    base64: data,
                    mime_type,
                }),
                None => log!(
                    "[Chat] image_hashes: blob {} missing on disk, dropping from message",
                    hash
                ),
            }
        }
        Some(resolved)
    } else {
        request.images.take()
    };
    let use_coding_agent = request.use_coding_agent;
    let cc_model = request.cc_model;
    let coding_agent = request.coding_agent;
    // `coding_agent` is a coding-agent-spawn payload — without CC requested it
    // would silently vanish (the chat-agent path never reads it). Reject early.
    if coding_agent.is_some() && use_coding_agent != Some(true) {
        log!("[Chat] chat_submit received `coding_agent` without `use_coding_agent=true` — rejecting");
        return Err(StatusCode::BAD_REQUEST.into());
    }
    let event_id = request.event_id;
    // Re-bind the value parsed above for the subprocess-origin gate.
    // Re-parsing it here would double the 400 branch.
    let thread_id = target_thread_id;
    let conflict_change_id = parse_optional_uuid(request.conflict_change_id.as_deref())?;
    // `repo_id` accepts a UUID or a registered repo name (the
    // `lucidos spawn-thread --repo <name>` CLI sends names). A UUID passes
    // through unchanged: it matches `cc_repo_id` storage in `thread_summaries`
    // directly, and downstream code already handles an unknown one. A name
    // resolves to the registered UUID. An unknown name surfaces as a clean 400
    // here, not a 500 from deep inside worktree creation.
    let repo_id = match request.repo_id.as_deref() {
        Some(s) if !s.is_empty() && Uuid::parse_str(s).is_err() => {
            match crate::core::repositories::RepositoryStore::get_by_name(state.engine.pool(), s)
                .await
            {
                Ok(Some(repo)) => Some(repo.id.to_string()),
                Ok(None) => {
                    log!("[Chat] Unknown repo name '{}' in chat_submit", s);
                    return Err(StatusCode::BAD_REQUEST.into());
                }
                Err(e) => {
                    log!("[Chat] Failed to look up repo '{}': {}", s, e);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR.into());
                }
            }
        }
        _ => request.repo_id,
    };

    // Scope-picker payload: `folder` is the new wire field used by the
    // compose-view scope picker. Resolve through the shared
    // `coding_agent_kind` pipeline to a `(repo_id, app stash)` pair so the
    // downstream spawn path is unchanged — see
    // `engine::agent_session::coding_agent_kind` for the rules.
    //
    // Mutual exclusion with `repo_id`: the back-compat path keeps the old
    // wire field; new callers send `folder` exclusively. Both set is a 400 so
    // a half-migrated frontend can't silently win one but lose the other.
    let folder_in = request
        .folder
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (repo_id, thread_id, app_id_to_stash) = if let Some(folder_str) = folder_in {
        if repo_id.as_deref().is_some_and(|s| !s.is_empty()) {
            log!("[Chat] chat_submit received both `folder` and `repo_id` — rejecting");
            return Err(StatusCode::BAD_REQUEST.into());
        }
        // `folder` is a coding-agent spawn payload. Without CC requested, the
        // resolved `app_id_to_stash` would sit forever in `pending_app_spawn`,
        // because the chat agent path never pops it.
        if use_coding_agent != Some(true) {
            log!(
                "[Chat] chat_submit received `folder` without `use_coding_agent=true` — rejecting"
            );
            return Err(StatusCode::BAD_REQUEST.into());
        }
        // Canonicalize workspace_root too — classify_resolved_folder's
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
                    log!("[Chat] canonicalize workspace_root {:?} failed ({}); falling back to raw path — symlinked workspaces may misclassify", raw, e);
                    raw
                }
            }
        };
        // Pre-fetch the registry once; reuse for both `lookup_repo_path` (sync
        // closure inside `resolve_folder_input`) and `external_repo_match`
        // (sync closure inside `classify_resolved_folder`). Sync closures keep
        // both helpers free of `async` plumbing.
        let registered_repos =
            match crate::core::repositories::RepositoryStore::list(state.engine.pool()).await {
                Ok(v) => v,
                Err(e) => {
                    log!("[Chat] failed to list repos for folder resolution: {}", e);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR.into());
                }
            };
        // Canonicalize each registered path once. Both `resolve_folder_input`
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
                        log!("[Chat] canonicalize repo {} path {:?} failed ({}); using raw — match may miss for symlinked / deleted paths", r.id, raw, e);
                        raw
                    }
                };
                (r.id, canon)
            })
            .collect();
        // The Lucidos *source* repo is registered only on a dev build. On a
        // packaged build it is absent, and `None` means the Lucidos-source
        // branch cannot match. App and External still classify, so those
        // coding-agent spawns work without a source repo.
        let lucidos_repo_root = registered_repos
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(crate::engine::LucidosEngine::DEFAULT_REPO_NAME))
            .map(|repo| {
                let raw = PathBuf::from(&repo.path);
                match std::fs::canonicalize(&raw) {
                    Ok(p) => p,
                    Err(e) => {
                        log!("[Chat] canonicalize Lucidos repo path {:?} failed ({}); using raw — Lucidos classification may misfire", raw, e);
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
                log!("[Chat] folder `{}` failed to resolve: {}", folder_str, e);
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
                log!(
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
                // Reuse the canonicalized snapshot — same paths
                // `external_repo_match` consulted, so this find is the
                // mirror lookup that converts the canonical path back to a
                // UUID. No race possible (single in-process snapshot).
                let repo = registered_canonical
                    .iter()
                    .find(|(_, canon)| canon == &repo_root);
                let repo_uuid = match repo {
                    Some((id, _)) => id.to_string(),
                    None => {
                        log!(
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
                // App spawns need the thread_id at stash-time so
                // `run_direct_agent` can pop the entry. Generate one when the
                // frontend didn't send a draft (raw new send) — matches what
                // `spawn_agent_thread` does for the LLM tool path.
                let tid = thread_id.unwrap_or_else(Uuid::new_v4);
                (None, Some(tid), Some((tid, app_id)))
            }
        }
    } else {
        (repo_id, thread_id, None)
    };

    // Lock the thread to its first (mode, repo). The frontend already disables
    // the selectors for an existing thread, so this is the backend backstop.
    // See `validate_thread_continuity`.
    //
    // Skip the lock for `state='composing'`: a draft's `source` reflects the
    // last compose-mode toggle. Toggling back before the first send races the
    // debounced PUT, and reading the lagged source as authoritative surfaces
    // as a 409 on Send. The lock applies once the thread has been sent.
    //
    // The row itself was read above, for the subprocess gate. Only the check
    // lives here.
    if let (Some(tid), Some((state_str, source, existing_repo, existing_agent))) =
        (thread_id, existing_thread_row)
    {
        let existing_state = ThreadState::from_db_str(&state_str).map_err(|e| {
            log!("[Chat] thread_summaries.state for {} invalid: {}", tid, e);
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
                // The message goes to the CLIENT, not just the log. Returning a
                // bare `sc` sent an empty body, and `statusText` is always ""
                // over HTTP/2, so the user's toast read "Failed to send
                // message: 409" with no hint that the thread was locked to a
                // different mode.
                log!("[Chat] Reject follow-up on thread {}: {}", tid, msg);
                return Err(ApiError::new(sc, msg));
            }
        }
    }

    // ---- Thread Queue gate ----
    // Agent/Engine-mode POSTs that START a new thread are background spawns: a
    // cross-workspace task POST, or the `lucidos spawn-thread` CLI. They route
    // through the Thread Queue's admission control like every other background
    // path. A `mode=Human` POST and a follow-up on an existing thread always
    // run immediately: user-initiated chat preempts (ADR 0007, refined by 0008).
    if mode != ActorMode::Human && !thread_exists {
        let queue_thread_id = thread_id.unwrap_or_else(Uuid::new_v4);
        // Persist images as content-addressed blobs — queue requests never
        // carry inline base64 (the request is persisted in the event payload).
        // The wire fields were consumed into `chat_images` above, so reuse the
        // supplied hashes, else re-derive hashes from the resolved images
        // (content-addressed, so re-persisting is idempotent).
        let image_hashes = match supplied_image_hashes {
            Some(hashes) => hashes,
            None => state.engine.queued_image_hashes(chat_images.as_deref()),
        };
        let response_event_id = event_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let queue_request = crate::engine::thread_queue::ThreadQueueRequest::AgentChat {
            // Stamped by `submit` from this task's chain depth.
            depth: 0,
            message: request.message.clone(),
            thread_id: queue_thread_id,
            event_id: Some(response_event_id.clone()),
            image_hashes,
            device_id: device_id.clone(),
            model: model.clone(),
            reasoning_effort: reasoning_effort.clone(),
            use_coding_agent,
            repo_id: repo_id.clone(),
            cc_model: cc_model.clone(),
            coding_agent,
            title: request.title.clone(),
            mode,
            origin: origin.clone(),
            parent_thread_id,
            spawning_event_id,
            app_id: app_id_to_stash.map(|(_, app_id)| app_id),
        };
        let outcome = state
            .engine
            .thread_queue
            .submit(queue_request, origin, None)
            .await;
        if !outcome.admitted {
            log!(
                "[Chat] Agent-mode spawn queued at position {} (thread {})",
                outcome.position,
                queue_thread_id
            );
        }
        return Ok(Json(ChatSubmitResponse {
            event_id: response_event_id,
        }));
    }

    // Stash the app spawn AFTER every early-return gate above. An entry
    // stamped earlier would leak in the in-memory map when a gate rejects:
    // `run_direct_agent` is the only popper, and it is never reached on the
    // 4xx/5xx paths.
    // On mutex poisoning the handler returns a 500, matching the rest of the
    // engine's `std::sync::Mutex` guard shape rather than crashing the worker.
    if let Some((tid, app_id)) = app_id_to_stash {
        match state.engine.pending_app_spawn.lock() {
            Ok(mut guard) => {
                guard.insert(tid, app_id);
            }
            Err(e) => {
                log!("[Chat] pending_app_spawn poisoned, cannot stash: {}", e);
                return Err(StatusCode::INTERNAL_SERVER_ERROR.into());
            }
        }
    }

    let title = request.title;
    let response_event_id = event_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    // Persist the follow-up's MessageReceived BEFORE this handler acks. The 200
    // then means "recorded, with its sequence assigned". A client that sends one
    // message at a time per thread keeps its order end to end.
    // Self-gating and non-fatal: see `pre_emit_chat_message_received`. Runs
    // after every validation gate above, so a request that 4xx'd cannot leave
    // an orphaned message behind.
    let pre_emitted_origin = state
        .engine
        .pre_emit_chat_message_received(
            thread_id,
            thread_exists,
            mode,
            use_coding_agent,
            &message,
            chat_images.as_deref(),
            device_id.as_deref(),
            model.as_deref(),
            reasoning_effort.as_deref(),
            event_id.as_deref(),
            origin.clone(),
            // Typed. A spoken message arrives on the voice socket instead,
            // never through this handler.
            None,
        )
        .await;

    let result_started_at = state.started_at;
    let engine_for_panic = state.engine.clone();
    let thread_id_for_panic = thread_id;
    let actor_for_apply = origin.clone();
    let handle = tokio::spawn(async move {
        // User-initiated work shares the one capacity pool (ADR 0008): take a
        // prioritized slot, held across the whole response. Admits at once
        // when the pool has room; at true pool-max this awaits a free slot
        // (the person sees "requesting" until then). Released on task end —
        // even on panic — by the guard's Drop.
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
                chat_images.as_deref(),
                device_id.as_deref(),
                use_coding_agent,
                event_id.as_deref(),
                thread_id,
                conflict_change_id,
                repo_id.as_deref(),
                url_ctx,
                parent_thread_id,
                spawning_event_id,
                mode,
                cc_model.as_deref(),
                coding_agent,
                pre_emitted_origin,
                title.as_deref(),
                origin,
                None,
                crate::engine::FollowUpUrgency::Normal,
            )
            .await;

        // Post-processing: auto-apply changes and broadcast updates
        match result {
            Ok(ref res) => {
                if res.proposed_change {
                    if res.auto_apply {
                        let pending = match engine_clone.changes().list_pending().await {
                            Ok(v) => v,
                            Err(e) => {
                                log!(
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
                                    log!("[Chat] Auto-applied change: {}", result.message);
                                }
                                Err(e) => {
                                    log!("[Chat] Failed to auto-apply change: {}", e);
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
                    let proj = engine_clone.changes();
                    let (pending_r, applied_r, restart_r) = tokio::join!(
                        proj.list_pending(),
                        proj.list_recently_applied(15, None),
                        proj.requires_restart_since(result_started_at),
                    );
                    match (pending_r, applied_r, restart_r) {
                        (Ok(mut pending), Ok(mut applied), Ok(restart)) => {
                            let (r1, r2) = tokio::join!(
                                crate::core::changes::enrich_thread_titles(
                                    engine_clone.pool(),
                                    &mut pending,
                                ),
                                crate::core::changes::enrich_thread_titles(
                                    engine_clone.pool(),
                                    &mut applied,
                                ),
                            );
                            if let Err(e) = r1 {
                                log!("[Chat] enrich pending titles: {}", e);
                            }
                            if let Err(e) = r2 {
                                log!("[Chat] enrich applied titles: {}", e);
                            }
                            engine_clone
                                .event_bus
                                .emit_or_log(
                                    crate::engine::event_bus::BusEvent::System(
                                        crate::engine::event_bus::SystemEvent::ChangesUpdated {
                                            total_pending: pending.len(),
                                            pending,
                                            applied,
                                            restart_required: restart,
                                        },
                                    ),
                                    "[Chat] ChangesUpdated",
                                )
                                .await;
                        }
                        (perr, aerr, rerr) => {
                            if let Err(e) = perr {
                                log!("[Chat] post-process list_pending: {}", e);
                            }
                            if let Err(e) = aerr {
                                log!("[Chat] post-process list_recently_applied: {}", e);
                            }
                            if let Err(e) = rerr {
                                log!("[Chat] post-process requires_restart_since: {}", e);
                            }
                            log!("[Chat] skipping post-turn ChangesUpdated broadcast");
                        }
                    }
                }

                // Re-submit orphaned injections (follow-ups that arrived after
                // the processing loop exited but before cleanup finished). See
                // `process_orphan_chain` for the chain-drain invariant.
                if !res.orphaned_injections.is_empty() {
                    let engine = engine_clone.clone();
                    let tid = res.thread_id;
                    let orphans = res.orphaned_injections.clone();
                    tokio::spawn(async move {
                        process_orphan_chain(engine, tid, orphans).await;
                    });
                }
            }
            // No terminator here. `process_message_with_steps` settles the
            // exchange itself on every error exit, anchored to the turn's
            // originating event. This branch used to emit its own unanchored
            // `ResponseFailed`. The idempotency gate could not match that, so
            // an in-loop failure wrote the event twice and fired every
            // subscribed trigger twice.
            Err(ref e) => log!("[Chat] Chat error: {}", e),
        }
    });

    // Monitor the spawned task. On a panic it emits ResponseFailed and
    // SessionEnded and cleans up the coding-agent session, so the thread does
    // not stick in "running".
    if let Some(tid) = thread_id_for_panic {
        // Dropping the JoinHandle detaches the watcher, which keeps running.
        drop(LucidosEngine::monitor_cc_task(
            engine_for_panic,
            tid,
            handle,
        ));
    } else {
        tokio::spawn(async move {
            if let Err(join_err) = handle.await {
                if join_err.is_panic() {
                    log!("[Chat] Task panicked (no thread context): {:?}", join_err);
                }
            }
        });
    }

    Ok(Json(ChatSubmitResponse {
        event_id: response_event_id,
    }))
}

#[derive(Deserialize)]
pub(super) struct CancelChatQuery {
    thread_id: Option<String>,
}

/// `ApiError` rather than a bare `StatusCode`, so the reach refusal can say
/// why: a bodyless 403 reaches an agent as nothing at all, since `statusText`
/// is always empty over HTTP/2.
///
/// The malformed-uuid 400 rides along through `From<StatusCode>` and keeps its
/// status, but its body changes from empty to `{"error": "Bad Request"}`. That
/// is what the conversion is for, per `api::error`: adopt `ApiError` for the
/// one branch with something to say, and let the rest carry the canonical
/// reason phrase instead of nothing.
pub(super) async fn cancel_chat(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<CancelChatQuery>,
) -> Result<Json<super::CancelResponse>, ApiError> {
    let thread_id = parse_optional_uuid(query.thread_id.as_deref())?;
    // Before the question card is cancel-stamped, so a refusal writes nothing.
    // A thread-bound caller with no `thread_id` is refused rather than reread
    // as "cancel yourself": the unscoped form stops the whole workspace, so it
    // takes the owner's standing instruction like any other root-wide act.
    crate::api::thread_reach::refuse_without_authority(
        &state.pool,
        &headers,
        thread_id,
        crate::api::thread_reach::ThreadReachVerb::Cancel,
    )
    .await?;
    // Resolve the actor once and reuse it for the question-card resolution, the
    // cancel-thread call, and the settle fallback. It stamps
    // `ResponseCanceled.actor` / `ResponseAborted.actor`, so the timeline
    // records which device clicked Stop.
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    // Resolve any pending question card before firing the cancel token.
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
    // Stop ends the TURN, and nothing else. It deliberately leaves this
    // thread's event subscriptions alone (ADR 0052): a subscription has not
    // held a turn since ADR 0049, so it is not part of what a turn-scoped Stop
    // owns.
    //
    // `canceled = false` means the server had nothing live to cancel, so the
    // client's optimistic "canceling" state is stale and it must re-sync.
    let canceled = match thread_id {
        Some(uuid) => {
            if state.engine.cancel_thread(uuid, actor.clone()) {
                true
            } else {
                // No live handle: the projection may still be stuck at
                // `running` (the client raced the terminal broadcast on load,
                // or a spawn errored before emitting one). Settle it so a
                // terminal event lands and the thread stops looking mid-turn,
                // parity with the CC interrupt path.
                //
                // A cancel-stamped question above is what set the row to
                // `running`, so that case is no wedge at all: it settles as the
                // `ResponseCanceled` a live loop would have emitted. See
                // `SettleTerminal`.
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
                        crate::log!("[API] cancel_chat settle failed for {}: {}", uuid, e);
                        false
                    }
                }
            }
        }
        None => state.engine.cancel_all_threads(actor),
    };
    Ok(Json(super::CancelResponse {
        canceled: question_resolved || canceled,
    }))
}

/// Routes for the `/chat*` surface.
///
/// There is no bare `POST /chat`. It was a strictly weaker duplicate of
/// `/chat/stream` over the same mutation. Keeping it meant maintaining two
/// gate lists that had already drifted apart (ADR 0116).
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/chat/stream", post(chat_submit))
        .route("/chat/cancel", post(cancel_chat))
        .route("/chat/queued-message/remove", post(remove_queued_message))
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
