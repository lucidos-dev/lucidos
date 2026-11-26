use crate::engine::agent_session::background_task::BackgroundTaskExt;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::parse_thread_id;
use crate::api::AppState;
use crate::engine::agent_session::background_task::BackgroundTaskStart;
use crate::engine::bash_background::CompletionDelivery;
use crate::engine::child_report_channel::ChildReport;

type HandlerResult = Result<Json<serde_json::Value>, (StatusCode, String)>;

pub(super) fn require_own_thread_agent(
    headers: &HeaderMap,
    thread_id: Uuid,
) -> Result<(), (StatusCode, String)> {
    use crate::core::subprocess_origin::SubprocessOrigin;
    match crate::core::subprocess_origin::subprocess_origin(headers) {
        SubprocessOrigin::Subprocess {
            source_thread_id: Some(caller),
            ..
        } if caller == thread_id => Ok(()),
        SubprocessOrigin::Subprocess { .. } => Err((
            StatusCode::FORBIDDEN,
            "A thread's background tasks are its own, and the id in the path is not the \
             calling thread. Drop the id: `lucidos background-task` acts on the thread you \
             are running in."
                .to_string(),
        )),
        SubprocessOrigin::NotSubprocess => Err((
            StatusCode::FORBIDDEN,
            "Background tasks are started by a coding agent inside its own thread, through \
             `lucidos background-task`."
                .to_string(),
        )),
    }
}

#[derive(Deserialize)]
pub(in crate::api) struct StartRequest {
    command: String,
    description: Option<String>,
    timeout_secs: Option<u64>,
    #[serde(default)]
    poll: bool,
}

pub(in crate::api) async fn start_background_task(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<StartRequest>,
) -> HandlerResult {
    let thread_id = parse_thread_id(&thread_id)?;
    require_own_thread_agent(&headers, thread_id)?;
    let started = state
        .engine
        .start_background_task_for_agent(
            thread_id,
            &req.command,
            req.description.as_deref(),
            req.timeout_secs,
            if req.poll {
                CompletionDelivery::Poll
            } else {
                CompletionDelivery::EventWait
            },
        )
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok(Json(start_response(started)))
}

fn start_response(started: BackgroundTaskStart) -> serde_json::Value {
    match started {
        BackgroundTaskStart::Watched {
            task_id,
            timeout_secs,
        } => serde_json::json!({
            "status": "watched",
            "task_id": task_id,
            "timeout_secs": timeout_secs,
            "message": format!(
                "Started background task {task_id} in this thread's worktree. It is killed \
                 if still running after {timeout_secs}s. An event wait re-opens this thread \
                 when it finishes, carrying the exit status and the tail of its output. \
                 Nothing is blocking: end your turn now. Read the output any time with \
                 `lucidos background-task output {task_id}`."
            ),
        }),
        BackgroundTaskStart::Unwatched {
            task_id,
            timeout_secs,
        } => serde_json::json!({
            "status": "unwatched",
            "task_id": task_id,
            "timeout_secs": timeout_secs,
            "message": format!(
                "Started background task {task_id}, but no event wait could be armed: this \
                 thread has hit a subscription limit. Nothing will re-open the thread when \
                 it finishes, so do not end your turn expecting a wake. Stop it with \
                 `lucidos background-task stop {task_id}` and run the command in the \
                 foreground instead."
            ),
        }),
        BackgroundTaskStart::Polled {
            task_id,
            timeout_secs,
        } => serde_json::json!({
            "status": "polled",
            "task_id": task_id,
            "timeout_secs": timeout_secs,
            "message": format!(
                "Started background task {task_id} in this thread's worktree. It is killed \
                 if still running after {timeout_secs}s. Keep your turn: run `lucidos \
                 background-task wait {task_id}` in the foreground, and re-issue it until it \
                 reports finished. If your turn ends first, the engine re-opens this thread \
                 when it finishes."
            ),
        }),
        BackgroundTaskStart::Finished { task_id, output } => serde_json::json!({
            "status": "finished",
            "task_id": task_id,
            "message": format!("Background task {task_id} finished before this returned."),
            "output": output_json(&output),
        }),
    }
}

fn output_json(output: &str) -> serde_json::Value {
    serde_json::from_str(output).unwrap_or_else(|_| serde_json::Value::String(output.to_string()))
}

pub(in crate::api) async fn background_task_output(
    State(state): State<AppState>,
    Path((thread_id, task_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> HandlerResult {
    let thread_id = parse_thread_id(&thread_id)?;
    require_own_thread_agent(&headers, thread_id)?;
    let output = state
        .engine
        .background_task_output_for_agent(thread_id, &task_id)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    Ok(Json(output_json(&output)))
}

#[derive(Deserialize)]
pub(in crate::api) struct WaitQuery {
    max_secs: Option<u64>,
}

pub(in crate::api) async fn wait_for_background_task(
    State(state): State<AppState>,
    Path((thread_id, task_id)): Path<(String, String)>,
    Query(query): Query<WaitQuery>,
    headers: HeaderMap,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let thread_id = parse_thread_id(&thread_id)?;
    require_own_thread_agent(&headers, thread_id)?;
    let answer = state
        .engine
        .background_task_wait_for_agent(
            thread_id,
            &task_id,
            query.max_secs,
            crate::api::actor::user_actor(&headers, None),
        )
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    let mut response = Json(wait_response(
        &task_id,
        output_json(&answer.output),
        &answer.child_reports,
    ))
    .into_response();
    response
        .extensions_mut()
        .insert(super::child_reports::ChildReportsCarried);
    Ok(response)
}

fn wait_response(
    task_id: &str,
    mut output: serde_json::Value,
    child_reports: &[ChildReport],
) -> serde_json::Value {
    let running = output["finished"] == serde_json::Value::Bool(false);
    let message = match (running, child_reports.is_empty()) {
        (true, true) => format!(
            "Still running. Re-issue `lucidos background-task wait {task_id}` in the \
             foreground."
        ),
        (true, false) => format!(
            "A child thread's report arrived, so this wait returned early: read it under \
             `child_reports`. The task is still running. Re-issue `lucidos background-task \
             wait {task_id}` in the foreground."
        ),
        (false, false) => {
            "A child thread's report arrived: read it under `child_reports`.".to_string()
        }
        (false, true) => return output,
    };
    output["message"] = serde_json::Value::String(message);
    if !child_reports.is_empty() {
        output["child_reports"] = child_reports_json(child_reports);
    }
    output
}

pub(super) fn child_reports_json(child_reports: &[ChildReport]) -> serde_json::Value {
    child_reports
        .iter()
        .map(|report| {
            serde_json::json!({
                "card_id": report.card_id,
                "report": report.text,
            })
        })
        .collect()
}

pub(in crate::api) async fn stop_background_task(
    State(state): State<AppState>,
    Path((thread_id, task_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> HandlerResult {
    let thread_id = parse_thread_id(&thread_id)?;
    require_own_thread_agent(&headers, thread_id)?;
    let message = state
        .engine
        .stop_background_task_for_agent(thread_id, &task_id)
        .await
        .map_err(|e| (StatusCode::CONFLICT, e))?;
    Ok(Json(
        serde_json::json!({ "status": "stopped", "message": message }),
    ))
}

#[cfg(test)]
#[path = "background_tasks_tests.rs"]
mod tests;
