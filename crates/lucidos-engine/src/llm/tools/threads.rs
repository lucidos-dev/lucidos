//! LLM-facing schemas for the thread-spawning tools (run_thread,
//! run_coding_agent). Thread INTROSPECTION (list_threads / count_threads) is the
//! grouped `threads` manifest tool (built from `crate::capability_manifest`); the
//! flat names stay wired as back-compat aliases in `execute_tool`.

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn spawn_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::RUN_THREAD.to_string(),
            description: "Start a new Lucidos thread to handle a subtask. Default behavior (relation=\"child\") is a child thread: it runs its own agentic loop with full tool access, and when it completes a callback automatically resumes THIS thread with the child thread's result (including its final response text) — you do NOT need to poll. You can spawn multiple child threads in parallel; each reports back independently. When a child thread reports back, review its result — if it's incomplete, spawn another run_thread with a refined prompt. Pass relation=\"top\" instead when the spawn is for the user to read later (research/report) and you do NOT need the result yourself; the spawned thread runs as an independent top-level thread and never reports back. Use for non-code tasks (research, analysis, drafting). For code tasks, use run_coding_agent instead.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The task for the new thread"
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional short descriptive title (3-6 words) for the thread. When provided, the system will not auto-generate a title. Recommended so the thread list is meaningful at a glance."
                    },
                    "relation": {
                        "type": "string",
                        "enum": ["child", "top"],
                        "description": "How the spawned thread relates to this one. 'child' (default): when the spawned thread finishes, this thread automatically resumes with its result — use for delegated subtasks whose answer you need. 'top': fire-and-forget — the spawned thread runs independently as a top-level thread; this thread does not resume when it finishes. Use when the spawn is for the user to look at later, not for you. ('sub' is accepted as a back-compat alias for 'child'.)"
                    }
                },
                "required": ["prompt"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_CODING_AGENT.to_string(),
            description: "Run a coding agent to edit source code. Default backend is Claude Code; set `coding_agent=\"codex\"` whenever the user asks for Codex or OpenAI Codex. ONLY use this when the user explicitly asks to modify code (Lucidos, an installed app, or an external repo). Never use this for workspace tasks like web scraping, file manipulation, data processing, or anything the native tools can handle — use browser_open, web_search, http_request, run_python, read_file, write_file, etc. instead.\n\nDefault behavior (relation=\"child\") is a child thread: spawning returns immediately; when the coding-agent session ends, a callback automatically resumes THIS thread with the child thread's final response text — you do NOT need to poll. For PARALLEL work, issue multiple run_coding_agent calls in one response and they spawn concurrently, each reporting back independently. For SEQUENTIAL pipelines (where step N depends on step N-1's outcome — e.g., build → harden → e2e, stopping on first failure), spawn ONE run_coding_agent and end the turn; the next step runs only after the callback resumes you. Never batch sequential spawns in one response — that defeats the dependency. Always inspect the child thread's final response text to determine pass/fail before acting on it or emitting milestones — the spawn ack is not a result.\n\nPass relation=\"top\" instead when the user asks for a piece of work to happen in its own thread that they will follow themselves (e.g. 'do this in a separate thread' / 'spawn a Codex session for this and I'll check in later'). The spawned coding-agent session runs as an independent top-level thread and will NOT report back to this conversation.\n\nCROSS-WORKSPACE: When the user asks for the work to happen in a DIFFERENT workspace ('do this in dev', 'fix it in myws'), set the `workspace` parameter to the target workspace's basename (e.g. `workspace=\"dev\"`). The tool will POST to that workspace's engine and the coding-agent session will land there — same UX as a local spawn. Cross-workspace requires `relation=\"top\"` (child-thread auto-resume callbacks across workspaces are unsupported); the tool refuses child + cross-workspace with an error. The `folder` parameter resolves on the target workspace's filesystem; for cross-workspace spawns make sure the target workspace has the app installed or the repo registered.\n\nBEFORE CALLING: identify what the work targets and pick the right `folder`.\n- Editing Lucidos itself (engine/UI under the Lucidos source tree) → omit `folder`.\n- Editing an installed app's UI/knowhow/intents → pass `folder=\"data/apps/<id>\"` (workspace-relative; the engine resolves against the target workspace's root). The session runs as an *app coding-agent thread*: sparse-checkout worktree narrowed to that app folder, Apply ff-merges to the workspace git's main, no engine restart, no `/harden`. For tiny edits that don't need a full agent worktree, use the chat path (file tools + the `lucidos` CLI) instead.\n- Editing a registered external repo → pass `folder=<repo name>` (resolves via the repo registry, same as the deprecated `repo` param).\n- Editing an unregistered git folder or a non-git directory → not supported in v1; refused with a clear error. Register the repo with `manage_repositories action='add'` first.\n- Editing a `data/` subtree other than `data/apps/<id>/` (knowhow, triggers, artifacts, config, scripts) → refused. Use the chat path: file tools + the `lucidos` CLI.\n- Ambiguous → ask one question: 'which folder should this run in?' before calling.\n\nIf Rust backend files are changed, Lucidos shows the user a toast suggesting a restart — the user must manually trigger the rebuild and restart for backend changes to take effect (do NOT promise that changes will be live shortly). Frontend (TypeScript/CSS) changes are picked up automatically. App `data/apps/<id>/` changes never restart the engine.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The coding task to perform. Be specific about what files to modify and what the desired outcome is."
                    },
                    "coding_agent": {
                        "type": "string",
                        "enum": ["claude-code", "codex"],
                        "description": "Coding-agent backend for the new thread. Use \"codex\" whenever the user asks for Codex or OpenAI Codex. Omit or set \"claude-code\" for Claude Code."
                    },
                    "folder": {
                        "type": "string",
                        "description": "What the coding-agent thread should edit. Accepts: (a) an absolute path to an app folder like `/Users/.../workspaces/myws/data/apps/habit-tracker`, (b) a workspace-relative path like `data/apps/habit-tracker` (resolved against the target workspace's root), or (c) a registered external repository name or UUID from `manage_repositories` (resolved to the repo's path). Omit to edit Lucidos source. The engine selects the spawn kind automatically: Lucidos source ⇒ Lucidos-internal coding-agent thread (full /harden, Apply may restart the engine); `data/apps/<id>/` ⇒ app coding-agent thread (sparse-checkout worktree, Apply ff-merges to workspace git main, no engine restart, no /harden); registered external repo ⇒ external-repo coding-agent thread (no Apply gate). REFUSED: any `data/` path outside `data/apps/<id>/` (use chat tools + `lucidos` CLI for non-app data), subpaths inside an app (`data/apps/<id>/ui/`), file paths, the whole `data/`, unregistered git folders, non-git folders, `<workspace>/.lucidos/`, system paths."
                    },
                    // Temporary measure — registered in docs/temporary-measures.md
                    // § "`repo` → `folder` deprecated alias on `run_coding_agent`".
                    "repo": {
                        "type": "string",
                        "description": "DEPRECATED — use `folder` instead. Accepted for one release as an alias: equivalent to `folder = <resolved repo path>`. Passing both `folder` and `repo` is an error."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Target workspace basename (e.g. \"dev\", \"myws\"). Omit (or set to the current workspace name) for the default same-workspace spawn. When set to a different workspace, the tool POSTs to that workspace's engine and the coding-agent session lands there. Cross-workspace requires `relation=\"top\"` (child-thread auto-resume callbacks across workspaces are unsupported). The `repo` parameter then resolves in the TARGET workspace's repo registry — make sure the repo is registered there."
                    },
                    "allowed_tools": {
                        "type": "string",
                        "description": "Tools to auto-approve, comma-separated (default: 'Bash,Read,Edit,Write,Glob,Grep')"
                    },
                    "model": {
                        "type": "string",
                        "description": "Model override for the selected coding agent. Omit to use that backend's default."
                    },
                    "append_system_prompt": {
                        "type": "string",
                        "description": "Additional instructions to append to the coding agent's system prompt"
                    },
                    "images": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of conversation images to forward to the coding-agent session, e.g. [\"thread:1\", \"thread:3\"]. Indices match the 1-based order images appear in this thread. Omit to forward the current message's images (default). Pass an empty array to forward none."
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional short descriptive title (3-6 words) for the spawned coding-agent thread. When provided, the system will not auto-generate a title. Recommended so the thread list is meaningful at a glance."
                    },
                    "relation": {
                        "type": "string",
                        "enum": ["child", "top"],
                        "description": "How the spawned coding-agent session relates to this thread. 'child' (default): when the session ends, this thread automatically resumes with its result — use for delegated coding subtasks whose outcome you need. 'top': fire-and-forget — the session runs independently as a top-level thread; this thread does not resume when it finishes. Use when the user asks for the work to happen in a separate thread they will follow themselves. ('sub' is accepted as a back-compat alias for 'child'.)"
                    }
                },
                "required": ["prompt"]
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::tool_names as tn;

    #[test]
    fn run_coding_agent_tool_declares_coding_agent_selector() {
        let tools = spawn_tools();
        let run_coding_agent = tools
            .iter()
            .find(|tool| tool.name == tn::RUN_CODING_AGENT)
            .expect("run_coding_agent tool must be registered");
        assert!(
            tools.iter().all(|tool| tool.name != tn::RUN_CLAUDE_LEGACY),
            "legacy run_claude tool must not be exposed to new LLM calls"
        );
        let coding_agent = &run_coding_agent.parameters["properties"]["coding_agent"];
        assert_eq!(coding_agent["type"], "string");
        assert_eq!(
            coding_agent["enum"],
            serde_json::json!(["claude-code", "codex"])
        );
    }
}
