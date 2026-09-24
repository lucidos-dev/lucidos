//! `lucidos background-task run | output | stop`: a coding agent's *background
//! tasks*, work the engine runs so it can outlive the agent's turn.
//!
//! `run` hands the command to the engine, which runs it in this thread's
//! worktree and arms an event wait on its completion. The agent then ends its
//! turn, and the wait re-opens the thread with the result. Over
//! `/api/v1/threads/<id>/background-tasks`.
//!
//! `$LUCIDOS_THREAD_ID` names the thread and there is no `--thread` flag, so a
//! session cannot start, read or stop another thread's tasks.

use serde_json::json;

use crate::http::{client as http_client, send_and_print};
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

pub(crate) fn cmd_run(
    ws: &Workspace,
    command: &[String],
    timeout_secs: Option<u64>,
) -> Result<(), BoxError> {
    if command.is_empty() {
        return Err("Pass the command after `--`: \
                    lucidos background-task run -- cargo test"
            .into());
    }
    let thread_id = calling_thread()?;
    let url = tasks_url(ws, &thread_id);
    let body = json!({
        "command": shell_command_line(command),
        "timeout_secs": timeout_secs,
    });
    send_and_print("POST", &url, http_client()?.post(&url).json(&body))
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
