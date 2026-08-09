//! LLM-facing schemas for the code/command execution tools
//! (run_python / run_bash family). Handlers live in `engine::tools::{python,bash}`.

use super::{BG_DEFAULT_TIMEOUT_SECS, BG_MAX_TIMEOUT_SECS, DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS};
use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn exec_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::RUN_PYTHON.to_string(),
            description: format!(
                "Create and process data files. Write to data/artifacts/ with open(); changes are staged and committed atomically, so apps see the old files until the commit. Print freely for debugging, since stdout is never captured as file content. Declare third-party packages in `packages` and they are auto-installed. \
                Hard {MAX_TIMEOUT_SECS}s ceiling: use run_python_background for slower work. \
                Credentials arrive as env vars: CRED_{{NAME}} for an api_key, bearer or basic credential, CRED_{{NAME}}_USERNAME and CRED_{{NAME}}_PASSWORD for a password one (which has no bare CRED_{{NAME}}), and OAUTH_{{PROVIDER}}_ACCESS_TOKEN plus OAUTH_{{PROVIDER}}_EMAIL for a connected account, auto-refreshed. The name is uppercased with hyphens, spaces and dots replaced by underscores."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python code. Write files with open('data/artifacts/name.csv', 'w')."
                    },
                    "packages": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Packages to install into the per-workspace venv first."
                    },
                    "commit_message": {
                        "type": "string",
                        "description": "Commit message for the files this script writes."
                    }
                },
                "required": ["code"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_PYTHON_BACKGROUND.to_string(),
            description: format!(
                "Spawn a long-running Python script in the background and return a task_id immediately. \
                Use it whenever a Python task needs scientific packages (numpy, pandas, scipy, matplotlib) AND may outrun run_python's {MAX_TIMEOUT_SECS}s sync ceiling: backtests, sweeps, model training, batch processing. \
                Same `packages` arg, venv and env-var injection as run_python. Drain with bash_output(task_id), cancel with bash_kill(task_id). Default timeout {BG_DEFAULT_TIMEOUT_SECS}s, max {BG_MAX_TIMEOUT_SECS}s, then the watchdog kills the child. \
                Unlike run_python, writes go straight to `data/` with no staging and no auto-commit, so apps see incremental progress; commit explicitly if you want git history."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python code. Imports run inside the per-workspace venv; declare third-party packages in `packages`."
                    },
                    "packages": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Packages to install into the venv before spawn. Same shared venv as run_python; an already-installed one is a no-op."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": format!("Seconds before the watchdog kills the child (default {BG_DEFAULT_TIMEOUT_SECS}, max {BG_MAX_TIMEOUT_SECS}).")
                    }
                },
                "required": ["code"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_BASH.to_string(),
            description: format!(
                "Run system commands (curl, wget, git, jq, system tools). NOT for creating or editing files in data/: use run_python, and git add plus git commit to commit what bash changed. \
                Stdout and stderr come back truncated to 100KB. Timeout {DEFAULT_TIMEOUT_SECS}s by default, {MAX_TIMEOUT_SECS}s max: bump `timeout_secs` to the max for a build, a full-repo grep, or anything over 30s, or the default kills it mid-stream. \
                The repeated-call guard buckets consecutive calls by the FIRST WHITESPACE TOKEN of `command`, so `sleep 60 && check` buckets under `sleep`. To WAIT on a background task use `bash_output(task_id, wait_secs=N)`. Same env-var injection as run_python."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command, run under `bash -o pipefail -c` so a failing stage is not masked by a later succeeding one. NOT for writing to data/: use run_python."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": format!("Timeout in seconds (default {DEFAULT_TIMEOUT_SECS}, max {MAX_TIMEOUT_SECS}).")
                    }
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_BASH_BACKGROUND.to_string(),
            description: format!(
                "Spawn a long-running shell command in the background and return a task_id immediately. \
                Use it whenever the command may exceed run_bash's {MAX_TIMEOUT_SECS}s sync ceiling: long HTTP polls, builds, scrapers, npm/cargo installs, repo-wide migrations. \
                Drain with bash_output(task_id), cancel with bash_kill(task_id). Never hand-roll a `time.sleep` polling loop in run_python. Default timeout {BG_DEFAULT_TIMEOUT_SECS}s, max {BG_MAX_TIMEOUT_SECS}s, then the child is killed. Same env-var injection as run_bash."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command, run under `bash -o pipefail -c` so a failing stage is not masked by a later succeeding one."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": format!("Seconds before the watchdog kills the child (default {BG_DEFAULT_TIMEOUT_SECS}, max {BG_MAX_TIMEOUT_SECS}).")
                    }
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: tn::BASH_OUTPUT.to_string(),
            description: format!(
                "Drain stdout and stderr from a background task, returning only what was emitted since the previous call, so call repeatedly to follow a stream. It finishes with the final tail and finished=true, even if your drain lands at the moment it completes. \
                STOP polling once you see finished=true. Nothing new can arrive, and a repeat eventually replays the FULL final output, wasting context. \
                SUCCESS TEST: exit_code == 0, nothing weaker; a null exit_code is NEVER success. Read and quote `status`, which renders exit code, signal and watchdog kill as one phrase. \
                Pass `wait_secs: N` (1 to {max}) to BLOCK server-side for the FULL N seconds unless the task finishes first, INSTEAD OF a `time.sleep(N)` poll in a fresh run_python, which spends two calls per wait and trips the repeated-call guard. New output does not end the wait early; a user message does. Default 0 is a non-blocking drain. \
                load_knowhow('system-knowhow/running-python') for the status table, what pipefail and SIGPIPE do to exit_code, and how an oversized window is truncated.",
                max = crate::engine::tools::bash_background::BASH_OUTPUT_MAX_WAIT_SECS
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "task_id returned by run_bash_background or run_python_background."
                    },
                    "wait_secs": {
                        "type": "integer",
                        "description": format!(
                            "Omit or pass 0 for a non-blocking drain; 1..={max} blocks for the FULL duration unless the task finishes first, and higher is clamped silently. Use {max} for a long build you are following.",
                            max = crate::engine::tools::bash_background::BASH_OUTPUT_MAX_WAIT_SECS
                        )
                    }
                },
                "required": ["task_id"]
            }),
        },
        ToolDefinition {
            name: tn::BASH_KILL.to_string(),
            description: "Cancel a running background task from run_bash_background or run_python_background. \
                No-op if it already finished."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "task_id from run_bash_background or run_python_background."
                    }
                },
                "required": ["task_id"]
            }),
        },
    ]
}
