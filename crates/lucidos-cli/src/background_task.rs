//! `lucidos background-task run | wait | output | stop`: a coding agent's
//! *background tasks*, work the engine runs so it can outlive one tool call.
//!
//! `run` hands the command to the engine, which runs it in this thread's
//! worktree and arms an event wait on its completion. The agent then ends its
//! turn, and the wait re-opens the thread with the result. `run --poll` arms
//! nothing, and the agent calls `wait` until the task finishes. Over
//! `/api/v1/threads/<id>/background-tasks`.
//!
//! `$LUCIDOS_THREAD_ID` names the thread and there is no `--thread` flag, so a
//! session cannot start, read or stop another thread's tasks.

use serde_json::json;

use crate::http::{client as http_client, client_with_timeout, send_and_print};
use crate::workspace::{BoxError, Workspace};

fn calling_thread() -> Result<String, BoxError> {
    std::env::var("LUCIDOS_THREAD_ID").map_err(|_| {
        "LUCIDOS_THREAD_ID is not set, so there is no thread to run a background task \
         for. This subcommand only works from inside a Lucidos coding-agent session."
            .into()
    })
}

fn tasks_url(ws: &Workspace, thread_id: &str) -> String {
    format!(
        "{}/api/v1/threads/{}/background-tasks",
        ws.base_url(),
        thread_id
    )
}

/// The command line the engine's shell runs.
///
/// One argument passes through verbatim, so a quoted pipeline or redirect
/// (`-- 'cargo test > log 2>&1'`) keeps its meaning. Several arguments are
/// quoted one by one, so `-- cargo test --lib "my filter"` runs exactly those
/// words and the shell expands nothing.
pub(crate) fn shell_command_line(args: &[String]) -> String {
    match args {
        [single] => single.clone(),
        many => many
            .iter()
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn shell_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./=:,+@%".contains(c));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// How long `wait` lets the engine answer. The engine caps one wait at 230 s,
/// so this only has to outlast that and the round trip.
const WAIT_RESPONSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(290);

/// What `run` asks for, beside the command.
pub(crate) struct RunOptions<'a> {
    pub description: Option<&'a str>,
    pub timeout_secs: Option<u64>,
    /// Arm no event wait: the agent polls with `wait`.
    pub poll: bool,
}

pub(crate) fn cmd_run(
    ws: &Workspace,
    command: &[String],
    options: RunOptions<'_>,
) -> Result<(), BoxError> {
    if command.is_empty() {
        return Err("Pass the command after `--`: \
                    lucidos background-task run --description \"the test suite\" -- cargo test"
            .into());
    }
    let thread_id = calling_thread()?;
    let url = tasks_url(ws, &thread_id);
    let body = run_body(command, &options);
    send_and_print("POST", &url, http_client()?.post(&url).json(&body))
}

fn run_body(command: &[String], options: &RunOptions<'_>) -> serde_json::Value {
    json!({
        "command": shell_command_line(command),
        "description": options.description,
        "timeout_secs": options.timeout_secs,
        "poll": options.poll,
    })
}

pub(crate) fn cmd_wait(
    ws: &Workspace,
    task_id: &str,
    max_secs: Option<u64>,
) -> Result<(), BoxError> {
    let thread_id = calling_thread()?;
    let url = format!("{}/{}/wait", tasks_url(ws, &thread_id), task_id);
    let mut request = client_with_timeout(WAIT_RESPONSE_DEADLINE)?.get(&url);
    if let Some(secs) = max_secs {
        request = request.query(&[("max_secs", secs)]);
    }
    send_and_print("GET", &url, request)
}

pub(crate) fn cmd_output(ws: &Workspace, task_id: &str) -> Result<(), BoxError> {
    let thread_id = calling_thread()?;
    let url = format!("{}/{}", tasks_url(ws, &thread_id), task_id);
    send_and_print("GET", &url, http_client()?.get(&url))
}

pub(crate) fn cmd_stop(ws: &Workspace, task_id: &str) -> Result<(), BoxError> {
    let thread_id = calling_thread()?;
    let url = format!("{}/{}/stop", tasks_url(ws, &thread_id), task_id);
    send_and_print("POST", &url, http_client()?.post(&url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn options(description: Option<&str>, poll: bool) -> RunOptions<'_> {
        RunOptions {
            description,
            timeout_secs: None,
            poll,
        }
    }

    /// The engine names the task on the thread's waiting row by this field.
    #[test]
    fn the_description_reaches_the_request_body() {
        let body = run_body(&args(&["make"]), &options(Some("the release build"), false));
        assert_eq!(body["description"], "the release build");
        assert!(run_body(&args(&["make"]), &options(None, false))["description"].is_null());
    }

    /// `--poll` is what keeps the engine from arming a wake-up the agent
    /// would read after it already polled the result.
    #[test]
    fn poll_reaches_the_request_body() {
        assert_eq!(
            run_body(&args(&["make"]), &options(None, true))["poll"],
            true
        );
        assert_eq!(
            run_body(&args(&["make"]), &options(None, false))["poll"],
            false
        );
    }

    #[test]
    fn a_single_argument_passes_through_so_a_quoted_pipeline_keeps_its_meaning() {
        let line = "./scripts/e2e.sh > .lucidos/e2e.log 2>&1";
        assert_eq!(shell_command_line(&args(&[line])), line);
    }

    #[test]
    fn several_arguments_are_quoted_so_the_shell_runs_exactly_those_words() {
        assert_eq!(
            shell_command_line(&args(&["cargo", "test", "--lib", "my filter"])),
            "cargo test --lib 'my filter'"
        );
        assert_eq!(
            shell_command_line(&args(&["echo", "it's", "$HOME"])),
            r"echo 'it'\''s' '$HOME'"
        );
        assert_eq!(shell_command_line(&args(&["printf", ""])), "printf ''");
    }
}
