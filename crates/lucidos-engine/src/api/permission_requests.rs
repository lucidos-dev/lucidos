//! `POST /api/v1/permission-requests`: an agent prepares a *permission
//! request* for an act outside its own authority (ADR 0664).
//!
//! The engine words the card. The agent supplies a reason, which the card
//! quotes as the agent's, and writes no part of what the owner taps. The
//! request lands on the thread the caller's *thread-bound origin token* names.
//! The agent then asks its question tool with the id this route returns, and
//! `engine::permission::prepare` swaps in the card.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::actor::SubprocessOrigin;
use crate::api::error::ApiError;
use crate::api::thread_reach::authority_held;
use crate::api::AppState;
use crate::engine::agent_question::is_engine_card;
use crate::engine::event_bus::EventBus;
use crate::engine::permission::{
    live_permission, menu, prepare::prepare_request, spend, Action, PermissionUser, RequestCard,
};
use crate::engine::thread_events::MessageOrigin;

/// The longest reason the card quotes, in characters. The card is one screen.
const REASON_MAX_CHARS: usize = 600;

/// The longest title the card shows, in characters.
const TITLE_MAX_CHARS: usize = 120;

/// A permission request's card header.
const HEADER: &str = "Permission";

/// What one gate asks the owner for: the action, its exact target and the
/// card's question.
pub(crate) struct Ask {
    pub(crate) action: Action,
    pub(crate) breadth: String,
    pub(crate) question: String,
    pub(crate) declined: &'static str,
}

impl Ask {
    fn card(self, asker: Uuid) -> RequestCard {
        RequestCard {
            question: self.question,
            header: HEADER,
            action: self.action,
            breadth: Some(self.breadth),
            options: menu::options(self.action, asker, self.declined),
            batch: None,
            unattested_act: None,
        }
    }
}

/// What an agent is told when it tries to answer an engine card.
const ENGINE_CARD_IS_THE_OWNERS: &str =
    "A permission request is answered by the workspace owner alone, from one of their own \
     devices. Its Allow grants a permission, so no agent may press it, including the one \
     that asked for it.";

/// Who is answering this card. An answer to a card the engine worded can
/// grant a permission, so it is refused to anyone but the owner at a
/// registered device. A card that cannot be classified is refused rather than
/// read as an ordinary one.
pub(in crate::api) async fn answering_actor(
    pool: &sqlx::PgPool,
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
        crate::api::owner_door::OnUnattested::Refuse,
    )
    .await
    .map(Some)
    .map_err(|_| ApiError::new(StatusCode::FORBIDDEN, ENGINE_CARD_IS_THE_OWNERS))
}

/// The breadth a snapshot restore names: one snapshot in one workspace.
pub(crate) fn snapshot_breadth(workspace: &str, handle: &str) -> String {
    format!("{workspace}/{handle}")
}

/// The asking thread as a permission user.
pub(crate) fn thread_user(thread_id: Uuid) -> PermissionUser {
    PermissionUser {
        thread_id: Some(thread_id),
        trigger_id: None,
    }
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
    match live_permission(bus.pool(), user, action, Some(breadth)).await {
        Ok(Some(permission)) => spend(bus, &permission).await,
        Ok(None) => false,
        Err(e) => {
            crate::log!(
                "[PermissionRequest] Could not read the permissions of {user:?}: {e}; \
                 treating it as holding none"
            );
            false
        }
    }
}

/// What a gate holds once it asked for its permission.
pub(crate) enum Spent {
    /// A live permission covered the act, and a once grant is now spent.
    Spent,
    /// None was live. The request is prepared and the agent asks with this id.
    Requested(String),
}

/// Spend the caller's live permission for `ask`, or prepare a request for
/// one. Never both, and never an act without a spend.
pub(crate) async fn spend_or_prepare(
    bus: &EventBus,
    caller: Uuid,
    ask: Ask,
) -> Result<Spent, ApiError> {
    if spend_live(bus, &thread_user(caller), ask.action, &ask.breadth).await {
        return Ok(Spent::Spent);
    }
    Ok(Spent::Requested(prepare(bus, caller, ask).await?))
}

/// Prepare `ask` on `caller`, and return the request id the agent asks with.
pub(crate) async fn prepare(bus: &EventBus, caller: Uuid, ask: Ask) -> Result<String, ApiError> {
    prepare_request(bus, caller, ask.card(caller))
        .await
        .map_err(|e| {
            crate::log!("[PermissionRequest] Could not prepare a request on {caller}: {e}");
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

/// A workspace broker verb's request. It names the verb and one target: a
/// workspace slug, or a Trash entry for a restore.
pub(crate) fn workspace_ask(action: Action, target: &str, reason: &str) -> Result<Ask, ApiError> {
    if !action.is_workspace_verb() || !valid_workspace_target(action, target) {
        return Err(ApiError::bad_request(format!(
            "Name the workspace {} aims at by its slug (a Trash entry by its id).",
            action.act()
        )));
    }
    let noun = if action == Action::RestoreWorkspace {
        "Trash entry"
    } else {
        "workspace"
    };
    Ok(Ask {
        action,
        breadth: target.to_owned(),
        question: question(
            &format!("Let this thread {} {noun} `{target}`?", action.word()),
            "Allow once lets it do that one act, one time.",
            checked_reason(reason)?,
        ),
        declined: "Nothing changes. The agent is told no.",
    })
}

/// A workspace verb names a workspace slug. A Trash restore names the Trash
/// entry instead, in the gateway's `workspace-` entry id form.
fn valid_workspace_target(action: Action, target: &str) -> bool {
    if action == Action::RestoreWorkspace {
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
    reason: String,
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

/// The calling agent's own thread, as its origin token's MAC proves it. Any
/// other caller is refused: the card must land on the thread that asks.
pub(super) fn calling_agent_thread(headers: &HeaderMap) -> Result<Uuid, ApiError> {
    match crate::api::actor::subprocess_origin(headers) {
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

/// A title on one line, capped, with its markdown escaped, so it cannot end
/// a card's bold sentence or start one of its own. Another thread may have
/// written it.
pub(crate) fn one_line(title: &str) -> String {
    let flat = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let capped = match flat.char_indices().nth(TITLE_MAX_CHARS) {
        Some((end, _)) => format!("{}\u{2026}", &flat[..end]),
        None => flat,
    };
    capped
        .chars()
        .flat_map(|c| {
            let escape = "\\`*_{}[]<>()#+-.!|~".contains(c);
            escape.then_some('\\').into_iter().chain(std::iter::once(c))
        })
        .collect()
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
