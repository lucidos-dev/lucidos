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
                        "description": "Shell command to execute (passed to `bash -o pipefail -c`, so a failing stage of a pipeline is not masked by a later succeeding one). NOT for writing to data/ — use run_python."
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
                        "description": "Shell command to execute (passed to `bash -o pipefail -c`, so a failing stage of a pipeline is not masked by a later succeeding one)."
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
                SUCCESS TEST: exit_code == 0, nothing weaker. exit_code carries the NORMAL exit status and only that; it is null while the task is running, when the child died on a signal (see `signal`), and when the engine could not obtain a status at all. A null exit_code is NEVER success. \
                `signal` is the Unix signal that killed the SHELL the engine spawned, when one did — 9 SIGKILL (also what a watchdog timeout and bash_kill use, alongside timed_out / killed), 11 SIGSEGV. It is null for a normal exit AND when a signal killed a stage inside your pipeline: that arrives as an exit_code of 128+signum (e.g. 141 = SIGPIPE). \
                `status` is the rendered one-line phrase (\"exit code 101\", \"killed by SIGKILL (signal 9)\", \"exit code 141 (probable SIGPIPE)\", \"exit code unknown\") and is the same phrase the completion summary uses — read it if you want one field instead of three. \
                Commands run under `bash -o pipefail`, so a failing stage is NEVER masked by a later succeeding one: `cargo clippy … | tee build.log` reports clippy's 101, not tee's 0. You do NOT need a sidecar file to detect a failure inside a pipeline. Precisely: the status is that of the RIGHTMOST failing stage (`sh -c 'exit 42' | sh -c 'exit 7'` reports 7), so split a pipeline whose stages can each fail if you need to attribute which one did. (Consequence: a producer SIGPIPE'd by an early-closing consumer, e.g. `yes | head -1`, reports exit_code 141 rather than 0.) \
                Pass `wait_secs: N` (1–{max}) to BLOCK server-side for the FULL N seconds unless the task finishes first — use this INSTEAD OF hand-rolling `time.sleep(N)` polling in a fresh run_python (which wastes two tool calls per wait, doubles context, and stalls the turn). New output does NOT end the wait early: one call gives you the whole N-second window at once, so following a 40-minute build is ~20 calls at wait_secs=120, not hundreds. A user message DOES end it early, so their follow-up isn't stuck behind your block. Default 0 = non-blocking drain. \
                `elapsed_secs` (how long the task has been running, or its total runtime once finished) and `waited_secs` (how long THIS call actually blocked) are the ONLY trustworthy clock you have — read them instead of estimating elapsed time from how long you asked to wait, or you will tell the user \"about 20 minutes in\" 90 seconds into a build. `elapsed_secs` may be null for a task that finished long ago. \
                Oversized windows keep the TAIL (newest output) with a leading `[truncated — N earlier bytes dropped]` marker, so the failure at the end of a build log is never the part that gets dropped.",
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
                            "How long to block, in seconds. Omit or pass 0 for a non-blocking drain. Pass 1..={max} to BLOCK for the FULL duration unless the task finishes first (max {max} — values above are clamped down silently). New output does NOT cut the wait short. Typical: {max} for a long build or backtest you're following, 30–60 for something you expect to finish soon, 0 for a quick liveness check. Non-integer values (strings, floats) are rejected with an error.",
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
