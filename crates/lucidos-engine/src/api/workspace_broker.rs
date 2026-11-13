//! The *workspace broker*: routes an agent calls to manage workspaces on this
//! machine (ADR 0671).
//!
//! Any workspace: create, stop, start, autostart, rename. A *machine-made
//! workspace* only: delete, and restore from Trash. Nothing restarts.
//!
//! - **The token stays here.** The engine calls its gateway with the local
//!   token. A refusal passes back only the gateway's message, redacted.
//! - **Agents only.** A caller needs a thread-bound origin token. The owner
//!   uses the picker.
//! - **One owner approval per call.** A call spends a live Allow once for its
//!   verb and target. With none, it records the request and refuses with
//!   `owner_approval_required`, and the agent asks with the request id.
//! - **The gateway decides.** The engine checks the target first, so the
//!   owner is never asked to approve an act the gateway would refuse.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::error::ApiError;
use super::gateway_control::{fetch_control, post_control, ControlRefusal};
use super::owner_approval::{
    calling_agent_thread, spend_or_request_workspace_approval, WorkspaceApproval,
};
use super::workspace_list::{list_from_gateway, ListedWorkspace, GATEWAY_DEADLINE};
use super::AppState;
use crate::engine::event_bus::EventBus;
use crate::engine::permission::Action;

/// The reason slug a client reads to ask the owner, then call again.
const OWNER_APPROVAL_REQUIRED: &str = "owner_approval_required";

/// What an agent reads when it asks for a restart.
const NO_RESTART: &str = "Lucidos has no workspace restart for agents: a restart can re-exec \
     the gateway itself. Stop the workspace, then start it.";

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/workspaces", post(create))
        .route("/workspaces/trash", get(list_trash))
        .route("/workspaces/trash/:id/restore", post(restore))
        .route("/workspaces/:id/stop", post(stop))
        .route("/workspaces/:id/start", post(start))
        .route("/workspaces/:id/autostart", post(autostart))
        .route("/workspaces/:id/rename", post(rename))
        .route("/workspaces/:id/delete", post(delete))
        .route("/workspaces/:id/restart", post(restart))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    name: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    autostart: bool,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReasonBody {
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AutostartBody {
    enabled: bool,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameBody {
    name: String,
    reason: String,
}

/// One Trash row, projected: serde drops what the gateway knows beyond this.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct TrashRow {
    id: String,
    name: String,
    status: String,
    restore_available: bool,
    #[serde(default)]
    machine_made: bool,
}

/// What a broker call came to.
enum Outcome {
    /// The gateway acted. Holds its success body.
    Done(String),
    /// No live approval: the request is recorded under this id.
    NeedsApproval(String),
}

/// Where a broker call runs: the event bus, the gateway, and this engine's own
/// workspace slug, which `created_by` and the delete caller name.
struct Broker<'a> {
    bus: &'a EventBus,
    port: String,
    own: String,
}

/// One broker call's verb, target and the agent's reason.
struct Call<'a> {
    verb: Action,
    target: &'a str,
    reason: &'a str,
}

impl<'a> Broker<'a> {
    fn of(state: &'a AppState) -> Result<Self, ApiError> {
        let own = super::base_path::workspace_id().ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "This engine does not know its own workspace slug, so the broker cannot \
                 record who acted.",
            )
        })?;
        Ok(Self {
            bus: &state.engine.event_bus,
            port: gateway()?,
            own,
        })
    }

    fn caller(&self, caller: Uuid) -> serde_json::Value {
        serde_json::json!({ "workspace_id": self.own, "thread_id": caller.to_string() })
    }

    /// A verb on a workspace the gateway lists. Delete also needs it machine-made.
    async fn on_listed(
        &self,
        headers: &HeaderMap,
        call: Call<'_>,
        gateway_body: serde_json::Value,
    ) -> Result<Response, ApiError> {
        let caller = calling_agent_thread(headers)?;
        let id = call.target;
        if matches!(call.verb, Action::StopWorkspace | Action::DeleteWorkspace) && id == self.own {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                format!(
                    "That is the workspace this agent runs in. To {} it would end this \
                     thread's turn, so the broker refuses it.",
                    call.verb.word()
                ),
            ));
        }
        let listing = list_from_gateway(&self.port, GATEWAY_DEADLINE).await?;
        if let Some(name) = gateway_body["name"].as_str() {
            refuse_a_taken_name(&listing, name, Some(id))?;
        }
        let listed = listing
            .into_iter()
            .find(|w| w.id == id)
            .ok_or_else(|| ApiError::not_found(format!("No workspace `{id}` exists.")))?;
        let (path, body) = if call.verb == Action::DeleteWorkspace {
            refuse_unless_machine_made(call.verb, listed.machine_made)?;
            (
                format!("/broker/workspaces/{id}/delete"),
                serde_json::json!({ "caller": self.caller(caller) }),
            )
        } else {
            (
                format!("/workspaces/{id}/{}", control_suffix(call.verb)),
                gateway_body,
            )
        };
        let outcome = self.act(caller, call, &path, body).await?;
        respond(outcome, |_| Ok(StatusCode::NO_CONTENT.into_response()))
    }

    /// Spend the approval, then call the gateway. Never the call without the spend.
    async fn act(
        &self,
        caller: Uuid,
        call: Call<'_>,
        path: &str,
        body: serde_json::Value,
    ) -> Result<Outcome, ApiError> {
        let Call {
            verb,
            target,
            reason,
        } = call;
        match spend_or_request_workspace_approval(self.bus, caller, verb, target, reason).await? {
            WorkspaceApproval::Requested(id) => return Ok(Outcome::NeedsApproval(id)),
            WorkspaceApproval::Spent => {}
        }
        let answer = tokio::time::timeout(BROKER_DEADLINE, post_control(&self.port, path, &body))
            .await
            .unwrap_or_else(|_| {
                Err(ControlRefusal::Unreachable(format!(
                    "the gateway did not answer within {BROKER_DEADLINE:?}"
                )))
            });
        match answer {
            Ok(done) => {
                crate::log!(
                    "[WorkspaceBroker] Thread {caller}: {} `{target}` done",
                    verb.act()
                );
                Ok(Outcome::Done(done))
            }
            Err(refusal) => Err(refused(verb, target, refusal)),
        }
    }
}

async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Result<Response, ApiError> {
    let name = body.name.trim().to_owned();
    if name.is_empty() {
        return Err(ApiError::bad_request("Give the workspace a name."));
    }
    let slug = body
        .slug
        .clone()
        .unwrap_or_else(|| lucidos_gateway_constants::slugify(&name));
    if !lucidos_gateway_constants::is_valid_workspace_id(&slug) {
        return Err(ApiError::bad_request(format!(
            "`{slug}` is not a workspace slug: use lowercase letters, digits and inner dashes."
        )));
    }
    let caller = calling_agent_thread(&headers)?;
    let broker = Broker::of(&state)?;
    let listing = list_from_gateway(&broker.port, GATEWAY_DEADLINE).await?;
    if let Some(taken) = listing.iter().find(|w| w.id == slug) {
        return Err(taken_conflict(taken));
    }
    refuse_a_taken_name(&listing, &name, None)?;
    let gateway_body = serde_json::json!({
        "name": name,
        "slug": slug,
        "autostart": body.autostart,
        "created_by": broker.caller(caller),
    });
    let call = Call {
        verb: Action::CreateWorkspace,
        target: &slug,
        reason: &body.reason,
    };
    let outcome = broker
        .act(caller, call, "/broker/workspaces", gateway_body)
        .await?;
    respond(outcome, |done| {
        #[derive(Deserialize)]
        struct Created {
            workspace: ListedWorkspace,
        }
        let created: Created = serde_json::from_str(&done).map_err(|_| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "The gateway created the workspace but its answer is not one this engine reads.",
            )
        })?;
        Ok((
            StatusCode::CREATED,
            Json(serde_json::json!({ "workspace": created.workspace })),
        )
            .into_response())
    })
}

async fn stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ReasonBody>,
) -> Result<Response, ApiError> {
    let call = Call {
        verb: Action::StopWorkspace,
        target: &id,
        reason: &body.reason,
    };
    let empty = serde_json::json!({});
    Broker::of(&state)?.on_listed(&headers, call, empty).await
}

async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ReasonBody>,
) -> Result<Response, ApiError> {
    let call = Call {
        verb: Action::StartWorkspace,
        target: &id,
        reason: &body.reason,
    };
    let empty = serde_json::json!({});
    Broker::of(&state)?.on_listed(&headers, call, empty).await
}

async fn autostart(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<AutostartBody>,
) -> Result<Response, ApiError> {
    let call = Call {
        verb: Action::SetWorkspaceAutostart,
        target: &id,
        reason: &body.reason,
    };
    let enabled = serde_json::json!({ "enabled": body.enabled });
    Broker::of(&state)?.on_listed(&headers, call, enabled).await
}

async fn rename(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<RenameBody>,
) -> Result<Response, ApiError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("Give the new name."));
    }
    let call = Call {
        verb: Action::RenameWorkspace,
        target: &id,
        reason: &body.reason,
    };
    let named = serde_json::json!({ "name": name });
    Broker::of(&state)?.on_listed(&headers, call, named).await
}

async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ReasonBody>,
) -> Result<Response, ApiError> {
    let call = Call {
        verb: Action::DeleteWorkspace,
        target: &id,
        reason: &body.reason,
    };
    let empty = serde_json::json!({});
    Broker::of(&state)?.on_listed(&headers, call, empty).await
}

async fn restore(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ReasonBody>,
) -> Result<Response, ApiError> {
    let caller = calling_agent_thread(&headers)?;
    let broker = Broker::of(&state)?;
    let row = trash_rows(&broker.port)
        .await?
        .into_iter()
        .find(|row| row.id == id)
        .ok_or_else(|| ApiError::not_found(format!("No Trash entry `{id}` exists.")))?;
    refuse_unless_machine_made(Action::RestoreWorkspace, row.machine_made)?;
    if !row.restore_available {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "Trash entry `{id}` cannot be restored: it holds no database copy. Deleting \
                 a machine-made workspace keeps none."
            ),
        ));
    }
    let call = Call {
        verb: Action::RestoreWorkspace,
        target: &id,
        reason: &body.reason,
    };
    let path = format!("/broker/trash/{id}/restore");
    let body = serde_json::json!({ "caller": broker.caller(caller) });
    let outcome = broker.act(caller, call, &path, body).await?;
    respond(outcome, |_| Ok(StatusCode::NO_CONTENT.into_response()))
}

async fn list_trash() -> Result<Json<serde_json::Value>, ApiError> {
    let rows = trash_rows(&gateway()?).await?;
    Ok(Json(serde_json::json!({ "trash": rows })))
}

async fn restart() -> ApiError {
    ApiError::new(StatusCode::METHOD_NOT_ALLOWED, NO_RESTART)
}

/// The existing gateway control route each reused verb calls.
fn control_suffix(verb: Action) -> &'static str {
    match verb {
        Action::StopWorkspace => "stop",
        Action::StartWorkspace => "start",
        Action::SetWorkspaceAutostart => "autostart",
        _ => "rename",
    }
}

/// The single engine-side check of the verb split. The gateway's listing
/// computes `machine_made`, so the four origins are never restated here.
fn refuse_unless_machine_made(verb: Action, machine_made: bool) -> Result<(), ApiError> {
    if machine_made {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        format!(
            "An agent may not {} a user or unclassified workspace. Only a machine-made one \
             (origin agent, demo, eval or e2e) is open to it. The owner uses the picker.",
            verb.word()
        ),
    ))
}

/// Refuse a display name another workspace carries, matched as the gateway
/// matches it, so the owner never approves a create or rename it would refuse.
fn refuse_a_taken_name(
    listing: &[ListedWorkspace],
    name: &str,
    except_id: Option<&str>,
) -> Result<(), ApiError> {
    match listing.iter().find(|w| {
        Some(w.id.as_str()) != except_id
            && lucidos_gateway_constants::display_names_match(&w.name, name)
    }) {
        Some(taken) => Err(taken_conflict(taken)),
        None => Ok(()),
    }
}

fn taken_conflict(taken: &ListedWorkspace) -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        format!(
            "Workspace `{}` (\u{201c}{}\u{201d}) already exists. Pick another name or slug: \
             the broker never adds a suffix.",
            taken.id, taken.name
        ),
    )
}

/// Deadline for one mutating gateway call. A create provisions a database.
const BROKER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

fn refused(verb: Action, target: &str, refusal: ControlRefusal) -> ApiError {
    let (status, why) = match refusal {
        ControlRefusal::Refused { status, message } => (status, message),
        ControlRefusal::Unreachable(why) => (StatusCode::BAD_GATEWAY.as_u16(), why),
    };
    crate::log!(
        "[WorkspaceBroker] {} `{target}` failed ({status}): {why}",
        verb.act()
    );
    ApiError::with_code(
        status,
        format!("The gateway refused to {} `{target}`: {why}", verb.word()),
    )
}

fn respond(
    outcome: Outcome,
    done: impl FnOnce(String) -> Result<Response, ApiError>,
) -> Result<Response, ApiError> {
    match outcome {
        Outcome::Done(body) => done(body),
        Outcome::NeedsApproval(request_id) => Ok(approval_required(&request_id)),
    }
}

/// 403 `owner_approval_required`, with the id the agent asks the owner with.
fn approval_required(request_id: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": "This act needs the owner's Allow once. Ask with the request id, then \
                      call again.",
            "reason": OWNER_APPROVAL_REQUIRED,
            "request_id": request_id,
        })),
    )
        .into_response()
}

fn gateway() -> Result<String, ApiError> {
    super::base_path::gateway_port().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "This engine runs without a gateway, and the gateway keeps the workspaces.",
        )
    })
}

/// Why a classification may not run now: a named workspace is gone, or is
/// no longer `unknown`. `None` when every one still waits to be classified.
pub(crate) async fn unclassified_drift(workspaces: &[String]) -> Result<Option<String>, String> {
    let port = gateway().map_err(|e| e.message)?;
    let listing = list_from_gateway(&port, GATEWAY_DEADLINE)
        .await
        .map_err(|e| e.message)?;
    Ok(classification_drift(&listing, workspaces))
}

fn classification_drift(listing: &[ListedWorkspace], workspaces: &[String]) -> Option<String> {
    workspaces
        .iter()
        .find_map(|slug| match listing.iter().find(|w| &w.id == slug) {
            None => Some(format!("workspace `{slug}` no longer exists")),
            Some(w) if w.origin != "unknown" => {
                Some(format!("workspace `{slug}` is already classified"))
            }
            Some(_) => None,
        })
}

/// Where a classification stopped: how many workspaces moved first, and why.
pub(crate) struct ClassifyStop {
    pub(crate) moved: usize,
    pub(crate) reason: String,
}

/// Classify each workspace on the owner's confirmed batch. Stops at the first
/// refusal, and says how many moved before it.
pub(crate) async fn classify(
    workspaces: &[String],
    origin: crate::engine::permission::batch::ClassifiedOrigin,
) -> Result<(), ClassifyStop> {
    let port = gateway().map_err(|e| ClassifyStop {
        moved: 0,
        reason: e.message,
    })?;
    let body = serde_json::json!({ "origin": origin.wire() });
    for (moved, slug) in workspaces.iter().enumerate() {
        let path = format!("/broker/workspaces/{slug}/classify");
        let answer = tokio::time::timeout(GATEWAY_DEADLINE, post_control(&port, &path, &body))
            .await
            .unwrap_or_else(|_| {
                Err(ControlRefusal::Unreachable(
                    "the gateway did not answer in time".into(),
                ))
            });
        if let Err(refusal) = answer {
            let why = match refusal {
                ControlRefusal::Refused { message, .. } => message,
                ControlRefusal::Unreachable(why) => why,
            };
            return Err(ClassifyStop {
                moved,
                reason: format!(
                    "{moved} of {} classified, then `{slug}` was refused: {why}",
                    workspaces.len()
                ),
            });
        }
    }
    crate::log!(
        "[WorkspaceBroker] Classified {} workspaces as {}",
        workspaces.len(),
        origin.wire()
    );
    Ok(())
}

async fn trash_rows(port: &str) -> Result<Vec<TrashRow>, ApiError> {
    let unreadable = |why: String| {
        crate::log!("[WorkspaceBroker] {why}");
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            format!("Could not read the Trash: {why}"),
        )
    };
    let body = tokio::time::timeout(GATEWAY_DEADLINE, fetch_control(port, "/trash"))
        .await
        .map_err(|_| unreadable("the gateway did not answer in time".into()))?
        .map_err(unreadable)?;
    serde_json::from_str(&body).map_err(|e| {
        unreadable(format!(
            "the Trash listing is not one this engine reads ({e})"
        ))
    })
}

#[cfg(test)]
#[path = "workspace_broker_tests.rs"]
mod tests;
