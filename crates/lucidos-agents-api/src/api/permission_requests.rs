//! `POST /api/v1/permission-requests`: an agent prepares a *permission
//! request* for an act outside its own authority (ADR 0664).
//!
//! The engine words the card. The agent supplies a reason, which the card
//! quotes as the agent's, and writes no part of what the owner taps. The
//! request lands on the thread the caller's *thread-bound origin token* names.
//! The agent then asks its question tool with the id this route returns, and
//! `engine::permission::prepare` swaps in the card.

use crate::engine::trigger_writes::RequestedTriggerGrants;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::thread_reach::authority_held;
use crate::api::AppState;
use crate::core::subprocess_origin::SubprocessOrigin;
use crate::engine::agent_question::is_engine_card;
use crate::engine::event_bus::EventBus;
use crate::engine::permission::{
    live_permission, menu, one_line, prepare::prepare_request, spend, thread_user, Action,
    PermissionUser, RequestCard,
};
use crate::engine::thread_events::MessageOrigin;

/// The longest reason the card quotes, in characters. The card is one screen.
const REASON_MAX_CHARS: usize = 600;

/// A permission request's card header.
const HEADER: &str = "Permission";

/// What one gate asks the owner for: the action, its exact target and the
/// card's question.
pub(crate) struct Ask {
    pub(crate) action: Action,
    pub(crate) breadth: String,
    pub(crate) question: String,
    device_scope: Option<crate::engine::permission::GrantBreadth>,
    pub(crate) declined: &'static str,
}

impl Ask {
    fn card(self, asker: Uuid) -> RequestCard {
        let mut options = menu::options(self.action, asker, self.declined);
        if let Some(scope) = self.device_scope {
            use crate::engine::permission::{
                GrantBreadth, Holder, Lifetime, PermissionChoice, RequestOption,
            };
            if let GrantBreadth::DeviceSession {
                creator_workspace,
                device_id,
                ttl_secs,
            } = &scope
            {
                let detail = format!("Device `{device_id}`, exactly {ttl_secs} seconds per session, in current and future machine-made workspaces created by threads in `{creator_workspace}`.");
                for (label, lifetime, holder, suffix) in [
                    (menu::ALLOW_FOR_THIS_THREAD_LABEL, Lifetime::ThisThread, Holder::Thread { thread_id: asker }, "For the rest of this thread."),
                    (menu::ALWAYS_ALLOW_LABEL, Lifetime::Standing, Holder::Workspace, "Any thread in this source workspace may mint without asking until you revoke this permission."),
                ] {
                    options.insert(options.len() - 1, RequestOption {
                        label: label.into(), description: format!("{detail} {suffix}"),
                        choice: PermissionChoice::Grant { lifetime, holder, breadth: scope.clone() },
                    });
                }
            }
        }
        RequestCard {
            question: self.question,
            header: HEADER,
            action: self.action,
            breadth: Some(self.breadth),
            options,
            batch: None,
        }
    }
}

/// Who is answering this card. An answer to a card the engine worded can
/// grant a permission, so it is refused to anyone but the owner at a
/// registered device. A card that cannot be classified is refused rather than
/// read as an ordinary one.
pub(in crate::api) async fn answering_actor(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    headers: &HeaderMap,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<MessageOrigin>, ApiError> {
    let engine_card = is_engine_card(pool, thread_id, tool_use_id)
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Could not read the question card: {e}"),
            )
        })?;
    if !engine_card {
        return Ok(crate::api::actor::user_actor(headers, None));
    }
    crate::api::actor::require_owner_device(
        headers,
        pool,
        event_bus,
        None,
        "answer a permission request",
    )
    .await
    .map(Some)
    .map_err(|mut refusal| {
        // Permission answers have always returned 403 for unidentified callers.
        refusal.status = StatusCode::FORBIDDEN;
        refusal
    })
}

/// The breadth a snapshot restore names: one snapshot in one workspace.
pub(crate) fn snapshot_breadth(workspace: &str, handle: &str) -> String {
    format!("{workspace}/{handle}")
}

/// Spend `user`'s live permission for `action` at `breadth`. `true` means the
/// act may run: a lasting grant covers it, or a once grant's spend landed.
/// An unreadable store, a lost race or a failed write is `false`.
pub(crate) async fn spend_live(
    bus: &EventBus,
    user: &PermissionUser,
    action: Action,
    breadth: &str,
) -> bool {
    spend_covering(bus, user, action, breadth).await.is_some()
}

/// [`spend_live`], naming a spent once grant so a refused act can give it
/// back with [`restore_spent`]. `None` means no live permission covered it.
pub(crate) async fn spend_live_once(
    bus: &EventBus,
    user: &PermissionUser,
    action: Action,
    breadth: &str,
) -> Option<Option<Uuid>> {
    let permission = spend_covering(bus, user, action, breadth).await?;
    let once = permission.lifetime == crate::engine::permission::Lifetime::Once.wire();
    Some(once.then_some(permission.permission_id))
}

/// Give `thread` back the once grant `spent`, because the act it paid for
/// was refused and changed nothing.
pub(crate) async fn restore_spent(
    bus: &EventBus,
    thread: Uuid,
    spent: Uuid,
) -> Result<(), ApiError> {
    use crate::engine::event_bus::BusEvent;
    use crate::engine::thread_events::{EventMeta, ThreadEvent};
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::PermissionRestored {
            spent,
            restored: Uuid::new_v4(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .map(|_| ())
    .map_err(|e| {
        ApiError::internal(format!(
            "The act was refused, but restoring its permission failed: {e}"
        ))
    })
}

/// The row whose spend succeeded, including a lasting permission's no-op spend.
async fn spend_covering(
    bus: &EventBus,
    user: &PermissionUser,
    action: Action,
    breadth: &str,
) -> Option<crate::engine::permission::PermissionRow> {
    match live_permission(bus.pool(), user, action, Some(breadth)).await {
        Ok(Some(permission)) if spend(bus, &permission).await => Some(permission),
        Ok(_) => None,
        Err(e) => {
            crate::log_legacy!(
                "[PermissionRequest] Could not read the permissions of {user:?}: {e}; \
                 treating it as holding none"
            );
            None
        }
    }
}

/// What a gate holds once it asked for its permission.
pub(crate) enum Spent {
    /// A live permission covered the act. Carries its id only for a spent once grant.
    Allowed(Option<Uuid>),
    /// None was live. The request is prepared and the agent asks with this id.
    Requested(String),
    /// A creator-bound lasting grant covered the mint; the gateway must recheck it.
    DeviceSessionScope,
}

/// Spend the caller's live permission for `ask`, or prepare a request for
/// one. Never both, and never an act without a spend.
pub(crate) async fn spend_or_prepare(
    bus: &EventBus,
    caller: Uuid,
    ask: Ask,
) -> Result<Spent, ApiError> {
    if let Some(key) = ask
        .device_scope
        .as_ref()
        .and_then(|scope| scope.device_scope_key())
    {
        if spend_covering(bus, &thread_user(caller), ask.action, &key)
            .await
            .is_some()
        {
            return Ok(Spent::DeviceSessionScope);
        }
    }
    if let Some(once) = spend_live_once(bus, &thread_user(caller), ask.action, &ask.breadth).await {
        return Ok(Spent::Allowed(once));
    }
    Ok(Spent::Requested(prepare(bus, caller, ask).await?))
}

/// Prepare `ask` on `caller`, and return the request id the agent asks with.
pub(crate) async fn prepare(bus: &EventBus, caller: Uuid, ask: Ask) -> Result<String, ApiError> {
    prepare_request(bus, caller, ask.card(caller))
        .await
        .map_err(|e| {
            crate::log_legacy!("[PermissionRequest] Could not prepare a request on {caller}: {e}");
            ApiError::internal("permission_request_failed")
        })
}

/// The card's question, in the engine's words, with the agent's reason quoted
/// line by line. A lone `\r` breaks a line too, as in the card's renderer. So
/// no blank line can close the quote and pose as engine text.
fn question(heading: &str, detail: &str, reason: &str) -> String {
    let quoted: String = reason
        .split(['\n', '\r'])
        .map(|line| format!("> {line}\n"))
        .collect();
    format!("**{heading}**\n\n{detail}\n\nThe agent's reason:\n\n{quoted}")
}

/// Only the table-bounded gateway route constructs this exact request.
pub(crate) fn gateway_control_ask(
    request: &str,
    summary: &str,
    reason: &str,
) -> Result<Ask, ApiError> {
    // JSON escaping keeps caller text from closing the card's code fence.
    let displayed = request.replace('`', "\\u0060");
    Ok(Ask {
        action: Action::ControlGateway,
        breadth: request.into(),
        question: question(
            "Send this gateway control request once?",
            &format!(
                "{summary}\n\nExact request (method, path and body):\n\n```json\n{displayed}\n```"
            ),
            checked_reason(reason)?,
        ),
        device_scope: None,
        declined: "The gateway setting stays unchanged.",
    })
}

/// Only the typed host-log broker can construct this once-only request.
pub(crate) fn host_log_ask(digest: &str, manifest: &str, reason: &str) -> Result<Ask, ApiError> {
    let reason = checked_reason(reason)?;
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request("Invalid host-log consent digest"));
    }
    Ok(Ask {
        action: Action::TransferHostLogs,
        breadth: digest.into(),
        question: question("Transfer these raw host logs once?",
            &format!("Raw host logs may contain secrets and text from OTHER WORKSPACES. The host captures the listed inode prefixes at execution time after approval, up to the listed byte counts. Consent cannot lift inherited OS confinement. Files go directly to a new operation directory under the exact SSH destination below. Only metadata and integrity status return to the agent. Deleted history cannot be recovered.\n\n```json\n{manifest}\n```"), reason),
        device_scope: None,
        declined: "Don't allow",
    })
}

/// A workspace broker verb's request. It names the verb and one target: a
/// workspace slug, or a Trash entry for a restore.
pub(crate) fn workspace_ask(action: Action, target: &str, reason: &str) -> Result<Ask, ApiError> {
    if !action.is_workspace_verb() || !valid_workspace_target(action, target) {
        return Err(ApiError::bad_request(format!(
            "Name the workspace {} aims at by its slug (a Trash entry by its id).",
            action.act()
        )));
    }
    let reason = checked_reason(reason)?;
    let question = if let Some((workspace, device, ttl)) = (action == Action::GrantDeviceSession)
        .then(|| decode_device_session_target(target))
        .flatten()
    {
        question(
            &format!(
                "Grant a device session in workspace `{workspace}` for device `{device}`, for \
                 {ttl} seconds?"
            ),
            "The page may act as your device in that workspace. Stored secrets stay unavailable.",
            reason,
        )
    } else {
        let noun = if action == Action::RestoreWorkspace {
            "Trash entry"
        } else {
            "workspace"
        };
        question(
            &format!("Let this thread {} {noun} `{target}`?", action.word()),
            "Allow once lets it do that one act, one time.",
            reason,
        )
    };
    Ok(Ask {
        device_scope: None,
        action,
        breadth: target.to_owned(),
        question,
        declined: "Nothing changes. The agent is told no.",
    })
}

/// Only the broker calls this after the gateway reports matching creation provenance.
pub(crate) fn device_session_ask(
    target: &str,
    reason: &str,
    source: &str,
) -> Result<Ask, ApiError> {
    let mut ask = workspace_ask(Action::GrantDeviceSession, target, reason)?;
    ask.device_scope = Some(
        crate::engine::permission::GrantBreadth::device_session(source, target)
            .ok_or_else(|| ApiError::bad_request("Invalid device session scope"))?,
    );
    Ok(ask)
}

/// A session request binds all mint parameters in one canonical breadth.
pub(super) fn device_session_target(
    workspace: &str,
    device: &str,
    ttl: u64,
) -> Result<String, ApiError> {
    let target = serde_json::json!([workspace, device, ttl]).to_string();
    decode_device_session_target(&target)
        .ok_or_else(|| ApiError::bad_request("Invalid device session target"))?;
    Ok(target)
}

pub(crate) fn decode_device_session_target(target: &str) -> Option<(String, String, u64)> {
    crate::engine::permission::device_session::decode_target(target)
}

/// A workspace verb names a workspace slug. A Trash restore names the Trash
/// entry instead, in the gateway's `workspace-` entry id form. A device
/// session names its canonical mint parameters.
fn valid_workspace_target(action: Action, target: &str) -> bool {
    if action == Action::GrantDeviceSession {
        decode_device_session_target(target).is_some()
    } else if action == Action::RestoreWorkspace {
        target.starts_with("workspace-")
            && target.len() <= 128
            && target
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    } else {
        lucidos_gateway_constants::is_valid_workspace_id(target)
    }
}

/// A snapshot restore's request: one snapshot, in one other workspace.
pub(crate) fn snapshot_ask(
    source: &str,
    target: &str,
    handle: &str,
    reason: &str,
) -> Result<Ask, ApiError> {
    let bad = || ApiError::new(StatusCode::BAD_REQUEST, "snapshot_request_target_invalid");
    if !lucidos_gateway_constants::is_valid_workspace_id(target)
        || crate::core::snapshot::handle::Handle::try_from(handle.to_owned()).is_err()
    {
        return Err(bad());
    }
    if target == source {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "snapshot_restore_own_workspace_refused",
        ));
    }
    Ok(Ask {
        device_scope: None,
        action: Action::RestoreSnapshot,
        breadth: snapshot_breadth(target, handle),
        question: question(
            &format!("Restore snapshot `{handle}` in workspace `{target}`?"),
            "This replaces the workspace database and files with the snapshot. All running \
             threads stop; archived work does not resume. Current device access and pairing \
             are preserved. A verified recovery snapshot is saved first. Allow once permits \
             only this snapshot in this workspace.",
            checked_reason(reason)?,
        ),
        declined: "The workspace stays as it is.",
    })
}

/// An event hook verb's request: one verb, on one hook, at `breadth`.
/// `effect` says what the act does, in the engine's words.
pub(crate) fn hook_ask(
    action: Action,
    breadth: String,
    slug: &str,
    effect: &str,
    reason: &str,
) -> Result<Ask, ApiError> {
    Ok(Ask {
        device_scope: None,
        action,
        breadth,
        question: question(
            &format!(
                "Let this thread {} the event hook \u{201c}{}\u{201d}?",
                action.word(),
                one_line(slug)
            ),
            &format!("{effect} Allow once lets it do that one act, one time."),
            checked_reason(reason)?,
        ),
        declined: "The hook stays as it is. The agent is told no.",
    })
}

/// The breadth a ladder edit names. One target covers every surface, so a
/// grant for this thread covers every ladder edit it makes.
pub(crate) const LADDER_BREADTH: &str = "intelligence-ladder";

/// A ladder edit's request: any edit to any surface's intelligence ladder.
pub(crate) fn ladder_ask(reason: &str) -> Result<Ask, ApiError> {
    Ok(Ask {
        device_scope: None,
        action: Action::EditIntelligenceLadder,
        breadth: LADDER_BREADTH.to_owned(),
        question: question(
            "Let this thread edit your intelligence ladders?",
            "It may turn rungs on or off, reorder them, or set a default rung, on any \
             surface. Allow once lets it make one edit.",
            checked_reason(reason)?,
        ),
        declined: "The ladders stay as they are. The agent is told no.",
    })
}

/// A thread verb's request: one verb, at one thread.
pub(crate) async fn thread_ask(
    pool: &sqlx::PgPool,
    caller: Uuid,
    action: Action,
    target: Option<Uuid>,
    reason: &str,
) -> Result<Ask, ApiError> {
    let bad = |msg: String| ApiError::new(StatusCode::BAD_REQUEST, msg);
    if !action.needs_authority() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "{} needs no permission. Do it without a card.",
                action.act()
            ),
        ));
    }
    if !action.proposable() || menu::menu(action).is_none() {
        return Err(bad(format!(
            "No agent asks for {}: that is the owner's own act, on screen.",
            action.act()
        )));
    }
    let Some(target) = target else {
        return Err(bad(format!(
            "Name the thread {} aims at, with --thread <id>.",
            action.act()
        )));
    };
    let reason = checked_reason(reason)?;
    let title = thread_title(pool, target).await?;
    if authority_held(pool, Some(caller), None, Some(target), action)
        .await
        .map_err(ApiError::from)?
        .is_some()
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "You already hold the authority for {}: it is inside your own subtree, or \
                 this turn carries the owner's standing instruction. Do it without a card.",
                action.act()
            ),
        ));
    }
    let act = format!("{} on \u{201c}{}\u{201d}", action.act(), one_line(&title));
    let (heading, declined) = if action == Action::SubmitDraft {
        (
            "Let this thread submit your saved draft?",
            "The draft stays unsent, for you to send or change.",
        )
    } else {
        (
            "Let this thread act outside its own subtree?",
            "The thread stays inside its own subtree.",
        )
    };
    Ok(Ask {
        device_scope: None,
        action,
        breadth: target.to_string(),
        question: question(heading, &format!("It asks for one act: **{act}**."), reason),
        declined,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api) struct PreparePermissionRequestBody {
    action: Action,
    #[serde(default)]
    target_thread_id: Option<Uuid>,
    #[serde(default)]
    target_workspace_id: Option<String>,
    #[serde(default)]
    snapshot_handle: Option<String>,
    /// The trigger a trigger grant names, by id.
    #[serde(default)]
    target_trigger_id: Option<String>,
    #[serde(default)]
    folders: Vec<String>,
    #[serde(default)]
    programs: Vec<String>,
    reason: String,
}

/// Prepare a grant change stamped with the facts and definition the owner sees.
async fn prepare_trigger_grant(
    state: &AppState,
    caller: Uuid,
    body: PreparePermissionRequestBody,
) -> Result<String, ApiError> {
    if body.target_thread_id.is_some()
        || body.target_workspace_id.is_some()
        || body.snapshot_handle.is_some()
    {
        return Err(ApiError::bad_request(
            "A trigger grant names one trigger with --trigger, and no other target.",
        ));
    }
    let trigger_id = body.target_trigger_id.as_deref().ok_or_else(|| {
        ApiError::bad_request("Name the trigger the grant aims at with --trigger <id>.")
    })?;
    let reason = checked_reason(&body.reason)?;
    let config = state
        .engine
        .trigger_configs
        .read()
        .unwrap()
        .get(trigger_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("Trigger not found"))?;
    if (body.action == Action::GrantTriggerPrograms && !body.folders.is_empty())
        || (body.action == Action::GrantTriggerFolders && !body.programs.is_empty())
    {
        return Err(ApiError::bad_request(
            "--folder requires grant-trigger-folders. --program requires grant-trigger-programs.",
        ));
    }
    let writer = state.engine.trigger_registry_writer();
    if writer
        .grant_card_channel(caller)
        .await
        .map_err(|error| {
            crate::log_legacy!("[PermissionRequest] Could not find the calling thread: {error}");
            ApiError::internal("Could not find the calling thread")
        })?
        .is_none()
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Request trigger grants from a work thread outside Home.",
        ));
    }
    let requested = RequestedTriggerGrants {
        folder_grants: body.folders,
        program_grants: body.programs,
    };
    let proposal = if requested.is_empty() {
        writer
            .propose_grants(&config, body.action)
            .await
            .map_err(|error| {
                crate::log_legacy!(
                    "[PermissionRequest] Could not read trigger grant evidence: {error}"
                );
                ApiError::internal("Could not read trigger grant evidence")
            })?
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "This trigger has no recorded grants of that kind left to add.",
                )
            })?
    } else {
        writer
            .propose_named_grants(&config, requested)
            .map_err(ApiError::bad_request)?
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "This trigger already holds the named grants.",
                )
            })?
    };
    prepare_request(
        &state.engine.event_bus,
        caller,
        proposal.card(&config, caller, Some(reason)),
    )
    .await
    .map_err(|error| {
        crate::log_legacy!(
            "[PermissionRequest] Could not prepare trigger grant on {caller}: {error}"
        );
        ApiError::internal("permission_request_failed")
    })
}

/// POST /api/v1/permission-requests: prepare a permission request on the
/// calling thread, and return the id the agent asks its question tool with.
///
/// Only an agent inside its own thread may call it. A caller already holding a
/// live permission for the act is told so, with 409, and acts with no card.
pub(in crate::api) async fn prepare_permission_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PreparePermissionRequestBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caller = calling_agent_thread(&headers)?;
    if body.action.is_trigger_grant() {
        let request_id = prepare_trigger_grant(&state, caller, body).await?;
        return Ok(Json(serde_json::json!({ "request_id": request_id })));
    }
    if body.target_trigger_id.is_some() || !body.folders.is_empty() || !body.programs.is_empty() {
        return Err(ApiError::bad_request(
            "Only a trigger grant may name a trigger, folders or programs.",
        ));
    }
    let ask = match body.action {
        Action::RestoreSnapshot => {
            let source = super::snapshots::restore_workspace_identity()?;
            if body.target_thread_id.is_some() {
                return Err(ApiError::bad_request(
                    "A snapshot restore names a workspace and a snapshot, and no thread.",
                ));
            }
            snapshot_ask(
                &source,
                body.target_workspace_id.as_deref().unwrap_or_default(),
                body.snapshot_handle.as_deref().unwrap_or_default(),
                &body.reason,
            )?
        }
        action if action.is_workspace_verb() => {
            if body.target_thread_id.is_some() || body.snapshot_handle.is_some() {
                return Err(ApiError::bad_request(
                    "A workspace request names a workspace with --workspace, and nothing else.",
                ));
            }
            workspace_ask(
                action,
                body.target_workspace_id.as_deref().unwrap_or_default(),
                &body.reason,
            )?
        }
        Action::EditIntelligenceLadder => {
            if body.target_thread_id.is_some()
                || body.target_workspace_id.is_some()
                || body.snapshot_handle.is_some()
            {
                return Err(ApiError::bad_request(
                    "A ladder edit request names no thread, workspace or snapshot.",
                ));
            }
            ladder_ask(&body.reason)?
        }
        action => {
            if body.target_workspace_id.is_some() || body.snapshot_handle.is_some() {
                return Err(ApiError::bad_request(
                    "A thread request names a thread with --thread, and nothing else.",
                ));
            }
            thread_ask(
                &state.pool,
                caller,
                action,
                body.target_thread_id,
                &body.reason,
            )
            .await?
        }
    };
    let held = live_permission(
        &state.pool,
        &thread_user(caller),
        ask.action,
        Some(&ask.breadth),
    )
    .await
    .map_err(|e| ApiError::internal(format!("Could not read your permissions: {e}")))?;
    if held.is_some() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "You already hold a permission for {}. Do it now, without a card.",
                ask.action.act()
            ),
        ));
    }
    let request_id = prepare(&state.engine.event_bus, caller, ask).await?;
    Ok(Json(serde_json::json!({ "request_id": request_id })))
}

/// The reason slug a client reads to ask the owner, then call again.
pub(crate) const PERMISSION_REQUIRED: &str = "permission_required";

/// 403 `permission_required`, with the id the agent asks the owner with.
pub(crate) fn permission_required(request_id: &str) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": "This act needs the owner's permission. Ask with the request id, then \
                      call again.",
            "reason": PERMISSION_REQUIRED,
            "request_id": request_id,
        })),
    )
        .into_response()
}

/// The calling agent's own thread, as its origin token's MAC proves it. Any
/// other caller is refused: the card must land on the thread that asks.
pub(super) fn calling_agent_thread(headers: &HeaderMap) -> Result<Uuid, ApiError> {
    match crate::core::subprocess_origin::subprocess_origin(headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id: Some(caller),
            ..
        } => Ok(caller),
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "A permission request is prepared by an agent inside its own thread, through \
             `lucidos ask-permission`. The card lands on that thread.",
        )),
    }
}

/// The reason, trimmed and capped. An empty one is refused: the owner reads it.
fn checked_reason(reason: &str) -> Result<&str, ApiError> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(ApiError::bad_request(
            "Give a reason with --reason. The owner reads it on the card.",
        ));
    }
    Ok(match reason.char_indices().nth(REASON_MAX_CHARS) {
        Some((end, _)) => &reason[..end],
        None => reason,
    })
}

/// The target's title, or a 404 naming the id when no such thread exists.
async fn thread_title(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<String, ApiError> {
    let row: Option<Option<String>> =
        sqlx::query_scalar("SELECT title FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    match row {
        Some(title) => Ok(title.unwrap_or_else(|| thread_id.to_string())),
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("No thread {thread_id} exists."),
        )),
    }
}

pub(super) fn router() -> axum::Router<AppState> {
    axum::Router::new().route(
        "/permission-requests",
        axum::routing::post(prepare_permission_request),
    )
}

#[cfg(test)]
#[path = "permission_requests_tests.rs"]
pub(crate) mod tests;
