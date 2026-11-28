//! The thread terminal's HTTP routes (ADRs 0356 and 0820). The socket lives
//! in [`super::terminal_socket`].
//!
//! INVARIANT: While the switch is off, nothing here starts a shell, hands out
//! a socket ticket or types a suggestion. Ending a shell is never refused.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::error::ApiError;
use super::secret_reveal::{reveal_request_allowed, RefererRule};
use super::socket_ticket::{SocketTarget, SocketTicketView};
use super::AppState;
use crate::core::subprocess_origin::{subprocess_origin, SubprocessOrigin};
use crate::engine::terminal::suggestion::{self, PostError};
use crate::engine::terminal::{
    self, environment, lifecycle, OpenError, OpenSpec, Terminal, TypeError,
};
use crate::engine::terminal_folder::{self, FolderError};
use crate::engine::thread_events::{EventMeta, MessageOrigin, TerminalExitCause};
use crate::runtime::pty::Size;

const DISABLED: &str = "The terminal is off. Turn it on in Settings.";
const NOT_FROM_THE_SHELL: &str = "A terminal opens from the Lucidos page itself, never \
     from an app page. This request named an app page, or no page at all. If this is Lucidos, \
     the browser is withholding the page address (the Referer header): allow it for this site.";
const NEEDS_A_DEVICE: &str = "A terminal opens from a paired device, in the Lucidos UI.";
const GONE: &str = "That terminal has ended. Open a new shell.";
const AGENTS_ONLY: &str = "A terminal suggestion is posted by an agent inside its own thread, \
     through `lucidos terminal suggest`. It lands on that thread.";
const BUSY: &str = "A program is running in the terminal, so the command was not typed. \
     Copy it instead, or wait for the prompt.";

const FIRST_PROMPT_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

pub(super) fn disabled() -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, DISABLED).with_reason(lifecycle::DISABLED_REASON)
}

#[derive(Deserialize)]
struct OpenRequest {
    #[serde(default = "default_cols")]
    cols: u16,
    #[serde(default = "default_rows")]
    rows: u16,
}

fn default_cols() -> u16 {
    Size::DEFAULT.cols
}

fn default_rows() -> u16 {
    Size::DEFAULT.rows
}

/// A terminal as a client sees it.
#[derive(Serialize)]
struct TerminalView {
    terminal_id: Uuid,
    cols: u16,
    rows: u16,
    attachments: usize,
}

impl TerminalView {
    fn of(terminal: &Terminal) -> TerminalView {
        let size = terminal.size();
        TerminalView {
            terminal_id: terminal.id,
            cols: size.cols,
            rows: size.rows,
            attachments: terminal.attachment_count(),
        }
    }
}

#[derive(Serialize)]
struct OpenedTerminal {
    #[serde(flatten)]
    terminal: TerminalView,
    #[serde(flatten)]
    ticket: SocketTicketView,
}

#[derive(Serialize)]
struct TerminalStatus {
    enabled: bool,
    terminal: Option<TerminalView>,
}

/// The device a request comes from. Only a person at a paired device opens
/// or attaches to a shell.
async fn device_caller(state: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    if !reveal_request_allowed(headers, RefererRule::Required) {
        crate::log!("[Terminal] Refusing a request from an app document or an unnamed page");
        return Err(ApiError::new(StatusCode::FORBIDDEN, NOT_FROM_THE_SHELL));
    }
    match super::actor::require_user_actor(headers, &state.pool, None).await? {
        MessageOrigin::Device { device_id } => Ok(device_id),
        _ => Err(ApiError::new(StatusCode::FORBIDDEN, NEEDS_A_DEVICE)),
    }
}

async fn mint_ticket(
    state: &AppState,
    device_id: &str,
    terminal: &Terminal,
) -> Result<SocketTicketView, ApiError> {
    let generation = crate::core::snapshot::boundary::read(&state.pool)
        .await
        .map_err(ApiError::db)?
        .map(|boundary| boundary.restore_id);
    let target = SocketTarget::Terminal {
        thread_id: terminal.thread_id,
        terminal_id: terminal.id,
    };
    let ticket = state
        .socket_tickets
        .mint(device_id, target, generation)
        .ok_or_else(|| ApiError::internal("The socket ticket store is poisoned."))?;
    Ok(SocketTicketView::new(ticket))
}

fn folder_refusal(error: FolderError) -> ApiError {
    match error {
        FolderError::NoSuchThread => ApiError::not_found("That thread does not exist."),
        FolderError::Archived => ApiError::new(
            StatusCode::CONFLICT,
            "That thread is archived. Unarchive it to open a terminal.",
        ),
        FolderError::NoWorkingFolder => ApiError::new(
            StatusCode::CONFLICT,
            "This coding-agent thread has no working folder yet. Send it a message first.",
        ),
        FolderError::Failed(message) => {
            crate::log!(
                "[Terminal] Could not resolve a working folder: {}",
                lucidos_diagnostic_log::private(message)
            );
            ApiError::internal("Could not find where this thread's shell should start.")
        }
    }
}

fn open_refusal(error: OpenError) -> ApiError {
    match error {
        OpenError::Disabled => disabled(),
        OpenError::CapReached => ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "This workspace already has {} terminals open. End one first.",
                terminal::WORKSPACE_CAP
            ),
        )
        .with_reason("terminal-cap"),
        OpenError::Spawn(e) => {
            crate::log!(
                "[Terminal] Could not start a shell: {}",
                lucidos_diagnostic_log::private(e.to_string())
            );
            ApiError::internal("Could not start a shell on this computer.")
        }
    }
}

async fn open(
    State(state): State<AppState>,
    Path(thread_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<OpenRequest>,
) -> Result<Json<OpenedTerminal>, ApiError> {
    if !lifecycle::switch_is_on(&state.pool).await {
        return Err(disabled());
    }
    let device_id = device_caller(&state, &headers).await?;
    let user_env =
        crate::core::EnvironmentVariableStore::spawn_pairs(&state.pool, "Terminal").await;
    let folder = terminal_folder::resolve(&state.engine, thread_id, &user_env)
        .await
        .map_err(folder_refusal)?;
    let workspace = state.engine.workspace_path();
    let spec = OpenSpec {
        thread_id,
        env: environment::for_thread(workspace, thread_id, &user_env, &folder.profile),
        cwd: folder.cwd,
        size: Size::clamped(request.cols, request.rows),
        actor: Some(MessageOrigin::Device {
            device_id: device_id.clone(),
        }),
    };
    let pool = state.pool.clone();
    let terminal = state
        .engine
        .terminals
        .open(&state.engine.event_bus, workspace, spec, || {
            lifecycle::switch_is_on(&pool)
        })
        .await
        .map_err(open_refusal)?;
    let ticket = mint_ticket(&state, &device_id, &terminal).await?;
    Ok(Json(OpenedTerminal {
        terminal: TerminalView::of(&terminal),
        ticket,
    }))
}

async fn status(
    State(state): State<AppState>,
    Path(thread_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<TerminalStatus>, ApiError> {
    super::actor::require_user_actor(&headers, &state.pool, None).await?;
    if !lifecycle::switch_is_on(&state.pool).await {
        return Ok(Json(TerminalStatus {
            enabled: false,
            terminal: None,
        }));
    }
    let terminal = state.engine.terminals.get(thread_id).await;
    Ok(Json(TerminalStatus {
        enabled: true,
        terminal: terminal.as_deref().map(TerminalView::of),
    }))
}

/// The thread's live terminal, when it is the one the client names.
pub(super) async fn named_terminal(
    state: &AppState,
    thread_id: Uuid,
    terminal_id: Uuid,
) -> Result<std::sync::Arc<Terminal>, ApiError> {
    state
        .engine
        .terminals
        .get(thread_id)
        .await
        .filter(|terminal| terminal.id == terminal_id)
        .ok_or_else(|| ApiError::not_found(GONE))
}

async fn ticket(
    State(state): State<AppState>,
    Path((thread_id, terminal_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<SocketTicketView>, ApiError> {
    if !lifecycle::switch_is_on(&state.pool).await {
        return Err(disabled());
    }
    let device_id = device_caller(&state, &headers).await?;
    let terminal = named_terminal(&state, thread_id, terminal_id).await?;
    Ok(Json(mint_ticket(&state, &device_id, &terminal).await?))
}

async fn kill(
    State(state): State<AppState>,
    Path((thread_id, terminal_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    super::actor::require_user_actor(&headers, &state.pool, None).await?;
    named_terminal(&state, thread_id, terminal_id).await?;
    state
        .engine
        .terminals
        .end(thread_id, TerminalExitCause::Killed)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SuggestRequest {
    command: String,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Serialize)]
struct PostedSuggestion {
    suggestion_id: Uuid,
}

fn suggestion_refusal(refusal: suggestion::Refusal) -> ApiError {
    ApiError::bad_request(refusal.message())
}

/// An agent posts a command for the user to run, on the agent's own thread.
async fn suggest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SuggestRequest>,
) -> Result<(StatusCode, Json<PostedSuggestion>), ApiError> {
    let SubprocessOrigin::Subprocess {
        source_thread_id: Some(thread_id),
        ..
    } = subprocess_origin(&headers)
    else {
        return Err(ApiError::new(StatusCode::FORBIDDEN, AGENTS_ONLY));
    };
    let actor = super::actor::user_actor(&headers, None);
    let suggestion_id = suggestion::post(
        &state.engine.event_bus,
        thread_id,
        &request.command,
        request.label.as_deref(),
        EventMeta::with_actor(actor),
    )
    .await
    .map_err(|error| match error {
        PostError::Refused(refusal) => suggestion_refusal(refusal),
        PostError::Emit(message) => {
            crate::log!(
                "[Terminal] Could not post a suggestion on {}: {}",
                thread_id,
                lucidos_diagnostic_log::private(message)
            );
            ApiError::internal("Could not post the terminal suggestion.")
        }
    })?;
    Ok((
        StatusCode::CREATED,
        Json(PostedSuggestion { suggestion_id }),
    ))
}

#[derive(Deserialize)]
struct TypeRequest {
    command: String,
}

/// Type a suggestion at the shell's prompt, never pressing Enter.
async fn type_command(
    State(state): State<AppState>,
    Path((thread_id, terminal_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(request): Json<TypeRequest>,
) -> Result<StatusCode, ApiError> {
    if !lifecycle::switch_is_on(&state.pool).await {
        return Err(disabled());
    }
    device_caller(&state, &headers).await?;
    let terminal = named_terminal(&state, thread_id, terminal_id).await?;
    terminal.await_first_output(FIRST_PROMPT_WAIT).await;
    terminal
        .type_suggestion(&request.command)
        .map_err(|error| match error {
            TypeError::Busy => {
                ApiError::new(StatusCode::CONFLICT, BUSY).with_reason(suggestion::BUSY_REASON)
            }
            TypeError::Refused(refusal) => suggestion_refusal(refusal),
            TypeError::Write(message) => {
                crate::log!(
                    "[Terminal] Could not type into the terminal: {}",
                    lucidos_diagnostic_log::private(message)
                );
                ApiError::internal("Could not type into the terminal.")
            }
        })?;
    if state
        .engine
        .sign_in_contexts
        .posted()
        .is_sign_in(thread_id, &request.command)
    {
        if let Some(session) = state.engine.agent_sessions.lock().await.get_mut(&thread_id) {
            session.respawn_after_sign_in = true;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/threads/:thread_id/terminal", post(open).get(status))
        .route("/threads/:thread_id/terminal/:terminal_id", delete(kill))
        .route(
            "/threads/:thread_id/terminal/:terminal_id/ticket",
            post(ticket),
        )
        .route(
            "/threads/:thread_id/terminal/:terminal_id/type",
            post(type_command),
        )
        .route("/terminal-suggestions", post(suggest))
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
