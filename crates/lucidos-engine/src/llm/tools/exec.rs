//! LLM-facing schemas for the code/command execution tools
//! (run_python / run_bash family). Handlers live in `engine::tools::{python,bash}`.


use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;
use super::{BG_DEFAULT_TIMEOUT_SECS, BG_MAX_TIMEOUT_SECS, DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS};

pub(super) fn exec_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::RUN_PYTHON.to_string(),
            description: "Create and process data files. Write to data/artifacts/ with open(). \
                Changes are staged and committed atomically — apps see old files until commit. \
                Print freely for debugging (stdout is never captured as file content). \
                Packages are auto-installed via the packages parameter. \
                Environment variables injected automatically: CRED_{NAME} for api_key/bearer/basic credentials, \
                CRED_{NAME}_USERNAME and CRED_{NAME}_PASSWORD for password credentials, \
                OAUTH_{PROVIDER}_ACCESS_TOKEN and OAUTH_{PROVIDER}_EMAIL for connected OAuth accounts \
                (tokens are auto-refreshed). Provider/name is uppercased with hyphens/spaces/dots replaced by underscores.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python code. Write files with open('data/artifacts/name.csv', 'w'). Print freely for debugging."
                    },
                    "packages": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Python packages to install before running (e.g., [\"pandas\", \"requests\"]). Installed into isolated venv."
                    },
                    "commit_message": {
                        "type": "string",
                        "description": "Git commit message for the files this script creates/updates (e.g., \"Import Oura sleep data\")."
                    }
                },
                "required": ["code"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_PYTHON_BACKGROUND.to_string(),
            description: format!(
                "Spawn a long-running Python script in the background and return a task_id immediately. \
                Use whenever a Python task needs scientific packages (numpy, pandas, scipy, scikit-learn, matplotlib, statsmodels, …) AND may run longer than run_python's {MAX_TIMEOUT_SECS}s sync ceiling — backtests, data sweeps, model training, batch processing, large file conversions. \
                Packages are auto-installed into the per-workspace venv before spawn (same `packages` arg and same venv as run_python). \
                The script runs from the workspace root with the same env-var injection as run_python (CRED_*, OAUTH_*, LUCIDOS_WORKSPACE). \
                Drain output incrementally with bash_output(task_id); cancel with bash_kill(task_id) — the same background-task tools used by run_bash_background, so the LLM-facing drain/cancel surface is identical. \
                Default timeout {BG_DEFAULT_TIMEOUT_SECS}s, max {BG_MAX_TIMEOUT_SECS}s — the watchdog kills the child when the timeout fires. \
                Unlike run_python, writes go directly to `data/` (no staging, no auto-commit) so apps can see incremental progress — commit explicitly via a follow-up `run_bash_background \"git add … && git commit …\"` if you want git history."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python code. Imports run inside the per-workspace venv; declare third-party packages in `packages`. Writes go directly to data/ (no staging) — apps can see incremental progress."
                    },
                    "packages": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Python packages to install into the venv before spawn (e.g., [\"numpy\", \"pandas\", \"scipy\"]). Same shared venv as run_python; already-installed packages are no-ops."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": format!("Wall-clock seconds before the watchdog kills the child (default: {BG_DEFAULT_TIMEOUT_SECS}, max: {BG_MAX_TIMEOUT_SECS}).")
                    }
                },
                "required": ["code"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_BASH.to_string(),
            description: format!(
                "Run system commands (curl, wget, git, jq, system tools). \
                Do NOT use for creating or editing files in data/ — use run_python for that. \
                If you need to commit changes made by bash, use git add + git commit with a descriptive message. \
                Stdout and stderr returned (truncated to 100KB). Timeout: {DEFAULT_TIMEOUT_SECS}s default, {MAX_TIMEOUT_SECS}s max. \
                Bump `timeout_secs` to {MAX_TIMEOUT_SECS} for cargo/npm builds, full-repo greps, large `git log`/`git blame`, \
                or any command you expect to run >30s — the {DEFAULT_TIMEOUT_SECS}s default will kill them mid-stream and waste a turn retrying. \
                Repeated-call guard: consecutive run_bash calls are bucketed by the FIRST WHITESPACE TOKEN of `command` \
                (so `sleep 60 && check` buckets under `sleep`); at 3 same-bucket calls the engine replaces the call's result with a STOP message (the call itself never runs), at 5 it force-ends your turn. \
                For periodic checks vary the first token (tail / wc / head / awk) or restructure as ONE command. \
                To WAIT on a background task spawned via run_bash_background / run_python_background, use `bash_output(task_id, wait_secs=N)` — never `run_python` containing `time.sleep`, which now ALSO buckets and trips its own 3-strike guard on verbatim retries. \
                Environment variables injected automatically: CRED_{{NAME}} for api_key/bearer/basic credentials, \
                CRED_{{NAME}}_USERNAME and CRED_{{NAME}}_PASSWORD for password credentials, \
                OAUTH_{{PROVIDER}}_ACCESS_TOKEN and OAUTH_{{PROVIDER}}_EMAIL for connected OAuth accounts \
                (tokens are auto-refreshed). Provider/name is uppercased with hyphens/spaces/dots replaced by underscores."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute (passed to /bin/sh -c). NOT for writing to data/ — use run_python."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": format!("Timeout in seconds (default: {DEFAULT_TIMEOUT_SECS}, max: {MAX_TIMEOUT_SECS})")
                    }
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_BASH_BACKGROUND.to_string(),
            description: format!(
                "Spawn a long-running shell command in the background and return a task_id immediately. \
                Use whenever the command may exceed run_bash's {MAX_TIMEOUT_SECS}s sync ceiling — long HTTP polls, builds, scrapers, npm/cargo installs, repo-wide migrations. \
                Drain output incrementally with bash_output(task_id); cancel with bash_kill(task_id). \
                Default timeout {BG_DEFAULT_TIMEOUT_SECS}s, max {BG_MAX_TIMEOUT_SECS}s — the child is killed when the timeout fires. \
                NEVER hand-roll `for i in range(...): time.sleep(...)` polling loops in run_python — use this trio instead. \
                Same env-var injection as run_bash (CRED_*, OAUTH_*)."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute (passed to /bin/sh -c)."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": format!("Wall-clock seconds before the watchdog kills the child (default: {BG_DEFAULT_TIMEOUT_SECS}, max: {BG_MAX_TIMEOUT_SECS}).")
                    }
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: tn::BASH_OUTPUT.to_string(),
            description: format!(
                "Fetch incremental stdout/stderr from a background task created by run_bash_background OR run_python_background. \
                Returns only output emitted since the previous bash_output call (drain semantics) — call repeatedly to follow a stream. \
                When the task finishes, returns the final tail with finished=true: STOP polling at this point — subsequent calls fall back to the event store and return the FULL final stdout/stderr again, which wastes context. \
                exit_code is null while the task is running, after a watchdog timeout (timed_out=true), and after bash_kill (killed=true). \
                Pass `wait_secs: N` (1–{max}) to BLOCK server-side until new output arrives OR the task finishes OR N seconds pass — use this INSTEAD OF hand-rolling `time.sleep(N)` polling in a fresh run_python (which wastes two tool calls per wait, doubles context, and stalls the turn). With `wait_secs` set you typically need 0–2 calls per task (one optional initial drain, one with `wait_secs` that returns when finished=true). Default 0 = legacy non-blocking drain.",
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
                            "Server-side block ceiling, in seconds. Omit or pass 0 for the legacy non-blocking drain. Pass 1..={max} to BLOCK until new output arrives OR the task finishes OR N seconds pass (max {max} — values above are clamped down silently). Typical: 30–60s for long-running backtests / builds, 0 for a quick liveness check. Non-integer values (strings, floats) are rejected with an error.",
                            max = crate::engine::tools::bash_background::BASH_OUTPUT_MAX_WAIT_SECS
                        )
                    }
                },
                "required": ["task_id"]
            }),
        },
        ToolDefinition {
            name: tn::BASH_KILL.to_string(),
            description: "Cancel a running background bash task spawned via run_bash_background. \
                No-op if the task already finished. Use when the user asks to stop a long-running job or when the task has already produced enough output to act on."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "task_id returned by run_bash_background."
                    }
                },
                "required": ["task_id"]
            }),
        },
    ]
}
