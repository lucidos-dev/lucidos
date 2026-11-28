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
    inline_code, live_permission, menu, one_line, prepare::prepare_request, spend, thread_user,
    Action, PermissionCardText, PermissionUser, RequestCard,
};
use crate::engine::thread_events::MessageOrigin;

const REASON_MAX_CHARS: usize = 600;

const HEADER: &str = "Permission";

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
        refusal.status = StatusCode::FORBIDDEN;
        refusal
    })
}

pub(crate) fn snapshot_breadth(workspace: &str, handle: &str) -> String {
    format!("{workspace}/{handle}")
}

pub(crate) async fn spend_live(
    bus: &EventBus,
    user: &PermissionUser,
    action: Action,
    breadth: &str,
) -> bool {
    spend_covering(bus, user, action, breadth).await.is_some()
}

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

/// INVARIANT: Give `thread` back the once grant `spent`, because the act it paid for
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

/// INVARIANT: What a gate holds once it asked for its permission.
pub(crate) enum Spent {
    Allowed(Option<Uuid>),
    Requested(String),
    /// INVARIANT: A creator-bound lasting grant covered the mint; the gateway must recheck it.
    DeviceSessionScope,
}

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

pub(crate) async fn prepare(bus: &EventBus, caller: Uuid, ask: Ask) -> Result<String, ApiError> {
    prepare_request(bus, caller, ask.card(caller))
        .await
        .map_err(|e| {
            crate::log_legacy!("[PermissionRequest] Could not prepare a request on {caller}: {e}");
            ApiError::internal("permission_request_failed")
        })
}

pub(crate) fn gateway_control_ask(
    request: &str,
    summary: &str,
    reason: &str,
) -> Result<Ask, ApiError> {
    let displayed = request.replace('`', "\\u0060");
    Ok(Ask {
        action: Action::ControlGateway,
        breadth: request.into(),
        question: PermissionCardText::new("Send this gateway control request once?")
            .paragraph(summary)
            .paragraph("Exact request (method, path and body):")
            .payload("json", &displayed)
            .finish(Some(checked_reason(reason)?)),
        device_scope: None,
        declined: "The gateway setting stays unchanged.",
    })
}

pub(crate) fn host_log_ask(digest: &str, manifest: &str, reason: &str) -> Result<Ask, ApiError> {
    let reason = checked_reason(reason)?;
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request("Invalid host-log consent digest"));
    }
    Ok(Ask {
        action: Action::TransferHostLogs,
        breadth: digest.into(),
        question: PermissionCardText::new("Transfer these raw host logs once?")
            .paragraph("Raw host logs may contain secrets and text from OTHER WORKSPACES. The host captures the listed inode prefixes at execution time after approval, up to the listed byte counts. Consent cannot lift inherited OS confinement. Files go directly to a new operation directory under the exact SSH destination below. Only metadata and integrity status return to the agent. Deleted history cannot be recovered.")
            .payload("json", manifest)
            .finish(Some(reason)),
        device_scope: None,
        declined: "Don't allow",
    })
}

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
        PermissionCardText::new(&format!(
            "Grant a device session in workspace {} for device {}, for {ttl} seconds?",
            inline_code(&workspace),
            inline_code(&device),
        ))
        .paragraph(
            "The page may act as your device in that workspace. Stored secrets stay unavailable.",
        )
        .finish(Some(reason))
    } else {
        let noun = if action == Action::RestoreWorkspace {
            "Trash entry"
        } else {
            "workspace"
        };
        PermissionCardText::new(&format!(
            "Let this thread {} {noun} {}?",
            action.word(),
            inline_code(target)
        ))
        .paragraph("Allow once lets it do that one act, one time.")
        .finish(Some(reason))
    };
    Ok(Ask {
        device_scope: None,
        action,
        breadth: target.to_owned(),
        question,
        declined: "Nothing changes. The agent is told no.",
    })
}

/// INVARIANT: Only the broker calls this after the gateway reports matching creation provenance.
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
        question: PermissionCardText::new(&format!(
            "Restore snapshot {} in workspace {}?",
            inline_code(handle),
            inline_code(target)
        ))
        .paragraph(
            "This replaces the workspace database and files with the snapshot. All running \
             threads stop; archived work does not resume. Current device access and pairing \
             are preserved. A verified recovery snapshot is saved first. Allow once permits \
             only this snapshot in this workspace.",
        )
        .finish(Some(checked_reason(reason)?)),
        declined: "The workspace stays as it is.",
    })
}

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
        question: PermissionCardText::new(&format!(
            "Let this thread {} the event hook \u{201c}{}\u{201d}?",
            action.word(),
            one_line(slug)
        ))
        .paragraph(&format!(
            "{effect} Allow once lets it do that one act, one time."
        ))
        .finish(Some(checked_reason(reason)?)),
        declined: "The hook stays as it is. The agent is told no.",
    })
}

pub(crate) const LADDER_BREADTH: &str = "intelligence-ladder";

pub(crate) fn ladder_ask(reason: &str) -> Result<Ask, ApiError> {
    Ok(Ask {
        device_scope: None,
        action: Action::EditIntelligenceLadder,
        breadth: LADDER_BREADTH.to_owned(),
        question: PermissionCardText::new("Let this thread edit your intelligence ladders?")
            .paragraph(
                "It may turn rungs on or off, reorder them, or set a default rung, on any \
             surface. Allow once lets it make one edit.",
            )
            .finish(Some(checked_reason(reason)?)),
        declined: "The ladders stay as they are. The agent is told no.",
    })
}

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
        question: PermissionCardText::new(heading)
            .paragraph(&format!("It asks for one act: {act}."))
            .finish(Some(reason)),
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
    #[serde(default)]
    target_trigger_id: Option<String>,
    #[serde(default)]
    folders: Vec<String>,
    #[serde(default)]
    programs: Vec<String>,
    reason: String,
}

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

pub(crate) const PERMISSION_REQUIRED: &str = "permission_required";

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

async fn thread_title(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<String, ApiError> {
    let row: Option<Option<String>> =
        sqlx::query_scalar("SELECT title FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    match row {
        Some(title) => Ok(title.unwrap_or_else(|| "Untitled thread".to_owned())),
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

#[cfg(any(test, feature = "test-support"))]
#[path = "permission_request_fixtures.rs"]
pub(crate) mod test_fixtures;

#[cfg(test)]
#[path = "permission_requests_tests.rs"]
pub(crate) mod tests;
