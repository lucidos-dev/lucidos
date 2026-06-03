use super::actor::build_message_origin;
use super::*;
use crate::engine::thread_events::ActorMode;
use crate::engine::thread_state::ThreadState;
use crate::engine::InjectedPrompt;
use std::collections::VecDeque;
use std::path::PathBuf;

/// Allow/deny matrix for a subprocess-originated chat POST. Allowed:
/// `lucidos spawn-thread` (`mode = Agent|Engine`, `parent_thread_id == source`)
/// and same-thread follow-up (`target_thread_id == source`). Denied: any
/// `mode = Human` (agents never claim user identity) and cross-thread agent
/// posts (the original incident shape).
///
/// Pure function — unit-testable without booting a router.
pub(crate) fn subprocess_chat_legitimate(
    mode: ActorMode,
    source_thread_id: Option<Uuid>,
    target_thread_id: Option<Uuid>,
    parent_thread_id: Option<Uuid>,
) -> bool {
    let target_matches_source = target_thread_id.is_some() && target_thread_id == source_thread_id;
    let parent_matches_source = match (parent_thread_id, source_thread_id) {
        (Some(p), Some(s)) => p == s,
        _ => false,
    };
    match mode {
        ActorMode::Human => false,
        ActorMode::Agent | ActorMode::Engine => target_matches_source || parent_matches_source,
    }
}

/// Re-process orphaned injections sequentially until the chain settles.
///
/// `process_message_with_steps` for a re-processed orphan can itself produce
/// orphans (a follow-up MessageReceived arrived during the re-process loop
/// but landed on `injection_rx` after the loop's final `try_recv()`). The
/// caller used to fire-and-forget a `tokio::spawn` per orphan, throwing away
/// `ProcessResult` and the inner `orphaned_injections` with it — so an
/// orphan-of-orphan was silently lost, leaving its MR without a response and
/// stamping the NEXT reply with the prior orphan's `request_event_id`
/// (observed on real thread 9b5a05aa as a chat exchange where the second
/// follow-up's MR went unanswered while the third turn's response bound
/// onto the wrong MR). Iterating here keeps the chain bounded by the
/// available orphans and serializes per-thread (`register_thread_queued`
/// already serializes anyway, so a queue costs nothing extra).
async fn process_orphan_chain(
    engine: SharedEngine,
    thread_id: Uuid,
    initial_orphans: Vec<InjectedPrompt>,
) {
    drain_orphan_queue(initial_orphans, |orphan| {
        let engine = engine.clone();
        async move {
            match engine
                .process_message_with_steps(
                    &orphan.text,
                    None,
                    None,
                    None,
                    None,
                    orphan.images.as_deref(),
                    None,
                    None,
                    None,
                    Some(thread_id),
                    None,
                    None,
                    None,
                    None,
                    orphan.spawning_event_id,
                    orphan.mode,
                    None,
                    orphan.event_id,
                    None,
                    None,
                )
                .await
            {
                Ok(res) => res.orphaned_injections,
                Err(e) => {
                    log!(
                        "[Chat] Failed to re-process orphaned injection for thread {}: {}",
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

/// Iterate over `initial`, calling `process_one` per item; any items the
/// processor returns are appended to the queue and processed in turn. Pure
/// queue-drain logic, extracted from `process_orphan_chain` so the
/// orphan-of-orphan invariant ("if A's re-process produces orphan B, B is
/// processed too") is unit-testable without a real engine.
async fn drain_orphan_queue<F, Fut>(initial: Vec<InjectedPrompt>, mut process_one: F)
where
    F: FnMut(InjectedPrompt) -> Fut,
    Fut: std::future::Future<Output = Vec<InjectedPrompt>>,
{
    let mut queue: VecDeque<InjectedPrompt> = initial.into();
    while let Some(item) = queue.pop_front() {
        let new_orphans = process_one(item).await;
        queue.extend(new_orphans);
    }
}

/// Convert API request contexts into engine file context string.
fn resolve_file_ctx(
    file_context: Option<&super::FileContext>,
    repo_file_context: Option<&super::RepoFileContext>,
) -> Option<String> {
    file_context.map(|ctx| ctx.path.clone()).or_else(|| {
        repo_file_context.map(|ctx| {
            if let Some((start, end)) = ctx.lines {
                format!("[repo:{}] {}:{}-{}", ctx.repo_id, ctx.path, start, end)
            } else {
                format!("[repo:{}] {}", ctx.repo_id, ctx.path)
            }
        })
    })
}

/// Parsed spawn / caller UUIDs returned by `validate_mode_and_spawn`.
///
/// Carrying parsed `caller_*` UUIDs out of validation lets the handler avoid
/// re-parsing (which would silently drop on regression — see CLAUDE.md
/// "no silent defaults").
#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedSpawn {
    parent_thread_id: Option<Uuid>,
    spawning_event_id: Option<Uuid>,
    caller_thread_id: Option<Uuid>,
    caller_event_id: Option<Uuid>,
}

/// Validate `mode` against the spawning / caller context from a `ChatRequest`.
///
/// `mode` is mandatory on the API: callers must explicitly state who originated
/// the message. The mapping is enforced so the source of a thread is never
/// silently inferred:
///
/// - `mode = Human` — must NOT supply `parent_thread_id` or `spawning_event_id`.
///   Human-originated threads have no spawning context. May supply
///   `caller_workspace` (e.g. a human curl from another workspace).
/// - `mode = Agent | Engine` — provenance is REQUIRED. Either `parent_thread_id`
///   (same-workspace spawn with callback) OR `caller_workspace`
///   (cross-workspace origin, fire-and-forget) must be present.
///
/// Cross-workspace `caller_*` fields are mutually exclusive with same-workspace
/// `parent_thread_id` / `spawning_event_id`: they describe incompatible
/// relationships (origin without callback vs. parent with callback). A
/// `caller_thread_id` or `caller_event_id` without `caller_workspace` is
/// malformed.
///
/// Returns `Err(StatusCode::BAD_REQUEST)` if the constraint is violated or any
/// UUID is malformed — failing fast beats silently dropping the parent link.
fn validate_mode_and_spawn(request: &ChatRequest) -> Result<ValidatedSpawn, StatusCode> {
    // Cross-workspace caller_* fields are mutually exclusive with same-workspace
    // parent_thread_id / spawning_event_id. They describe incompatible
    // relationships: caller_* = origin (no callback), parent_* = parent (callback).
    let has_caller = request.caller_workspace.is_some()
        || request.caller_thread_id.is_some()
        || request.caller_event_id.is_some();

    if has_caller && request.caller_workspace.is_none() {
        // caller_thread_id / caller_event_id without caller_workspace is malformed.
        return Err(StatusCode::BAD_REQUEST);
    }

    let has_parent = request.parent_thread_id.is_some() || request.spawning_event_id.is_some();

    if has_caller && has_parent {
        return Err(StatusCode::BAD_REQUEST);
    }

    let caller_thread_id = parse_optional_uuid(request.caller_thread_id.as_deref())?;
    let caller_event_id = parse_optional_uuid(request.caller_event_id.as_deref())?;
    let parent_thread_id = parse_optional_uuid(request.parent_thread_id.as_deref())?;
    let spawning_event_id = parse_optional_uuid(request.spawning_event_id.as_deref())?;

    match request.mode {
        ActorMode::Human => {
            if parent_thread_id.is_some() || spawning_event_id.is_some() {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
        ActorMode::Agent | ActorMode::Engine => {
            // Agent/Engine mode requires EITHER parent_thread_id (same-workspace
            // spawn with callback) OR caller_workspace (cross-workspace origin).
            // Without either, no provenance — reject.
            if parent_thread_id.is_none() && request.caller_workspace.is_none() {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }

    Ok(ValidatedSpawn {
        parent_thread_id,
        spawning_event_id,
        caller_thread_id,
        caller_event_id,
    })
}

/// Wire-format string for `ThreadType::CodingAgent` (`thread_summaries.source`
/// and the `channel` payload field). Mirrored here to avoid pulling the engine
/// enum across the API boundary just for this single comparison; the source of
/// truth is the `#[serde(rename = "claude_code")]` on `ThreadType::CodingAgent`.
const CC_SOURCE: &str = "claude_code";

/// A thread is locked to the (mode, repo) it picked on its first message.
/// Subsequent follow-ups must match — switching either makes the executor card
/// disagree with the commands menu (the menu collapses to one repo) and breaks
/// the assumption that the thread's worktree branch lives in one repo.
///
/// `existing_source` is the thread's `thread_summaries.source`; `None` means
/// no row yet (new thread — nothing to lock against). `existing_repo_id` is
/// the bound `cc_repo_id`, only meaningful when the source is `claude_code`.
pub(super) fn validate_thread_continuity(
    existing_source: Option<&str>,
    existing_repo_id: Option<&str>,
    requested_use_cc: Option<bool>,
    requested_repo_id: Option<&str>,
) -> Result<(), (StatusCode, String)> {
    let Some(source) = existing_source else {
        return Ok(());
    };
    let existing_is_cc = source == CC_SOURCE;
    let requested_is_cc = requested_use_cc == Some(true);
    if existing_is_cc != requested_is_cc {
        let from = if existing_is_cc {
            "Claude Code"
        } else {
            "Lucidos"
        };
        let to = if requested_is_cc {
            "Claude Code"
        } else {
            "Lucidos"
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
    }
    Ok(())
}

pub(super) async fn chat(
    State(state): State<AppState>,
    Json(request): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, StatusCode> {
    if let Some(ref model) = request.model {
        log!("[Chat] Using model: {}", model);
    }

    let validated = validate_mode_and_spawn(&request)?;
    let ValidatedSpawn {
        parent_thread_id,
        spawning_event_id,
        ..
    } = validated;
    let mode = request.mode;

    let app_ctx = request.app_context;
    let file_ctx = resolve_file_ctx(
        request.file_context.as_ref(),
        request.repo_file_context.as_ref(),
    );
    let url_ctx = request.url_context;

    // Reject malformed thread_id with 400 instead of silently starting a new
    // thread (CLAUDE.md "no silent defaults").
    let thread_id = parse_optional_uuid(request.thread_id.as_deref())?;
    Ok(
        match state
            .engine
            .process_message_with_steps(
                &request.message,
                request.model.as_deref(),
                app_ctx,
                file_ctx,
                request.reasoning_effort.as_deref(),
                request.images.as_deref(),
                request.device_id.as_deref(),
                None,
                request.event_id.as_deref(),
                thread_id,
                None,
                None,
                url_ctx,
                parent_thread_id,
                spawning_event_id,
                mode,
                request.cc_model.as_deref(),
                None,
                request.title.as_deref(),
                None,
            )
            .await
        {
            Ok(result) => Json(ChatResponse {
                response: result.response,
                steps: result.steps,
            }),
            Err(e) => Json(ChatResponse {
                response: format!("[ERROR] {}", e),
                steps: vec![],
            }),
        },
    )
}

#[derive(Serialize)]
pub(super) struct ChatSubmitResponse {
    event_id: String,
}

/// POST endpoint for chat with progress updates.
/// Returns immediately with a message_id. All progress events are sent
/// via the global SSE stream as ThreadEvent events.
pub(super) async fn chat_submit(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(mut request): Json<ChatRequest>,
) -> Result<Json<ChatSubmitResponse>, StatusCode> {
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
    let ValidatedSpawn {
        parent_thread_id,
        spawning_event_id,
        caller_thread_id,
        caller_event_id,
    } = validate_mode_and_spawn(&request)?;
    let mode = request.mode;

    // Parse target thread id up front so the subprocess gate can see it.
    let target_thread_id = parse_optional_uuid(request.thread_id.as_deref())?;

    // Subprocess gate — see `subprocess_chat_legitimate` for the matrix.
    // Skipped on the cross-workspace path: those requests have their own
    // origin contract (Workspace variant) and don't route through the
    // per-engine subprocess token channel.
    if request.caller_workspace.is_none() {
        if let crate::api::actor::SubprocessOrigin::Subprocess { source_thread_id } =
            crate::api::actor::subprocess_origin(&headers)
        {
            if !subprocess_chat_legitimate(mode, source_thread_id, target_thread_id, parent_thread_id) {
                log!(
                    "[Chat] Rejecting subprocess chat POST: mode={:?}, source_thread={:?}, target_thread={:?}, parent_thread={:?}",
                    mode,
                    source_thread_id,
                    target_thread_id,
                    parent_thread_id
                );
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    let engine_clone = state.engine.clone();
    let message = request.message.clone();
    let model = request.model.clone();
    let reasoning_effort = request.reasoning_effort.clone();
    let device_id = request.device_id.clone();

    // A cross-workspace caller (`caller_workspace` set in body) takes
    // precedence over device_id in build_message_origin — skip the device-name
    // lookup when present to save a roundtrip on the cross-workspace path.
    // Lookups are otherwise independent, so run them concurrently when both
    // apply.
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
    // Resolve `image_hashes` to ChatImage by reading + base64-encoding the
    // blobs once per send. Mutually exclusive with `images`; the latter
    // (legacy base64 body) is still accepted and runs through the same
    // compression pipeline. After Phase 4 ships, every frontend send takes
    // the hash path and the legacy body becomes dead.
    let chat_images = if let Some(hashes) = request.image_hashes.take() {
        let mut resolved = Vec::with_capacity(hashes.len());
        for hash in &hashes {
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
        request.images.take().map(super::compress_images)
    };
    let use_claude_code = request.use_claude_code;
    let cc_model = request.cc_model;
    let event_id = request.event_id;
    // Re-bind under the existing name so the rest of the handler reads
    // exactly as before (this is the same value parsed above for the
    // subprocess-origin gate). Re-parsing would double the 400 branch.
    let thread_id = target_thread_id;
    let conflict_change_id = parse_optional_uuid(request.conflict_change_id.as_deref())?;
    // `repo_id` accepts either a UUID or a registered repo name (the
    // `lucidos spawn-thread --repo <name>` CLI sends names). UUIDs pass
    // through unchanged — they match `cc_repo_id` storage in
    // `thread_summaries` directly, and downstream code already handles
    // unknown UUIDs (the tests seeding random UUIDs depend on that). Names
    // resolve to the registered UUID; an unknown name surfaces as a clean
    // 400 here instead of a 500 from deep inside worktree creation.
    let repo_id = match request.repo_id.as_deref() {
        Some(s) if !s.is_empty() && Uuid::parse_str(s).is_err() => {
            match crate::core::repositories::RepositoryStore::get_by_name(state.engine.pool(), s)
                .await
            {
                Ok(Some(repo)) => Some(repo.id.to_string()),
                Ok(None) => {
                    log!("[Chat] Unknown repo name '{}' in chat_submit", s);
                    return Err(StatusCode::BAD_REQUEST);
                }
                Err(e) => {
                    log!("[Chat] Failed to look up repo '{}': {}", s, e);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
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
    let folder_in = request.folder.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (repo_id, thread_id, app_id_to_stash) = if let Some(folder_str) = folder_in {
        if repo_id.as_deref().is_some_and(|s| !s.is_empty()) {
            log!("[Chat] chat_submit received both `folder` and `repo_id` — rejecting");
            return Err(StatusCode::BAD_REQUEST);
        }
        // `folder` is a CC-spawn payload — when CC isn't requested, the
        // resolved app_id_to_stash would sit forever in `pending_app_spawn`
        // because the chat agent path never pops it. Reject early.
        if use_claude_code != Some(true) {
            log!(
                "[Chat] chat_submit received `folder` without `use_claude_code=true` — rejecting"
            );
            return Err(StatusCode::BAD_REQUEST);
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
        let registered_repos = match crate::core::repositories::RepositoryStore::list(state.engine.pool()).await {
            Ok(v) => v,
            Err(e) => {
                log!("[Chat] failed to list repos for folder resolution: {}", e);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
        // Canonicalize each registered path once. Both `resolve_folder_input`
        // and `classify_resolved_folder` canonicalize their working
        // `folder_abs`; without symmetric canonicalization here, a
        // registered repo under a symlinked ancestor (macOS `/var/...` →
        // `/private/var/...`, /home → /Users, …) would miss the External
        // branch and fall through to the unrecognised-path refusal.
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
        let lucidos_repo_root = match registered_repos
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(crate::engine::LucidosEngine::DEFAULT_REPO_NAME))
        {
            Some(repo) => {
                let raw = PathBuf::from(&repo.path);
                match std::fs::canonicalize(&raw) {
                    Ok(p) => p,
                    Err(e) => {
                        log!("[Chat] canonicalize Lucidos repo path {:?} failed ({}); using raw — Lucidos classification may misfire", raw, e);
                        raw
                    }
                }
            }
            None => {
                log!("[Chat] Lucidos repo not registered — cannot classify folder");
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
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
                return Err(StatusCode::BAD_REQUEST);
            }
        };
        let external_repo_match = |path: &std::path::Path| -> Option<PathBuf> {
            registered_canonical
                .iter()
                .find_map(|(_, canon)| if canon == path { Some(canon.clone()) } else { None })
        };
        use crate::engine::agent_session::coding_agent_kind::{
            classify_resolved_folder, FolderClassification,
        };
        let classification = match classify_resolved_folder(
            &folder_abs,
            &workspace_root,
            &lucidos_repo_root,
            external_repo_match,
        ) {
            Ok(c) => c,
            Err(e) => {
                log!("[Chat] folder `{}` classification failed: {}", folder_str, e);
                return Err(StatusCode::BAD_REQUEST);
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
                        return Err(StatusCode::INTERNAL_SERVER_ERROR);
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

    // Lock thread to its first (mode, repo). Switching either mid-thread
    // makes the executor card and commands menu disagree (different repo's
    // skills, branch can't follow across repos). Frontend should already
    // disable the selectors for existing threads — this is the backend
    // backstop. See `validate_thread_continuity`.
    //
    // Skip the lock for `state='composing'`: a draft's `source` reflects the
    // last compose-mode toggle (the PUT compose CASE writes 'chat' or
    // 'claude_code' to it). Toggling back across modes before the first send
    // races the debounced PUT; reading the lagged source as authoritative
    // here surfaces as a 409 ("Thread is locked to X mode") on Send. The
    // lock only applies once the thread has actually been sent.
    if let Some(tid) = thread_id {
        let row: Option<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT state, source, cc_repo_id FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(tid)
        .fetch_optional(state.engine.pool())
        .await
        .map_err(|e| {
            log!("[Chat] thread_summaries lookup failed for {}: {}", tid, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        if let Some((state_str, source, existing_repo)) = row {
            let existing_state = ThreadState::from_db_str(&state_str).map_err(|e| {
                log!("[Chat] thread_summaries.state for {} invalid: {}", tid, e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            if !existing_state.can_change_mode() {
                if let Err((sc, msg)) = validate_thread_continuity(
                    Some(&source),
                    existing_repo.as_deref(),
                    use_claude_code,
                    repo_id.as_deref(),
                ) {
                    log!("[Chat] Reject follow-up on thread {}: {}", tid, msg);
                    return Err(sc);
                }
            }
        }
    }

    // Stash the app spawn AFTER every early-return gate above (continuity
    // lock, thread_summaries lookup error). Earlier-stamped entries would
    // leak in the in-memory map when those gates reject — `run_direct_agent`
    // is the only popper, and it's never reached on the 4xx/5xx paths.
    // Mutex poisoning: parking_lot would be cleaner, but the rest of the
    // engine uses std::sync::Mutex with the same `match` guard; matching
    // that pattern means a panic in any other holder of this lock degrades
    // app-spawn dispatching to a 500 instead of crashing the worker.
    if let Some((tid, app_id)) = app_id_to_stash {
        match state.engine.pending_app_spawn.lock() {
            Ok(mut guard) => {
                guard.insert(tid, app_id);
            }
            Err(e) => {
                log!("[Chat] pending_app_spawn poisoned, cannot stash: {}", e);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }

    let title = request.title;
    // Generate an event_id for tracking progress events
    let response_event_id = event_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    // Spawn task to process message — all events flow through EventBus now.
    // The JoinHandle is monitored so panics emit ResponseFailed + SessionEnded
    // instead of silently dropping the thread into a stuck "running" state.
    let result_started_at = state.started_at;
    let engine_for_panic = state.engine.clone();
    let thread_id_for_panic = thread_id;
    let actor_for_apply = origin.clone();
    let handle = tokio::spawn(async move {
        let result = engine_clone
            .process_message_with_steps(
                &message,
                model.as_deref(),
                app_ctx,
                file_ctx,
                reasoning_effort.as_deref(),
                chat_images.as_deref(),
                device_id.as_deref(),
                use_claude_code,
                event_id.as_deref(),
                thread_id,
                conflict_change_id,
                repo_id.as_deref(),
                url_ctx,
                parent_thread_id,
                spawning_event_id,
                mode,
                cc_model.as_deref(),
                None,
                title.as_deref(),
                origin,
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
            Err(ref e) => {
                log!("[Chat] Chat error: {}", e);
                // Signal frontend to exit "running" state with error
                if let Some(tid) = thread_id {
                    if let Err(emit_err) = engine_clone
                        .event_bus
                        .emit(crate::engine::event_bus::BusEvent::Thread {
                            thread_id: tid,
                            event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                error: e.to_string(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        })
                        .await
                    {
                        log!("[Chat] Failed to emit ResponseFailed: {}", emit_err);
                    }
                }
            }
        }
    });

    // Monitor the spawned task — if it panics, emit ResponseFailed + SessionEnded
    // and clean up the Claude Code session so the thread doesn't get stuck in "running" state.
    if let Some(tid) = thread_id_for_panic {
        LucidosEngine::monitor_cc_task(engine_for_panic, tid, handle);
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

pub(super) async fn cancel_chat(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<CancelChatQuery>,
) -> Result<StatusCode, StatusCode> {
    let thread_id = parse_optional_uuid(query.thread_id.as_deref())?;
    // Resolve actor once and reuse for both the question-card resolution
    // and the cancel-thread call — the actor stamps `ResponseCanceled.actor`
    // so the timeline records which device clicked Stop.
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    // Resolve any pending question card before firing the cancel token —
    // otherwise the chat agent's `ask_user_question` tool stays blocked on
    // `walk_question_batch.recv()` (no cancel-aware select) and the cancel
    // token is never observed. Mirrors `claude_code_stop`'s behavior; without
    // it, canceling a chat thread with an active question card hangs
    // indefinitely in the "Canceling…" state.
    if let Some(tid) = thread_id {
        crate::engine::agent_question::resolve_pending_question_as_canceled(
            &state.engine,
            tid,
            actor.clone(),
        )
        .await;
    }
    match thread_id {
        Some(uuid) => {
            state.engine.cancel_thread(uuid, actor);
        }
        None => state.engine.cancel_all_threads(actor),
    }
    Ok(StatusCode::OK)
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
