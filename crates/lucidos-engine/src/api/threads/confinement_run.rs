//! `POST /api/v1/threads/:thread_id/confinement-tests`, a coding agent's
//! *confinement run* (`lucidos confinement-tests run`).
//!
//! The engine owns the command and the permission check; see
//! `engine::agent_session::confinement_run`.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;

use super::background_tasks::require_own_thread_agent;
use super::parse_thread_id;
use crate::api::AppState;
use crate::engine::agent_session::confinement_run::{Asking, ConfinementRun};

/// No command field: the engine runs only its own script.
#[derive(Deserialize)]
pub(in crate::api) struct RunRequest {
    /// The card an earlier call answered with, while the caller waits on it.
    card_id: Option<String>,
    /// `false` raises no card when no permission covers the run.
    #[serde(default = "ask_owner_by_default")]
    ask_owner: bool,
}

fn ask_owner_by_default() -> bool {
    true
}

impl RunRequest {
    fn asking(&self) -> Asking<'_> {
        match (&self.card_id, self.ask_owner) {
            (Some(card_id), _) => Asking::Waiting { card_id },
            (None, true) => Asking::Owner,
            (None, false) => Asking::Nobody,
        }
    }
}

/// POST /api/v1/threads/:thread_id/confinement-tests
pub(in crate::api) async fn run_confinement_tests(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<RunRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_id = parse_thread_id(&thread_id)?;
    require_own_thread_agent(&headers, thread_id)?;
    let run = state
        .engine
        .run_confinement_tests_for_agent(thread_id, req.asking())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    run_response(run).map(Json)
}

/// What the CLI reads. `state` decides its next move, and `message` is for the
/// agent reading the output. A refusal is a 409 carrying its reason.
fn run_response(run: ConfinementRun) -> Result<serde_json::Value, (StatusCode, String)> {
    Ok(match run {
        ConfinementRun::Refused { reason } => return Err((StatusCode::CONFLICT, reason)),
        ConfinementRun::Started {
            task_id,
            timeout_secs,
        } => serde_json::json!({
            "state": "started",
            "task_id": task_id,
            "timeout_secs": timeout_secs,
            "message": format!(
                "Started the confinement tests outside the sandbox as background task \
                 {task_id}. It is killed if still running after {timeout_secs}s."
            ),
        }),
        ConfinementRun::Asked { card_id } => serde_json::json!({
            "state": "asked",
            "card_id": card_id,
            "message": "The owner has a card on this thread to allow the run. Nothing runs \
                        until they answer it.",
        }),
        ConfinementRun::Closed { card_id } => serde_json::json!({
            "state": "closed",
            "card_id": card_id,
            "message": "The card closed without a grant, so the confinement tests did not \
                        run.",
        }),
        ConfinementRun::Unpermitted => serde_json::json!({
            "state": "unpermitted",
            "message": "No permission covers a confinement run, and no card was raised. Run \
                        `lucidos confinement-tests run` in the foreground: it asks the owner \
                        and holds the card open until they answer.",
        }),
    })
}

#[cfg(test)]
#[path = "confinement_run_tests.rs"]
mod tests;
